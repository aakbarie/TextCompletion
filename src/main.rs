#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod platform;
mod tray;

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use textcompletion::{
    app::{self, LibraryFilter, SaveRequest},
    autostart,
    enterprise::EnterpriseConfig,
    expansion::SharedSnippetIndex,
    instance,
    model::SnippetScope,
    runtime::{spawn_global_binding, PauseFlag, RuntimeStatus},
    storage::{SnippetRepository, SqliteSnippetRepository},
    transfer, APP_VERSION,
};
use tray::TrayAction;
use uuid::Uuid;

slint::include_modules!();

type Repo = Arc<SqliteSnippetRepository>;

fn main() {
    let dirs = match project_dirs() {
        Ok(dirs) => dirs,
        Err(error) => {
            platform::fatal(&error.to_string());
            std::process::exit(1);
        }
    };
    init_logging(&dirs);
    log::info!("Scriblet {APP_VERSION} starting");

    // One process, one keyboard hook. A second launch is told to use the tray.
    let _instance = match instance::acquire(dirs.data_local_dir()) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            platform::inform(
                "Scriblet is already running",
                "Open it from the tray icon. Only one Scriblet can run at a time so triggers are not expanded twice.",
            );
            return;
        }
        Err(error) => {
            log::warn!("single-instance lock unavailable, continuing: {error:#}");
            match run(&dirs) {
                Ok(()) => return,
                Err(error) => {
                    log::error!("{error:#}");
                    platform::fatal(&format!("{error:#}"));
                    std::process::exit(1);
                }
            }
        }
    };

    if let Err(error) = run(&dirs) {
        log::error!("{error:#}");
        platform::fatal(&format!("{error:#}"));
        std::process::exit(1);
    }
}

fn run(dirs: &ProjectDirs) -> Result<()> {
    let db_path = dirs.data_local_dir().join("scriblet.db");
    let repository: Repo =
        Arc::new(SqliteSnippetRepository::open(&db_path).with_context(|| {
            format!("failed to open snippet database at {}", db_path.display())
        })?);
    let index = SharedSnippetIndex::default();
    app::rebuild_index(repository.as_ref(), &index)?;
    let paused: PauseFlag = PauseFlag::default();

    let ui = AppWindow::new().context("failed to create Scriblet window")?;
    ui.set_app_version(APP_VERSION.into());
    ui.set_autostart_supported(autostart::supported());
    ui.set_autostart_enabled(autostart::is_enabled().unwrap_or(false));
    let enterprise_config = EnterpriseConfig::from_env();
    ui.set_enterprise_available(enterprise_config.is_some());

    {
        let ui_weak = ui.as_weak();
        let status = Arc::new(move |status: RuntimeStatus| {
            log::info!("runtime status: {status:?}");
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                ui.set_expansion_active(status == RuntimeStatus::Active);
                ui.set_expansion_status_text(status.label().into());
            });
        });
        let _binding_thread = spawn_global_binding(index.clone(), paused.clone(), status);
    }

    refresh_all(&ui, &repository)?;
    clear_editor(&ui);
    ui.set_status_text(if enterprise_config.is_some() {
        "Ready · enterprise sync configured".into()
    } else if cfg!(target_os = "windows") {
        "Ready · enterprise sync not configured".into()
    } else {
        "Ready · SQL Server sync is available on Windows".into()
    });

    wire_editor(&ui, &repository, &index);
    wire_library(&ui, &repository);
    wire_controls(&ui, &repository, &index, &paused, enterprise_config);

    if platform::tray_supported() {
        let ui_weak = ui.as_weak();
        let paused_for_tray = paused.clone();
        let handler: tray::TrayHandler = Rc::new(move |action| match action {
            TrayAction::ShowWindow => {
                if let Some(ui) = ui_weak.upgrade() {
                    let _ = ui.show();
                }
            }
            TrayAction::TogglePause => {
                let now_paused = !paused_for_tray.load(Ordering::Relaxed);
                paused_for_tray.store(now_paused, Ordering::Relaxed);
                tray::set_paused(now_paused);
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_paused(now_paused);
                }
            }
            TrayAction::Quit => {
                tray::remove();
                let _ = slint::quit_event_loop();
            }
        });
        tray::install(handler, false);

        // Closing the window keeps Scriblet running in the tray.
        ui.window()
            .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    } else {
        ui.window().on_close_requested(|| {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        });
    }

    ui.show().context("failed to show Scriblet window")?;
    slint::run_event_loop_until_quit().context("Scriblet UI exited with an error")?;
    let _ = ui.hide();
    tray::remove();
    log::info!("Scriblet exiting");
    Ok(())
}

fn wire_editor(ui: &AppWindow, repository: &Repo, index: &SharedSnippetIndex) {
    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        let index = index.clone();
        ui.on_save_snippet(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let request = SaveRequest {
                id: parse_id(&ui.get_selected_id()),
                title: ui.get_title_text().to_string(),
                category: ui.get_category_text().to_string(),
                binding: ui.get_trigger_text().to_string(),
                replacement: ui.get_replacement_text().to_string(),
                enabled: ui.get_enabled_value(),
                favorite: ui.get_favorite_value(),
            };
            match app::save_snippet(repository.as_ref(), request) {
                Ok(snippet) => {
                    ui.set_selected_id(snippet.id.to_string().into());
                    ui.set_title_text(snippet.title.into());
                    ui.set_confirm_delete(false);
                    report(&ui, &repository, &index, "Saved");
                    set_category(&ui, &snippet.category);
                }
                Err(error) => ui.set_status_text(error.to_string().into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.on_clear_form(move || {
            if let Some(ui) = ui_weak.upgrade() {
                clear_editor(&ui);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        ui.on_select_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            let Ok(Some(snippet)) = repository.get(id) else {
                return;
            };
            let binding = repository
                .bindings_for(id)
                .ok()
                .and_then(|items| items.into_iter().find(|b| b.enabled))
                .map(|b| b.value)
                .unwrap_or_default();
            let enterprise = snippet.is_enterprise();
            ui.set_selected_id(snippet.id.to_string().into());
            ui.set_title_text(snippet.title.into());
            set_category(&ui, &snippet.category);
            ui.set_trigger_text(binding.into());
            ui.set_replacement_text(snippet.replacement.into());
            ui.set_enabled_value(snippet.enabled);
            ui.set_favorite_value(snippet.favorite);
            ui.set_read_only(enterprise);
            ui.set_confirm_delete(false);
            ui.set_status_text(if enterprise {
                "Enterprise · read-only".into()
            } else {
                "Selected".into()
            });
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        let index = index.clone();
        ui.on_delete_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            match app::delete_snippet(repository.as_ref(), id) {
                Ok(()) => {
                    clear_editor(&ui);
                    report(&ui, &repository, &index, "Deleted");
                }
                Err(error) => ui.set_status_text(error.to_string().into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        let index = index.clone();
        ui.on_duplicate_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            match app::duplicate_snippet(repository.as_ref(), id) {
                Ok(duplicate) => {
                    report(
                        &ui,
                        &repository,
                        &index,
                        "Duplicated as personal snippet without binding",
                    );
                    ui.invoke_select_snippet(duplicate.id.to_string().into());
                }
                Err(error) => ui.set_status_text(error.to_string().into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        ui.on_copy_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            let Ok(Some(snippet)) = repository.get(id) else {
                return;
            };
            match arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(snippet.replacement))
            {
                Ok(()) => ui.set_status_text("Copied to clipboard".into()),
                Err(error) => ui.set_status_text(format!("Copy failed: {error}").into()),
            }
        });
    }
}

fn wire_library(ui: &AppWindow, repository: &Repo) {
    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        ui.on_search_library(move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Err(error) =
                    refresh_library(&ui, &repository, &query, &ui.get_active_filter())
                {
                    ui.set_status_text(format!("Search failed: {error}").into());
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        ui.on_filter_library(move |filter| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_active_filter(filter.clone());
                if let Err(error) =
                    refresh_library(&ui, &repository, &ui.get_search_text(), &filter)
                {
                    ui.set_status_text(format!("Filter failed: {error}").into());
                }
            }
        });
    }
}

fn wire_controls(
    ui: &AppWindow,
    repository: &Repo,
    index: &SharedSnippetIndex,
    paused: &PauseFlag,
    enterprise_config: Option<EnterpriseConfig>,
) {
    {
        let ui_weak = ui.as_weak();
        let paused = paused.clone();
        ui.on_toggle_pause(move || {
            let now_paused = !paused.load(Ordering::Relaxed);
            paused.store(now_paused, Ordering::Relaxed);
            tray::set_paused(now_paused);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_paused(now_paused);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        let index = index.clone();
        ui.on_sync_now(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(config) = enterprise_config.clone() else {
                ui.set_status_text("Enterprise sync is not configured".into());
                return;
            };
            start_enterprise_sync(&ui, &repository, &index, config);
        });
        // Initial sync in the background, only when configured.
        if ui.get_enterprise_available() {
            ui.invoke_sync_now();
        }
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        ui.on_export_snippets(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = platform::pick_export_path() else {
                ui.set_status_text("Export cancelled".into());
                return;
            };
            match transfer::export_to_file(repository.as_ref(), &path) {
                Ok(count) => ui.set_status_text(
                    format!(
                        "Exported {count} snippet{} to {}",
                        if count == 1 { "" } else { "s" },
                        path.display()
                    )
                    .into(),
                ),
                Err(error) => ui.set_status_text(format!("Export failed: {error:#}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(repository);
        let index = index.clone();
        ui.on_import_snippets(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = platform::pick_import_path() else {
                ui.set_status_text("Import cancelled".into());
                return;
            };
            match transfer::import_from_file(repository.as_ref(), &path) {
                Ok(import) => report(&ui, &repository, &index, &import.summary()),
                Err(error) => ui.set_status_text(format!("Import failed: {error:#}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.on_set_autostart(move |enabled| {
            let Some(ui) = ui_weak.upgrade() else { return };
            match autostart::set_enabled(enabled) {
                Ok(()) => ui.set_status_text(
                    if enabled {
                        "Scriblet will start at login"
                    } else {
                        "Scriblet will not start at login"
                    }
                    .into(),
                ),
                Err(error) => {
                    ui.set_autostart_enabled(autostart::is_enabled().unwrap_or(false));
                    ui.set_status_text(format!("Start at login failed: {error}").into());
                }
            }
        });
    }
}

/// Runs an enterprise sync on a background thread so a slow or unreachable
/// SQL Server never blocks the window.
fn start_enterprise_sync(
    ui: &AppWindow,
    repository: &Repo,
    index: &SharedSnippetIndex,
    config: EnterpriseConfig,
) {
    if ui.get_syncing() {
        return;
    }
    ui.set_syncing(true);
    ui.set_status_text("Syncing enterprise library…".into());

    let ui_weak = ui.as_weak();
    let repository = Arc::clone(repository);
    let index = index.clone();
    std::thread::spawn(move || {
        let outcome = run_enterprise_sync(&repository, config);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.set_syncing(false);
            match outcome {
                Ok(summary) => report(&ui, &repository, &index, &summary),
                Err(error) => {
                    log::warn!("enterprise sync failed: {error:#}");
                    ui.set_status_text(
                        format!("Offline · using cached enterprise library ({error})").into(),
                    );
                }
            }
        });
    });
}

#[cfg(target_os = "windows")]
fn run_enterprise_sync(repository: &Repo, config: EnterpriseConfig) -> Result<String> {
    use textcompletion::enterprise::{sync_enterprise, SqlServerEnterpriseSource};
    let source = SqlServerEnterpriseSource::new(config);
    let report = sync_enterprise(&source, repository.as_ref())?;
    log::info!("enterprise sync: {report:?}");
    Ok(report.summary())
}

#[cfg(not(target_os = "windows"))]
fn run_enterprise_sync(_repository: &Repo, _config: EnterpriseConfig) -> Result<String> {
    Err(anyhow!("SQL Server sync is only available on Windows"))
}

/// Rebuilds the trigger index and every library view, then shows `status`.
fn report(ui: &AppWindow, repository: &Repo, index: &SharedSnippetIndex, status: &str) {
    if let Err(error) = app::rebuild_index(repository.as_ref(), index) {
        ui.set_status_text(format!("Index refresh failed: {error}").into());
        return;
    }
    match refresh_all(ui, repository) {
        Ok(()) => ui.set_status_text(status.into()),
        Err(error) => ui.set_status_text(format!("Refresh failed: {error}").into()),
    }
}

fn refresh_all(ui: &AppWindow, repository: &Repo) -> Result<()> {
    let categories = app::categories(repository.as_ref())?
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_categories(ModelRc::from(Rc::new(VecModel::from(categories))));
    refresh_library(
        ui,
        repository,
        &ui.get_search_text(),
        &ui.get_active_filter(),
    )
}

/// Points the category dropdown at `category`, or clears the selection for a
/// custom category. Slint's ComboBox only follows `current-index`.
fn set_category(ui: &AppWindow, category: &str) {
    use slint::Model;
    let index = ui
        .get_categories()
        .iter()
        .position(|c| c.as_str().eq_ignore_ascii_case(category))
        .map_or(-1, |i| i as i32);
    ui.set_category_text(category.into());
    ui.set_category_index(index);
}

fn clear_editor(ui: &AppWindow) {
    ui.set_selected_id("".into());
    ui.set_title_text("".into());
    set_category(ui, app::DEFAULT_CATEGORY);
    ui.set_trigger_text("".into());
    ui.set_replacement_text("".into());
    ui.set_enabled_value(true);
    ui.set_favorite_value(false);
    ui.set_read_only(false);
    ui.set_confirm_delete(false);
    ui.set_status_text("Ready".into());
}

fn parse_id(value: &SharedString) -> Option<Uuid> {
    Uuid::parse_str(value.as_str()).ok()
}

fn refresh_library(ui: &AppWindow, repository: &Repo, query: &str, filter: &str) -> Result<()> {
    let filter = LibraryFilter::parse(filter);
    let bindings = app::primary_bindings(repository.as_ref())?;
    let mut rows = Vec::new();
    for snippet in repository.search(query)? {
        if !filter.matches(&snippet) {
            continue;
        }
        let preview = snippet.replacement.lines().next().unwrap_or("").to_string();
        rows.push(SnippetRow {
            id: snippet.id.to_string().into(),
            title: if snippet.title.trim().is_empty() {
                app::DEFAULT_TITLE.into()
            } else {
                snippet.title.into()
            },
            category: snippet.category.into(),
            binding: bindings
                .get(&snippet.id)
                .cloned()
                .unwrap_or_default()
                .into(),
            preview: preview.into(),
            favorite: snippet.favorite,
            enabled: snippet.enabled,
            enterprise: snippet.scope == SnippetScope::Enterprise,
        });
    }
    let count = rows.len();
    ui.set_library_count_text(
        format!("{count} snippet{}", if count == 1 { "" } else { "s" }).into(),
    );
    ui.set_snippet_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    Ok(())
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("com", "aakbarie", "Scriblet")
        .ok_or_else(|| anyhow!("unable to resolve local application data directory"))
}

fn init_logging(dirs: &ProjectDirs) {
    let log_dir: PathBuf = dirs.data_local_dir().to_path_buf();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("scriblet.log");

    // Keep the log from growing without bound: rotate once it passes 2 MB.
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 2 * 1024 * 1024 {
            let _ = std::fs::rename(&log_path, log_dir.join("scriblet.log.1"));
        }
    }

    let config = simplelog::ConfigBuilder::new()
        .set_time_format_rfc3339()
        .build();
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            let _ = simplelog::WriteLogger::init(log::LevelFilter::Info, config, file);
        }
        Err(_) => {
            let _ = simplelog::SimpleLogger::init(log::LevelFilter::Info, config);
        }
    }
}

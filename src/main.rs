#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use slint::{ModelRc, SharedString, VecModel};
use std::{rc::Rc, sync::Arc};
use textcompletion::{
    expansion::SharedSnippetIndex,
    model::{now_epoch_seconds, Binding, Snippet},
    runtime::spawn_global_binding,
    storage::{SnippetRepository, SqliteSnippetRepository},
};
use uuid::Uuid;

slint::include_modules!();

fn main() -> Result<()> {
    let db_path = database_path()?;
    let repository = Arc::new(SqliteSnippetRepository::open(&db_path)?);
    let index = SharedSnippetIndex::default();
    refresh_index(repository.as_ref(), &index)?;

    let _binding_thread = spawn_global_binding(index.clone());
    let ui = AppWindow::new().context("failed to create Scriblet window")?;
    refresh_library(&ui, repository.as_ref(), "", "All")?;

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        let index = index.clone();
        ui.on_save_snippet(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let replacement = ui.get_replacement_text().to_string();
            let binding_value = ui.get_trigger_text().trim().to_string();
            if replacement.trim().is_empty() {
                ui.set_status_text("Phrase cannot be empty".into());
                return;
            }

            let selected = parse_id(&ui.get_selected_id());
            if !binding_value.is_empty() {
                match repository.binding_collision(&binding_value, selected) {
                    Ok(true) => {
                        ui.set_status_text(format!("Binding {binding_value} is already in use").into());
                        return;
                    }
                    Err(error) => {
                        ui.set_status_text(format!("Binding check failed: {error}").into());
                        return;
                    }
                    _ => {}
                }
            }

            let mut snippet = selected
                .and_then(|id| repository.list().ok()?.into_iter().find(|s| s.id == id))
                .unwrap_or_else(|| Snippet::personal("", replacement.clone()));
            snippet.title = ui.get_title_text().trim().to_string();
            if snippet.title.is_empty() {
                snippet.title = "Untitled snippet".to_string();
            }
            snippet.category = ui.get_category_text().trim().to_string();
            if snippet.category.is_empty() {
                snippet.category = "Personal".to_string();
            }
            snippet.trigger.clear();
            snippet.replacement = replacement;
            snippet.enabled = ui.get_enabled_value();
            snippet.favorite = ui.get_favorite_value();
            snippet.updated_at = now_epoch_seconds();
            snippet.version += 1;

            let save_result = (|| -> Result<()> {
                repository.upsert(&snippet)?;
                repository.delete_bindings_for(snippet.id)?;
                if !binding_value.is_empty() {
                    repository.upsert_binding(&Binding::text(snippet.id, binding_value.clone()))?;
                }
                refresh_index(repository.as_ref(), &index)?;
                Ok(())
            })();

            match save_result {
                Ok(()) => {
                    ui.set_selected_id(snippet.id.to_string().into());
                    ui.set_status_text("Saved".into());
                    let _ = refresh_library(&ui, repository.as_ref(), &ui.get_search_text(), &ui.get_active_filter());
                }
                Err(error) => ui.set_status_text(format!("Save failed: {error}").into()),
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
        let repository = Arc::clone(&repository);
        ui.on_search_library(move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Err(error) = refresh_library(&ui, repository.as_ref(), &query, &ui.get_active_filter()) {
                    ui.set_status_text(format!("Search failed: {error}").into());
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        ui.on_filter_library(move |filter| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_active_filter(filter.clone());
                if let Err(error) = refresh_library(&ui, repository.as_ref(), &ui.get_search_text(), &filter) {
                    ui.set_status_text(format!("Filter failed: {error}").into());
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        ui.on_select_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            let Ok(snippets) = repository.list() else { return };
            let Some(snippet) = snippets.into_iter().find(|s| s.id == id) else { return };
            let binding = repository.bindings_for(id).ok()
                .and_then(|items| items.into_iter().find(|b| b.enabled))
                .map(|b| b.value)
                .unwrap_or_default();
            ui.set_selected_id(snippet.id.to_string().into());
            ui.set_title_text(snippet.title.into());
            ui.set_category_text(snippet.category.into());
            ui.set_trigger_text(binding.into());
            ui.set_replacement_text(snippet.replacement.into());
            ui.set_enabled_value(snippet.enabled);
            ui.set_favorite_value(snippet.favorite);
            ui.set_status_text("Selected".into());
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        let index = index.clone();
        ui.on_delete_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            match repository.delete(id) {
                Ok(()) => {
                    let _ = refresh_index(repository.as_ref(), &index);
                    clear_editor(&ui);
                    let _ = refresh_library(&ui, repository.as_ref(), &ui.get_search_text(), &ui.get_active_filter());
                    ui.set_status_text("Deleted".into());
                }
                Err(error) => ui.set_status_text(format!("Delete failed: {error}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        ui.on_duplicate_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            let Ok(snippets) = repository.list() else { return };
            let Some(source) = snippets.into_iter().find(|s| s.id == id) else { return };
            let mut duplicate = Snippet::personal("", source.replacement);
            duplicate.title = format!("{} copy", source.title);
            duplicate.category = source.category;
            duplicate.favorite = false;
            duplicate.enabled = source.enabled;
            match repository.upsert(&duplicate) {
                Ok(()) => {
                    let _ = refresh_library(&ui, repository.as_ref(), &ui.get_search_text(), &ui.get_active_filter());
                    ui.set_status_text("Duplicated without binding".into());
                }
                Err(error) => ui.set_status_text(format!("Duplicate failed: {error}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        ui.on_copy_snippet(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = parse_id(&id) else { return };
            let Ok(snippets) = repository.list() else { return };
            let Some(snippet) = snippets.into_iter().find(|s| s.id == id) else { return };
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(snippet.replacement)) {
                Ok(()) => ui.set_status_text("Copied to clipboard".into()),
                Err(error) => ui.set_status_text(format!("Copy failed: {error}").into()),
            }
        });
    }

    ui.run().context("Scriblet UI exited with an error")?;
    Ok(())
}

fn clear_editor(ui: &AppWindow) {
    ui.set_selected_id("".into());
    ui.set_title_text("".into());
    ui.set_category_text("Personal".into());
    ui.set_trigger_text("".into());
    ui.set_replacement_text("".into());
    ui.set_enabled_value(true);
    ui.set_favorite_value(false);
    ui.set_status_text("Ready".into());
}

fn parse_id(value: &SharedString) -> Option<Uuid> {
    Uuid::parse_str(value.as_str()).ok()
}

fn refresh_library(ui: &AppWindow, repository: &SqliteSnippetRepository, query: &str, filter: &str) -> Result<()> {
    let snippets = repository.search(query)?;
    let mut rows = Vec::new();
    for snippet in snippets {
        let keep = match filter {
            "Favorites" => snippet.favorite,
            "All" | "" => true,
            category => snippet.category.eq_ignore_ascii_case(category),
        };
        if !keep { continue; }
        let binding = repository.bindings_for(snippet.id)?
            .into_iter()
            .find(|b| b.enabled)
            .map(|b| b.value)
            .unwrap_or_default();
        let preview = snippet.replacement.lines().next().unwrap_or("");
        rows.push(SnippetRow {
            id: snippet.id.to_string().into(),
            title: if snippet.title.trim().is_empty() { "Untitled snippet".into() } else { snippet.title.into() },
            category: snippet.category.into(),
            binding: binding.into(),
            preview: preview.into(),
            favorite: snippet.favorite,
            enabled: snippet.enabled,
        });
    }
    let count = rows.len();
    ui.set_library_count_text(format!("{count} snippet{}", if count == 1 { "" } else { "s" }).into());
    ui.set_snippet_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    Ok(())
}

fn refresh_index(repository: &SqliteSnippetRepository, index: &SharedSnippetIndex) -> Result<()> {
    let snippets = repository.list()?;
    let bindings = repository.list_bindings()?;
    let by_id = snippets.into_iter().map(|s| (s.id, s)).collect::<std::collections::HashMap<_, _>>();
    let mut guard = index.write();
    guard.clear();
    for binding in bindings {
        if !binding.enabled || binding.value.trim().is_empty() { continue; }
        if let Some(snippet) = by_id.get(&binding.snippet_id) {
            if snippet.enabled {
                guard.insert(binding.value, snippet.replacement.clone());
            }
        }
    }
    Ok(())
}

fn database_path() -> Result<std::path::PathBuf> {
    let dirs = ProjectDirs::from("com", "aakbarie", "Scriblet")
        .ok_or_else(|| anyhow!("unable to resolve local application data directory"))?;
    Ok(dirs.data_local_dir().join("scriblet.db"))
}

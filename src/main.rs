use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use std::sync::Arc;
use textcompletion::{
    expansion::SharedSnippetIndex,
    model::{now_epoch_seconds, Snippet},
    runtime::spawn_global_binding,
    storage::{SnippetRepository, SqliteSnippetRepository},
};

slint::include_modules!();

fn main() -> Result<()> {
    let db_path = database_path()?;
    let repository = Arc::new(SqliteSnippetRepository::open(&db_path)?);
    let index = SharedSnippetIndex::default();
    refresh_index(repository.as_ref(), &index)?;

    // Keep the global binding thread alive for the lifetime of the UI.
    let _binding_thread = spawn_global_binding(index.clone());

    let ui = AppWindow::new().context("failed to create Scriblet window")?;

    {
        let ui_weak = ui.as_weak();
        let repository = Arc::clone(&repository);
        let index = index.clone();

        ui.on_save_snippet(move || {
            let Some(ui) = ui_weak.upgrade() else { return };

            let trigger = ui.get_trigger_text().trim().to_string();
            let replacement = ui.get_replacement_text().to_string();

            if trigger.is_empty() {
                ui.set_status_text("Trigger cannot be empty".into());
                return;
            }
            if replacement.is_empty() {
                ui.set_status_text("Replacement cannot be empty".into());
                return;
            }

            let mut snippet = Snippet::personal(trigger.clone(), replacement);
            snippet.enabled = ui.get_enabled_value();
            snippet.updated_at = now_epoch_seconds();

            match repository.upsert(&snippet) {
                Ok(()) => match refresh_index(repository.as_ref(), &index) {
                    Ok(()) => ui.set_status_text(format!("Saved {trigger}").into()),
                    Err(error) => ui.set_status_text(format!("Saved, but binding refresh failed: {error}").into()),
                },
                Err(error) => ui.set_status_text(format!("Save failed: {error}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.on_clear_form(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_trigger_text("".into());
                ui.set_replacement_text("".into());
                ui.set_enabled_value(true);
                ui.set_status_text("Ready".into());
            }
        });
    }

    ui.run().context("Scriblet UI exited with an error")?;
    Ok(())
}

fn refresh_index(
    repository: &SqliteSnippetRepository,
    index: &SharedSnippetIndex,
) -> Result<()> {
    let snippets = repository.list()?;
    let mut guard = index.write();
    guard.clear();
    for snippet in snippets {
        if snippet.enabled && !snippet.trigger.trim().is_empty() {
            guard.insert(snippet.trigger, snippet.replacement);
        }
    }
    Ok(())
}

fn database_path() -> Result<std::path::PathBuf> {
    let dirs = ProjectDirs::from("com", "aakbarie", "Scriblet")
        .ok_or_else(|| anyhow!("unable to resolve local application data directory"))?;
    Ok(dirs.data_local_dir().join("scriblet.db"))
}

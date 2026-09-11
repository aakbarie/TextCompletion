//! JSON import and export of the personal library.
//!
//! Enterprise snippets are governed centrally and are never exported; on
//! import, every snippet becomes a personal snippet.

use crate::model::{now_epoch_seconds, Binding, Snippet, SnippetScope};
use crate::storage::SnippetRepository;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

pub const FORMAT_VERSION: u32 = 1;
pub const FILE_EXTENSION: &str = "json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportDocument {
    pub format_version: u32,
    pub app_version: String,
    pub exported_at: i64,
    pub snippets: Vec<ExportedSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportedSnippet {
    pub id: Uuid,
    pub title: String,
    pub category: String,
    pub replacement: String,
    pub enabled: bool,
    pub favorite: bool,
    #[serde(default)]
    pub bindings: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    /// Bindings that were skipped because another snippet already uses them.
    pub skipped_bindings: Vec<String>,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "Imported {} snippet{}",
            self.imported,
            plural(self.imported)
        );
        if !self.skipped_bindings.is_empty() {
            text.push_str(&format!(
                " · {} binding{} skipped (in use: {})",
                self.skipped_bindings.len(),
                plural(self.skipped_bindings.len()),
                self.skipped_bindings.join(", ")
            ));
        }
        text
    }
}

pub fn export_personal<R: SnippetRepository + ?Sized>(repository: &R) -> Result<ExportDocument> {
    let mut snippets = Vec::new();
    for snippet in repository.list()? {
        if snippet.is_enterprise() {
            continue;
        }
        let bindings = repository
            .bindings_for(snippet.id)?
            .into_iter()
            .filter(|b| b.enabled)
            .map(|b| b.value)
            .collect();
        snippets.push(ExportedSnippet {
            id: snippet.id,
            title: snippet.title,
            category: snippet.category,
            replacement: snippet.replacement,
            enabled: snippet.enabled,
            favorite: snippet.favorite,
            bindings,
            updated_at: snippet.updated_at,
        });
    }
    Ok(ExportDocument {
        format_version: FORMAT_VERSION,
        app_version: crate::APP_VERSION.to_string(),
        exported_at: now_epoch_seconds(),
        snippets,
    })
}

pub fn export_to_file<R: SnippetRepository + ?Sized>(repository: &R, path: &Path) -> Result<usize> {
    let document = export_personal(repository)?;
    let json = serde_json::to_string_pretty(&document)?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(document.snippets.len())
}

pub fn import_document<R: SnippetRepository + ?Sized>(
    repository: &R,
    document: &ExportDocument,
) -> Result<ImportReport> {
    if document.format_version > FORMAT_VERSION {
        return Err(anyhow!(
            "export format {} is newer than this Scriblet understands ({})",
            document.format_version,
            FORMAT_VERSION
        ));
    }

    let mut report = ImportReport::default();
    repository.transaction(&mut || {
        report = ImportReport::default();
        for item in &document.snippets {
            if item.replacement.trim().is_empty() {
                continue;
            }

            // Never overwrite a cached enterprise snippet with the same id.
            let id = match repository.get(item.id)? {
                Some(existing) if existing.is_enterprise() => Uuid::new_v4(),
                Some(existing) => existing.id,
                None => item.id,
            };
            let version = repository
                .get(id)?
                .map(|existing| existing.version + 1)
                .unwrap_or(1);

            let snippet = Snippet {
                id,
                title: item.title.clone(),
                category: item.category.clone(),
                trigger: String::new(),
                replacement: item.replacement.clone(),
                scope: SnippetScope::Personal,
                enabled: item.enabled,
                favorite: item.favorite,
                version,
                updated_at: now_epoch_seconds(),
            };
            repository.upsert(&snippet)?;
            repository.delete_bindings_for(snippet.id)?;
            report.imported += 1;

            for value in &item.bindings {
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                if repository.binding_collision(value, Some(snippet.id))? {
                    report.skipped_bindings.push(value.to_string());
                    continue;
                }
                repository.upsert_binding(&Binding::text(snippet.id, value))?;
            }
        }
        Ok(())
    })?;
    Ok(report)
}

pub fn import_from_file<R: SnippetRepository + ?Sized>(
    repository: &R,
    path: &Path,
) -> Result<ImportReport> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let document: ExportDocument =
        serde_json::from_str(&json).context("file is not a Scriblet export")?;
    import_document(repository, &document)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteSnippetRepository;

    fn personal(
        title: &str,
        replacement: &str,
        binding: Option<&str>,
        repo: &SqliteSnippetRepository,
    ) -> Snippet {
        let mut snippet = Snippet::personal("", replacement);
        snippet.title = title.into();
        snippet.category = "Personal".into();
        repo.upsert(&snippet).unwrap();
        if let Some(binding) = binding {
            repo.upsert_binding(&Binding::text(snippet.id, binding))
                .unwrap();
        }
        snippet
    }

    #[test]
    fn roundtrip_excludes_enterprise_and_keeps_bindings() -> Result<()> {
        let source = SqliteSnippetRepository::open_in_memory()?;
        let mine = personal("Mine", "My phrase", Some(";mine"), &source);
        let mut governed = Snippet::personal("", "Governed");
        governed.scope = SnippetScope::Enterprise;
        source.upsert(&governed)?;

        let document = export_personal(&source)?;
        assert_eq!(document.snippets.len(), 1);
        assert_eq!(document.snippets[0].id, mine.id);
        assert_eq!(document.snippets[0].bindings, vec![";mine".to_string()]);

        let json = serde_json::to_string(&document)?;
        let parsed: ExportDocument = serde_json::from_str(&json)?;

        let target = SqliteSnippetRepository::open_in_memory()?;
        let report = import_document(&target, &parsed)?;
        assert_eq!(report.imported, 1);
        assert!(report.skipped_bindings.is_empty());
        let imported = target.find_by_trigger(";mine")?.unwrap();
        assert_eq!(imported.id, mine.id);
        assert_eq!(imported.title, "Mine");
        assert_eq!(imported.scope, SnippetScope::Personal);
        Ok(())
    }

    #[test]
    fn import_skips_colliding_bindings_and_reimports_in_place() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        personal("Existing", "Existing phrase", Some(";taken"), &repo);

        let document = ExportDocument {
            format_version: FORMAT_VERSION,
            app_version: "test".into(),
            exported_at: 0,
            snippets: vec![ExportedSnippet {
                id: Uuid::new_v4(),
                title: "Incoming".into(),
                category: "Clinical".into(),
                replacement: "Incoming phrase".into(),
                enabled: true,
                favorite: false,
                bindings: vec![";taken".into(), ";free".into()],
                updated_at: 0,
            }],
        };

        let report = import_document(&repo, &document)?;
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped_bindings, vec![";taken".to_string()]);
        assert_eq!(repo.find_by_trigger(";taken")?.unwrap().title, "Existing");
        assert_eq!(repo.find_by_trigger(";free")?.unwrap().title, "Incoming");

        // Importing the same document again updates rather than duplicates.
        let report = import_document(&repo, &document)?;
        assert_eq!(report.imported, 1);
        assert_eq!(repo.list()?.len(), 2);
        assert_eq!(repo.get(document.snippets[0].id)?.unwrap().version, 2);
        Ok(())
    }

    #[test]
    fn import_never_overwrites_cached_enterprise_snippet() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let mut governed = Snippet::personal("", "Governed");
        governed.scope = SnippetScope::Enterprise;
        repo.upsert(&governed)?;

        let document = ExportDocument {
            format_version: FORMAT_VERSION,
            app_version: "test".into(),
            exported_at: 0,
            snippets: vec![ExportedSnippet {
                id: governed.id,
                title: "Impostor".into(),
                category: String::new(),
                replacement: "Tampered".into(),
                enabled: true,
                favorite: false,
                bindings: vec![],
                updated_at: 0,
            }],
        };
        import_document(&repo, &document)?;
        assert_eq!(repo.get(governed.id)?.unwrap().replacement, "Governed");
        assert_eq!(repo.list()?.len(), 2);
        Ok(())
    }

    #[test]
    fn newer_format_is_rejected() {
        let repo = SqliteSnippetRepository::open_in_memory().unwrap();
        let document = ExportDocument {
            format_version: FORMAT_VERSION + 1,
            app_version: "future".into(),
            exported_at: 0,
            snippets: vec![],
        };
        assert!(import_document(&repo, &document).is_err());
    }

    #[test]
    fn file_roundtrip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("export.json");
        let repo = SqliteSnippetRepository::open_in_memory()?;
        personal("A", "Alpha", Some(";a"), &repo);
        assert_eq!(export_to_file(&repo, &path)?, 1);

        let target = SqliteSnippetRepository::open_in_memory()?;
        let report = import_from_file(&target, &path)?;
        assert_eq!(report.imported, 1);
        assert!(target.find_by_trigger(";a")?.is_some());
        assert!(import_from_file(&target, &dir.path().join("missing.json")).is_err());
        Ok(())
    }
}

use crate::note::{validate_markdown_path, NoteDocument};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub schema_version: u32,
    pub last_opened_note: Option<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            last_opened_note: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let workspace = Self { root: root.into() };
        fs::create_dir_all(workspace.notes_dir())?;
        fs::create_dir_all(workspace.attachments_dir())?;
        fs::create_dir_all(workspace.state_dir())?;
        if !workspace.config_path().exists() {
            workspace.save_config(&WorkspaceConfig::default())?;
        }
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    pub fn attachments_dir(&self) -> PathBuf {
        self.root.join("attachments")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".scriblet")
    }

    pub fn config_path(&self) -> PathBuf {
        self.state_dir().join("workspace.json")
    }

    pub fn events_path(&self) -> PathBuf {
        self.state_dir().join("events.sqlite")
    }

    pub fn index_path(&self) -> PathBuf {
        self.state_dir().join("index.sqlite")
    }

    pub fn new_note(&self, title: &str) -> Result<NoteDocument> {
        let slug = unique_slug(&self.notes_dir(), title);
        let path = self.notes_dir().join(format!("{slug}.md"));
        let note = NoteDocument::new(path, title, Utc::now())?;
        self.save_note(&note)?;
        Ok(note)
    }

    pub fn open_note(&self, path: impl AsRef<Path>) -> Result<NoteDocument> {
        let path = normalize_note_path(&self.notes_dir(), path.as_ref());
        validate_markdown_path(&path)?;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read note {}", path.display()))?;
        NoteDocument::from_markdown(path, &source)
    }

    /// Crash-safe write: write, flush and fsync a temporary file in the same
    /// directory, then atomically persist it over the target.
    pub fn save_note(&self, note: &NoteDocument) -> Result<()> {
        validate_markdown_path(&note.path)?;
        let parent = note.path.parent().context("note has no parent directory")?;
        fs::create_dir_all(parent)?;
        let mut temp = NamedTempFile::new_in(parent)?;
        temp.write_all(note.markdown().as_bytes())?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        temp.persist(&note.path)
            .map_err(|e| e.error)
            .with_context(|| format!("failed to atomically save {}", note.path.display()))?;
        sync_parent(parent);
        Ok(())
    }

    pub fn rename_note(&self, note: &mut NoteDocument, new_name: &str) -> Result<()> {
        let new_path = self.notes_dir().join(format!("{}.md", slugify(new_name)));
        validate_markdown_path(&new_path)?;
        fs::rename(&note.path, &new_path).with_context(|| {
            format!("failed to rename {} to {}", note.path.display(), new_path.display())
        })?;
        note.path = new_path;
        sync_parent(&self.notes_dir());
        Ok(())
    }

    pub fn delete_note(&self, note: &NoteDocument) -> Result<()> {
        fs::remove_file(&note.path)
            .with_context(|| format!("failed to delete {}", note.path.display()))?;
        sync_parent(&self.notes_dir());
        Ok(())
    }

    pub fn list_notes(&self) -> Result<Vec<PathBuf>> {
        let mut notes = Vec::new();
        for entry in fs::read_dir(self.notes_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                notes.push(path);
            }
        }
        notes.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .map(std::cmp::Reverse)
        });
        Ok(notes)
    }

    pub fn load_config(&self) -> Result<WorkspaceConfig> {
        let source = fs::read_to_string(self.config_path())?;
        Ok(serde_json::from_str(&source)?)
    }

    pub fn save_config(&self, config: &WorkspaceConfig) -> Result<()> {
        let source = serde_json::to_vec_pretty(config)?;
        atomic_write(&self.config_path(), &source)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    sync_parent(parent);
    Ok(())
}

fn normalize_note_path(notes_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { notes_dir.join(path) }
}

fn unique_slug(notes_dir: &Path, title: &str) -> String {
    let base = slugify(title);
    if !notes_dir.join(format!("{base}.md")).exists() {
        return base;
    }
    for suffix in 2..10_000 {
        let candidate = format!("{base}-{suffix}");
        if !notes_dir.join(format!("{candidate}.md")).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", Utc::now().timestamp())
}

pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in input.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "untitled".to_string() } else { out }
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_creates_open_layout_and_round_trips_note() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let mut note = workspace.new_note("Policy Review").unwrap();
        note.body.push_str("Evidence stays in Markdown.\n");
        workspace.save_note(&note).unwrap();
        let loaded = workspace.open_note(note.path.clone()).unwrap();
        assert_eq!(loaded.body, note.body);
        assert!(workspace.events_path().ends_with("events.sqlite"));
    }

    #[test]
    fn slug_is_human_readable_and_filesystem_safe() {
        assert_eq!(slugify("  Medical Necessity / Review  "), "medical-necessity-review");
        assert_eq!(slugify("***"), "untitled");
    }
}
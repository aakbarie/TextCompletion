use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Stable identity for a note inside a Scriblet workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoteId(pub Uuid);

impl NoteId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for NoteId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDocument {
    pub id: NoteId,
    pub path: PathBuf,
    /// Front matter without the surrounding `---` delimiters. Kept as raw text so
    /// Scriblet never rewrites metadata it does not understand.
    pub front_matter: Option<String>,
    /// Markdown body exactly as authored, excluding optional front matter.
    pub body: String,
}

impl NoteDocument {
    pub fn from_markdown(path: impl Into<PathBuf>, markdown: &str) -> Result<Self> {
        let path = path.into();
        validate_markdown_path(&path)?;
        let (front_matter, body) = split_front_matter(markdown);
        Ok(Self {
            id: NoteId::new(),
            path,
            front_matter,
            body,
        })
    }

    pub fn new(path: impl Into<PathBuf>, title: &str, now: DateTime<Utc>) -> Result<Self> {
        let path = path.into();
        validate_markdown_path(&path)?;
        let title = if title.trim().is_empty() { "Untitled" } else { title.trim() };
        let metadata = format!(
            "type: note\nschema: scriblet/note/v1\ncreated: {}\nupdated: {}",
            now.to_rfc3339(),
            now.to_rfc3339()
        );
        Ok(Self {
            id: NoteId::new(),
            path,
            front_matter: Some(metadata),
            body: format!("# {title}\n\n"),
        })
    }

    pub fn markdown(&self) -> String {
        match &self.front_matter {
            Some(front) => format!("---\n{}\n---\n\n{}", front.trim_end(), self.body),
            None => self.body.clone(),
        }
    }

    pub fn display_title(&self) -> String {
        self.body
            .lines()
            .find_map(|line| line.trim().strip_prefix("# "))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "Untitled".to_string())
    }
}

pub fn validate_markdown_path(path: &Path) -> Result<()> {
    if path.extension().and_then(|s| s.to_str()) != Some("md") {
        bail!("Scriblet notes must use the .md extension");
    }
    Ok(())
}

/// Split optional YAML front matter while preserving both halves verbatim.
/// Ordinary Markdown without front matter is always accepted.
pub fn split_front_matter(markdown: &str) -> (Option<String>, String) {
    let normalized = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    if !(normalized.starts_with("---\n") || normalized.starts_with("---\r\n")) {
        return (None, normalized.to_string());
    }

    let first_newline = normalized.find('\n').unwrap_or(3) + 1;
    let rest = &normalized[first_newline..];
    let mut offset = first_newline;
    for segment in rest.split_inclusive('\n') {
        let line = segment.trim_end_matches(|c| c == '\r' || c == '\n');
        if line == "---" {
            let front = &normalized[first_newline..offset];
            let body_start = offset + segment.len();
            let body = normalized[body_start..]
                .strip_prefix('\n')
                .unwrap_or(&normalized[body_start..]);
            return (
                Some(
                    front
                        .trim_end_matches(|c| c == '\r' || c == '\n')
                        .to_string(),
                ),
                body.to_string(),
            );
        }
        offset += segment.len();
    }

    // An unterminated leading `---` is ordinary Markdown, not corrupt metadata.
    (None, normalized.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn ordinary_markdown_round_trips_without_conversion() {
        let source = "# Existing note\n\nKeep **everything** as-is.\n";
        let note = NoteDocument::from_markdown("existing.md", source).unwrap();
        assert_eq!(note.front_matter, None);
        assert_eq!(note.markdown(), source);
    }

    #[test]
    fn front_matter_is_preserved_as_raw_text() {
        let source = "---\ntype: clinical\ncustom: untouched\n---\n\n# Review\nBody\n";
        let note = NoteDocument::from_markdown("review.md", source).unwrap();
        assert_eq!(note.front_matter.as_deref(), Some("type: clinical\ncustom: untouched"));
        assert_eq!(note.body, "# Review\nBody\n");
        assert_eq!(note.markdown(), source);
    }

    #[test]
    fn new_note_uses_open_markdown_with_optional_metadata() {
        let now = Utc.with_ymd_and_hms(2026, 9, 13, 20, 0, 0).unwrap();
        let note = NoteDocument::new("notes/idea.md", "Idea", now).unwrap();
        assert!(note.markdown().contains("schema: scriblet/note/v1"));
        assert!(note.markdown().ends_with("# Idea\n\n"));
    }
}

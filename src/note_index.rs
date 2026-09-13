use crate::workspace::Workspace;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSearchHit {
    pub path: String,
    pub title: String,
    pub preview: String,
}

pub struct NoteIndex {
    conn: Connection,
}

impl NoteIndex {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS notes (
                path TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                modified_unix INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_notes_title ON notes(title);"
        )?;
        Ok(Self { conn })
    }

    /// The index is disposable state: rebuilding only reads canonical Markdown.
    pub fn rebuild(&mut self, workspace: &Workspace) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM notes", [])?;
        let mut count = 0usize;
        for path in workspace.list_notes()? {
            let note = workspace.open_note(&path)?;
            let modified = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            tx.execute(
                "INSERT INTO notes(path, title, body, modified_unix) VALUES (?1, ?2, ?3, ?4)",
                params![
                    note.path.to_string_lossy().to_string(),
                    note.display_title(),
                    note.body,
                    modified
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<NoteSearchHit>> {
        let trimmed = query.trim();
        let mut stmt = if trimmed.is_empty() {
            self.conn.prepare(
                "SELECT path, title, body FROM notes ORDER BY modified_unix DESC LIMIT ?1",
            )?
        } else {
            self.conn.prepare(
                "SELECT path, title, body FROM notes
                 WHERE lower(title) LIKE ?1 OR lower(body) LIKE ?1
                 ORDER BY modified_unix DESC LIMIT ?2",
            )?
        };

        let mut hits = Vec::new();
        if trimmed.is_empty() {
            let rows = stmt.query_map([limit as i64], row_to_hit)?;
            for row in rows {
                hits.push(row?);
            }
        } else {
            let needle = format!("%{}%", trimmed.to_lowercase());
            let rows = stmt.query_map(params![needle, limit as i64], row_to_hit)?;
            for row in rows {
                hits.push(row?);
            }
        }
        Ok(hits)
    }
}

fn row_to_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteSearchHit> {
    let body: String = row.get(2)?;
    let preview = body
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    Ok(NoteSearchHit {
        path: row.get(0)?,
        title: row.get(1)?,
        preview: preview.chars().take(180).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_rebuilds_from_markdown_and_finds_body_text() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path().join("workspace")).unwrap();
        let mut note = workspace.new_note("Medical Necessity").unwrap();
        note.body.push_str("The member meets policy criteria after conservative therapy.\n");
        workspace.save_note(&note).unwrap();

        let mut index = NoteIndex::open(&workspace.index_path()).unwrap();
        assert_eq!(index.rebuild(&workspace).unwrap(), 1);
        let hits = index.search("conservative therapy", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Medical Necessity");
    }
}
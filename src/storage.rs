use crate::model::{Snippet, SnippetScope};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;
use uuid::Uuid;

pub trait SnippetRepository: Send + Sync {
    fn list(&self) -> Result<Vec<Snippet>>;
    fn search(&self, query: &str) -> Result<Vec<Snippet>>;
    fn find_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>>;
    fn upsert(&self, snippet: &Snippet) -> Result<()>;
    fn delete(&self, id: Uuid) -> Result<()>;
}

pub struct SqliteSnippetRepository {
    conn: Mutex<Connection>,
}

impl SqliteSnippetRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;

        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS snippets (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                category TEXT NOT NULL DEFAULT '',
                trigger TEXT NOT NULL UNIQUE,
                replacement TEXT NOT NULL,
                scope TEXT NOT NULL DEFAULT 'personal',
                enabled INTEGER NOT NULL DEFAULT 1,
                version INTEGER NOT NULL DEFAULT 1,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_snippets_trigger_enabled
                ON snippets(trigger, enabled);
            CREATE INDEX IF NOT EXISTS idx_snippets_category
                ON snippets(category);
            "#,
        )?;

        ensure_column(&conn, "snippets", "title", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "snippets", "category", "TEXT NOT NULL DEFAULT ''")?;

        Ok(Self { conn: Mutex::new(conn) })
    }
}

impl SnippetRepository for SqliteSnippetRepository {
    fn list(&self) -> Result<Vec<Snippet>> {
        self.query_snippets(
            "SELECT id, title, category, trigger, replacement, scope, enabled, version, updated_at FROM snippets ORDER BY category COLLATE NOCASE, title COLLATE NOCASE, trigger COLLATE NOCASE",
            [],
        )
    }

    fn search(&self, query: &str) -> Result<Vec<Snippet>> {
        let pattern = format!("%{}%", query.trim());
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, category, trigger, replacement, scope, enabled, version, updated_at\n             FROM snippets\n             WHERE title LIKE ?1 COLLATE NOCASE\n                OR category LIKE ?1 COLLATE NOCASE\n                OR trigger LIKE ?1 COLLATE NOCASE\n                OR replacement LIKE ?1 COLLATE NOCASE\n             ORDER BY category COLLATE NOCASE, title COLLATE NOCASE, trigger COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![pattern], row_to_snippet)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    fn find_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, category, trigger, replacement, scope, enabled, version, updated_at\n             FROM snippets WHERE trigger = ?1 AND enabled = 1 LIMIT 1",
        )?;

        let mut rows = stmt.query(params![trigger])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_snippet(row)?)),
            None => Ok(None),
        }
    }

    fn upsert(&self, snippet: &Snippet) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO snippets(id, title, category, trigger, replacement, scope, enabled, version, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                category = excluded.category,
                trigger = excluded.trigger,
                replacement = excluded.replacement,
                scope = excluded.scope,
                enabled = excluded.enabled,
                version = excluded.version,
                updated_at = excluded.updated_at
            "#,
            params![
                snippet.id.to_string(),
                snippet.title,
                snippet.category,
                snippet.trigger,
                snippet.replacement,
                snippet.scope.as_str(),
                snippet.enabled as i32,
                snippet.version,
                snippet.updated_at,
            ],
        )?;
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM snippets WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }
}

impl SqliteSnippetRepository {
    fn query_snippets<P>(&self, sql: &str, params: P) -> Result<Vec<Snippet>>
    where
        P: rusqlite::Params,
    {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, row_to_snippet)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if !columns.iter().any(|name| name == column) {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition};"))?;
    }
    Ok(())
}

fn row_to_snippet(row: &rusqlite::Row<'_>) -> rusqlite::Result<Snippet> {
    let id: String = row.get(0)?;
    let scope: String = row.get(5)?;

    Ok(Snippet {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        title: row.get(1)?,
        category: row.get(2)?,
        trigger: row.get(3)?,
        replacement: row.get(4)?,
        scope: SnippetScope::from_str(&scope),
        enabled: row.get::<_, i32>(6)? != 0,
        version: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

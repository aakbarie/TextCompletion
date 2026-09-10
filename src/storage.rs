use crate::model::{Binding, BindingKind, Snippet, SnippetScope};
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
    fn list_bindings(&self) -> Result<Vec<Binding>>;
    fn bindings_for(&self, snippet_id: Uuid) -> Result<Vec<Binding>>;
    fn upsert_binding(&self, binding: &Binding) -> Result<()>;
    fn delete_bindings_for(&self, snippet_id: Uuid) -> Result<()>;
    fn binding_collision(&self, value: &str, excluding_snippet: Option<Uuid>) -> Result<bool>;
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
                favorite INTEGER NOT NULL DEFAULT 0,
                version INTEGER NOT NULL DEFAULT 1,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS bindings (
                id TEXT PRIMARY KEY,
                snippet_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                value TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                FOREIGN KEY(snippet_id) REFERENCES snippets(id) ON DELETE CASCADE
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_bindings_kind_value
                ON bindings(kind, value);
            CREATE INDEX IF NOT EXISTS idx_bindings_snippet
                ON bindings(snippet_id);
            CREATE INDEX IF NOT EXISTS idx_snippets_category
                ON snippets(category);
            "#,
        )?;

        ensure_column(&conn, "snippets", "title", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "snippets", "category", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "snippets", "favorite", "INTEGER NOT NULL DEFAULT 0")?;

        // Compatibility migration: genuine pre-v0.3 triggers become text bindings.
        // Internal sentinels used for unbound snippets must never become user-visible bindings.
        conn.execute_batch(
            r#"
            DELETE FROM bindings
            WHERE kind = 'text' AND value LIKE '__scriblet_unbound_%';

            INSERT OR IGNORE INTO bindings(id, snippet_id, kind, value, enabled)
            SELECT lower(hex(randomblob(16))), id, 'text', trigger, enabled
            FROM snippets
            WHERE trim(trigger) <> ''
              AND trigger NOT LIKE '__scriblet_unbound_%';
            "#,
        )?;

        Ok(Self { conn: Mutex::new(conn) })
    }
}

impl SnippetRepository for SqliteSnippetRepository {
    fn list(&self) -> Result<Vec<Snippet>> {
        self.query_snippets(
            "SELECT id, title, category, trigger, replacement, scope, enabled, favorite, version, updated_at FROM snippets ORDER BY favorite DESC, category COLLATE NOCASE, title COLLATE NOCASE, trigger COLLATE NOCASE",
            [],
        )
    }

    fn search(&self, query: &str) -> Result<Vec<Snippet>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list();
        }
        let pattern = format!("%{}%", trimmed);
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT s.id, s.title, s.category, s.trigger, s.replacement, s.scope, s.enabled, s.favorite, s.version, s.updated_at\n             FROM snippets s\n             LEFT JOIN bindings b ON b.snippet_id = s.id\n             WHERE s.title LIKE ?1 COLLATE NOCASE\n                OR s.category LIKE ?1 COLLATE NOCASE\n                OR s.replacement LIKE ?1 COLLATE NOCASE\n                OR b.value LIKE ?1 COLLATE NOCASE\n             ORDER BY s.favorite DESC, s.category COLLATE NOCASE, s.title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![pattern], row_to_snippet)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    fn find_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.category, s.trigger, s.replacement, s.scope, s.enabled, s.favorite, s.version, s.updated_at\n             FROM snippets s\n             JOIN bindings b ON b.snippet_id = s.id\n             WHERE b.kind = 'text' AND b.value = ?1 AND b.enabled = 1 AND s.enabled = 1 LIMIT 1",
        )?;

        let mut rows = stmt.query(params![trigger])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_snippet(row)?)),
            None => Ok(None),
        }
    }

    fn upsert(&self, snippet: &Snippet) -> Result<()> {
        let conn = self.conn.lock();
        let legacy_trigger = if snippet.trigger.trim().is_empty() {
            format!("__scriblet_unbound_{}", snippet.id)
        } else {
            snippet.trigger.clone()
        };
        conn.execute(
            r#"
            INSERT INTO snippets(id, title, category, trigger, replacement, scope, enabled, favorite, version, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                category = excluded.category,
                trigger = excluded.trigger,
                replacement = excluded.replacement,
                scope = excluded.scope,
                enabled = excluded.enabled,
                favorite = excluded.favorite,
                version = excluded.version,
                updated_at = excluded.updated_at
            "#,
            params![
                snippet.id.to_string(),
                snippet.title,
                snippet.category,
                legacy_trigger,
                snippet.replacement,
                snippet.scope.as_str(),
                snippet.enabled as i32,
                snippet.favorite as i32,
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

    fn list_bindings(&self) -> Result<Vec<Binding>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, snippet_id, kind, value, enabled FROM bindings ORDER BY kind, value COLLATE NOCASE")?;
        let rows = stmt.query_map([], row_to_binding)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    fn bindings_for(&self, snippet_id: Uuid) -> Result<Vec<Binding>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, snippet_id, kind, value, enabled FROM bindings WHERE snippet_id = ?1 ORDER BY kind, value COLLATE NOCASE")?;
        let rows = stmt.query_map(params![snippet_id.to_string()], row_to_binding)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    fn upsert_binding(&self, binding: &Binding) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO bindings(id, snippet_id, kind, value, enabled) VALUES (?1, ?2, ?3, ?4, ?5)\n             ON CONFLICT(id) DO UPDATE SET snippet_id=excluded.snippet_id, kind=excluded.kind, value=excluded.value, enabled=excluded.enabled",
            params![binding.id.to_string(), binding.snippet_id.to_string(), binding.kind.as_str(), binding.value, binding.enabled as i32],
        )?;
        Ok(())
    }

    fn delete_bindings_for(&self, snippet_id: Uuid) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM bindings WHERE snippet_id = ?1", params![snippet_id.to_string()])?;
        Ok(())
    }

    fn binding_collision(&self, value: &str, excluding_snippet: Option<Uuid>) -> Result<bool> {
        let conn = self.conn.lock();
        let count: i64 = match excluding_snippet {
            Some(id) => conn.query_row(
                "SELECT COUNT(*) FROM bindings WHERE kind='text' AND value=?1 AND snippet_id<>?2",
                params![value, id.to_string()],
                |row| row.get(0),
            )?,
            None => conn.query_row(
                "SELECT COUNT(*) FROM bindings WHERE kind='text' AND value=?1",
                params![value],
                |row| row.get(0),
            )?,
        };
        Ok(count > 0)
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
    let trigger: String = row.get(3)?;

    Ok(Snippet {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        title: row.get(1)?,
        category: row.get(2)?,
        trigger: if trigger.starts_with("__scriblet_unbound_") { String::new() } else { trigger },
        replacement: row.get(4)?,
        scope: SnippetScope::from_str(&scope),
        enabled: row.get::<_, i32>(6)? != 0,
        favorite: row.get::<_, i32>(7)? != 0,
        version: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<Binding> {
    let id: String = row.get(0)?;
    let snippet_id: String = row.get(1)?;
    let kind: String = row.get(2)?;
    Ok(Binding {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        snippet_id: Uuid::parse_str(&snippet_id).unwrap_or_else(|_| Uuid::new_v4()),
        kind: BindingKind::from_str(&kind),
        value: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
    })
}

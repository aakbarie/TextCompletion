use crate::model::{Binding, BindingKind, Snippet, SnippetScope};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;
use uuid::Uuid;

/// Schema version recorded in SQLite's `user_version` pragma.
///
/// - 0: pre-v0.5 databases, migrated on open
/// - 1: v0.5 (legacy `trigger` column migrated into `bindings` exactly once)
pub const SCHEMA_VERSION: i64 = 1;

const UNBOUND_SENTINEL_PREFIX: &str = "__scriblet_unbound_";

pub trait SnippetRepository: Send + Sync {
    fn list(&self) -> Result<Vec<Snippet>>;
    fn get(&self, id: Uuid) -> Result<Option<Snippet>>;
    fn search(&self, query: &str) -> Result<Vec<Snippet>>;
    fn find_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>>;
    fn upsert(&self, snippet: &Snippet) -> Result<()>;
    fn delete(&self, id: Uuid) -> Result<()>;
    fn list_categories(&self) -> Result<Vec<String>>;
    fn list_bindings(&self) -> Result<Vec<Binding>>;
    fn bindings_for(&self, snippet_id: Uuid) -> Result<Vec<Binding>>;
    fn upsert_binding(&self, binding: &Binding) -> Result<()>;
    fn delete_bindings_for(&self, snippet_id: Uuid) -> Result<()>;
    fn binding_collision(&self, value: &str, excluding_snippet: Option<Uuid>) -> Result<bool>;
    /// Runs `work` inside a single transaction. The transaction is committed
    /// when `work` returns `Ok` and rolled back otherwise. Nested calls join
    /// the outer transaction.
    fn transaction(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()>;
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

        let conn =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        Self::initialize(conn)
    }

    /// Opens a private in-memory database. Useful for tests and previews.
    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(conn: Connection) -> Result<Self> {
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
            "#,
        )?;

        ensure_column(&conn, "snippets", "title", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "snippets", "category", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "snippets", "favorite", "INTEGER NOT NULL DEFAULT 0")?;
        // Must follow the column migration: pre-v0.3 databases have no category column.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_snippets_category ON snippets(category);",
        )?;

        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version < 1 {
            log::info!("migrating snippet database from schema version {user_version} to 1");
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
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn schema_version(&self) -> Result<i64> {
        let conn = self.conn.lock();
        Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }
}

const SNIPPET_COLUMNS: &str =
    "id, title, category, trigger, replacement, scope, enabled, favorite, version, updated_at";

impl SnippetRepository for SqliteSnippetRepository {
    fn list(&self) -> Result<Vec<Snippet>> {
        self.query_snippets(
            &format!(
                "SELECT {SNIPPET_COLUMNS} FROM snippets \
                 ORDER BY favorite DESC, category COLLATE NOCASE, title COLLATE NOCASE, trigger COLLATE NOCASE"
            ),
            [],
        )
    }

    fn get(&self, id: Uuid) -> Result<Option<Snippet>> {
        let rows = self.query_snippets(
            &format!("SELECT {SNIPPET_COLUMNS} FROM snippets WHERE id = ?1"),
            params![id.to_string()],
        )?;
        Ok(rows.into_iter().next())
    }

    fn search(&self, query: &str) -> Result<Vec<Snippet>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list();
        }
        let pattern = format!("%{}%", trimmed);
        self.query_snippets(
            "SELECT DISTINCT s.id, s.title, s.category, s.trigger, s.replacement, s.scope, s.enabled, s.favorite, s.version, s.updated_at
             FROM snippets s
             LEFT JOIN bindings b ON b.snippet_id = s.id
             WHERE s.title LIKE ?1 COLLATE NOCASE
                OR s.category LIKE ?1 COLLATE NOCASE
                OR s.replacement LIKE ?1 COLLATE NOCASE
                OR b.value LIKE ?1 COLLATE NOCASE
             ORDER BY s.favorite DESC, s.category COLLATE NOCASE, s.title COLLATE NOCASE",
            params![pattern],
        )
    }

    fn find_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>> {
        let rows = self.query_snippets(
            "SELECT s.id, s.title, s.category, s.trigger, s.replacement, s.scope, s.enabled, s.favorite, s.version, s.updated_at
             FROM snippets s
             JOIN bindings b ON b.snippet_id = s.id
             WHERE b.kind = 'text' AND b.value = ?1 AND b.enabled = 1 AND s.enabled = 1 LIMIT 1",
            params![trigger],
        )?;
        Ok(rows.into_iter().next())
    }

    fn upsert(&self, snippet: &Snippet) -> Result<()> {
        let conn = self.conn.lock();
        let legacy_trigger = if snippet.trigger.trim().is_empty() {
            format!("{UNBOUND_SENTINEL_PREFIX}{}", snippet.id)
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
        conn.execute(
            "DELETE FROM snippets WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn list_categories(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT category FROM snippets WHERE trim(category) <> '' ORDER BY category COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn list_bindings(&self) -> Result<Vec<Binding>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, snippet_id, kind, value, enabled FROM bindings ORDER BY kind, value COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], row_to_binding)?;
        Ok(collect_valid(rows, "binding"))
    }

    fn bindings_for(&self, snippet_id: Uuid) -> Result<Vec<Binding>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, snippet_id, kind, value, enabled FROM bindings WHERE snippet_id = ?1 ORDER BY kind, value COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![snippet_id.to_string()], row_to_binding)?;
        Ok(collect_valid(rows, "binding"))
    }

    fn upsert_binding(&self, binding: &Binding) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO bindings(id, snippet_id, kind, value, enabled) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET snippet_id=excluded.snippet_id, kind=excluded.kind, value=excluded.value, enabled=excluded.enabled",
            params![
                binding.id.to_string(),
                binding.snippet_id.to_string(),
                binding.kind.as_str(),
                binding.value,
                binding.enabled as i32
            ],
        )
        .with_context(|| format!("binding {} could not be saved", binding.value))?;
        Ok(())
    }

    fn delete_bindings_for(&self, snippet_id: Uuid) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM bindings WHERE snippet_id = ?1",
            params![snippet_id.to_string()],
        )?;
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

    fn transaction(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        {
            let conn = self.conn.lock();
            if !conn.is_autocommit() {
                // Already inside a transaction: join it.
                drop(conn);
                return work();
            }
            conn.execute_batch("BEGIN IMMEDIATE")?;
        }

        let outcome = work();

        let conn = self.conn.lock();
        match outcome {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(error) => {
                if let Err(rollback_error) = conn.execute_batch("ROLLBACK") {
                    log::error!("rollback failed after {error}: {rollback_error}");
                }
                Err(error)
            }
        }
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
        Ok(collect_valid(rows, "snippet"))
    }
}

/// Collects rows, skipping and logging any row that fails to decode so one
/// corrupt record cannot hide the whole library.
fn collect_valid<T>(rows: impl Iterator<Item = rusqlite::Result<T>>, what: &str) -> Vec<T> {
    rows.filter_map(|row| match row {
        Ok(value) => Some(value),
        Err(error) => {
            log::warn!("skipping unreadable {what} row: {error}");
            None
        }
    })
    .collect()
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if !columns.iter().any(|name| name == column) {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

fn parse_uuid_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let raw: String = row.get(index)?;
    Uuid::parse_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn row_to_snippet(row: &rusqlite::Row<'_>) -> rusqlite::Result<Snippet> {
    let scope: String = row.get(5)?;
    let trigger: String = row.get(3)?;

    Ok(Snippet {
        id: parse_uuid_column(row, 0)?,
        title: row.get(1)?,
        category: row.get(2)?,
        trigger: if trigger.starts_with(UNBOUND_SENTINEL_PREFIX) {
            String::new()
        } else {
            trigger
        },
        replacement: row.get(4)?,
        scope: SnippetScope::parse(&scope),
        enabled: row.get::<_, i32>(6)? != 0,
        favorite: row.get::<_, i32>(7)? != 0,
        version: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<Binding> {
    let kind: String = row.get(2)?;
    Ok(Binding {
        id: parse_uuid_column(row, 0)?,
        snippet_id: parse_uuid_column(row, 1)?,
        kind: BindingKind::parse(&kind),
        value: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
    })
}

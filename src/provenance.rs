use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEvent {
    pub id: Uuid,
    pub event_type: String,
    pub note_id: Option<String>,
    pub actor: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub content_hash: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
}

impl ProvenanceEvent {
    pub fn new(event_type: impl Into<String>, actor: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.into(),
            note_id: None,
            actor: actor.into(),
            provider: None,
            model: None,
            content_hash: None,
            metadata_json: None,
            created_at: Utc::now().to_rfc3339(),
        }
    }

    pub fn with_note(mut self, note_id: impl Into<String>) -> Self {
        self.note_id = Some(note_id.into());
        self
    }

    pub fn with_generated_content(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        accepted_text: &str,
    ) -> Self {
        self.provider = Some(provider.into());
        self.model = Some(model.into());
        self.content_hash = Some(content_fingerprint(accepted_text));
        self
    }
}

pub struct ProvenanceStore {
    conn: Connection,
}

impl ProvenanceStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                event_type TEXT NOT NULL,
                note_id TEXT,
                actor TEXT NOT NULL,
                provider TEXT,
                model TEXT,
                content_hash TEXT,
                metadata_json TEXT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_note_seq ON events(note_id, seq);
             CREATE INDEX IF NOT EXISTS idx_events_type_seq ON events(event_type, seq);"
        )?;
        Ok(Self { conn })
    }

    /// Append-only by API design. There is deliberately no update or delete
    /// method; correction is represented by a later semantic event.
    pub fn append(&self, event: &ProvenanceEvent) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events
             (id, event_type, note_id, actor, provider, model, content_hash, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.id.to_string(),
                event.event_type,
                event.note_id,
                event.actor,
                event.provider,
                event.model,
                event.content_hash,
                event.metadata_json,
                event.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn events_for_note(&self, note_id: &str) -> Result<Vec<ProvenanceEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, event_type, note_id, actor, provider, model, content_hash, metadata_json, created_at
             FROM events WHERE note_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([note_id], |row| {
            let id: String = row.get(0)?;
            Ok(ProvenanceEvent {
                id: Uuid::parse_str(&id).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                event_type: row.get(1)?,
                note_id: row.get(2)?,
                actor: row.get(3)?,
                provider: row.get(4)?,
                model: row.get(5)?,
                content_hash: row.get(6)?,
                metadata_json: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// A deterministic non-secret fingerprint for audit correlation. It is not used
/// for security decisions and intentionally avoids retaining generated text.
pub fn content_fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_store_is_append_only_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProvenanceStore::open(&dir.path().join("events.sqlite")).unwrap();
        store.append(&ProvenanceEvent::new("note.created", "human").with_note("n1")).unwrap();
        store.append(&ProvenanceEvent::new("note.saved", "human").with_note("n1")).unwrap();
        let events = store.events_for_note("n1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "note.created");
        assert_eq!(events[1].event_type, "note.saved");
    }

    #[test]
    fn generated_text_is_fingerprinted_not_stored() {
        let event = ProvenanceEvent::new("completion.accepted", "human")
            .with_generated_content("mock", "deterministic-v1", "sensitive generated sentence");
        assert!(event.content_hash.as_deref().unwrap().starts_with("fnv1a64:"));
        assert!(!format!("{event:?}").contains("sensitive generated sentence"));
    }
}
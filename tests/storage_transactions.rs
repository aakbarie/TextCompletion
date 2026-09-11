use anyhow::{anyhow, Result};
use rusqlite::Connection;
use tempfile::tempdir;
use textcompletion::model::{Binding, Snippet};
use textcompletion::storage::{SnippetRepository, SqliteSnippetRepository, SCHEMA_VERSION};

#[test]
fn transaction_rolls_back_every_write_on_error() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let snippet = Snippet::personal("", "Phrase");

    let outcome = repo.transaction(&mut || {
        repo.upsert(&snippet)?;
        repo.upsert_binding(&Binding::text(snippet.id, ";x"))?;
        Err(anyhow!("simulated failure"))
    });

    assert!(outcome.is_err());
    assert!(repo.get(snippet.id)?.is_none());
    assert!(repo.find_by_trigger(";x")?.is_none());
    Ok(())
}

#[test]
fn transaction_commits_and_nested_calls_join_the_outer_one() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let snippet = Snippet::personal("", "Phrase");

    repo.transaction(&mut || {
        repo.upsert(&snippet)?;
        repo.transaction(&mut || repo.upsert_binding(&Binding::text(snippet.id, ";y")))
    })?;

    assert_eq!(repo.find_by_trigger(";y")?.unwrap().id, snippet.id);
    Ok(())
}

#[test]
fn get_and_categories() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let mut a = Snippet::personal("", "A");
    a.category = "Clinical".into();
    let mut b = Snippet::personal("", "B");
    b.category = "admin".into();
    let mut blank = Snippet::personal("", "C");
    blank.category = "  ".into();
    for s in [&a, &b, &blank] {
        repo.upsert(s)?;
    }

    assert_eq!(repo.get(a.id)?.unwrap().replacement, "A");
    assert!(repo.get(uuid::Uuid::new_v4())?.is_none());
    assert_eq!(
        repo.list_categories()?,
        vec!["admin".to_string(), "Clinical".to_string()]
    );
    Ok(())
}

#[test]
fn legacy_trigger_column_is_migrated_exactly_once() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("legacy.db");

    // A pre-v0.3 database: triggers live on the snippet row and there is no bindings table.
    {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE snippets (
                id TEXT PRIMARY KEY,
                trigger TEXT NOT NULL UNIQUE,
                replacement TEXT NOT NULL,
                scope TEXT NOT NULL DEFAULT 'personal',
                enabled INTEGER NOT NULL DEFAULT 1,
                version INTEGER NOT NULL DEFAULT 1,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO snippets(id, trigger, replacement, updated_at)
            VALUES ('3f2504e0-4f89-11d3-9a0c-0305e82c3301', ';old', 'Old phrase', 1);",
        )?;
    }

    {
        let repo = SqliteSnippetRepository::open(&path)?;
        assert_eq!(repo.schema_version()?, SCHEMA_VERSION);
        let migrated = repo
            .find_by_trigger(";old")?
            .expect("legacy trigger became a binding");
        assert_eq!(migrated.replacement, "Old phrase");
        assert_eq!(migrated.trigger, ";old");

        // The user removes the binding; reopening must not resurrect it.
        repo.delete_bindings_for(migrated.id)?;
        let mut unbound = migrated.clone();
        unbound.trigger.clear();
        repo.upsert(&unbound)?;
    }

    let repo = SqliteSnippetRepository::open(&path)?;
    assert!(repo.find_by_trigger(";old")?.is_none());
    assert_eq!(repo.list()?.len(), 1);
    assert_eq!(repo.list()?[0].trigger, "");
    Ok(())
}

#[test]
fn unreadable_rows_are_skipped_rather_than_failing_the_library() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("corrupt.db");
    {
        let repo = SqliteSnippetRepository::open(&path)?;
        repo.upsert(&Snippet::personal("", "Good"))?;
    }
    {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "INSERT INTO snippets(id, trigger, replacement, updated_at) VALUES ('not-a-uuid', ';bad', 'Bad', 1);",
        )?;
    }
    let repo = SqliteSnippetRepository::open(&path)?;
    let list = repo.list()?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].replacement, "Good");
    Ok(())
}

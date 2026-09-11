//! End-to-end user acceptance scenarios run through the same service layer
//! the window uses. Each test is one story a Scriblet user would act out.
//! They stop short of the keyboard hook and the window, which need a desktop;
//! those steps live in docs/uat-plan.md.

use anyhow::{anyhow, Result};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;
use textcompletion::{
    app::{self, LibraryFilter, SaveRequest, SnippetError},
    enterprise::{sync_enterprise, EnterpriseRecord, EnterpriseSource},
    expansion::{ExpansionMatcher, SharedSnippetIndex},
    model::{Binding, BindingKind, Snippet, SnippetScope},
    runtime::plan_injection,
    storage::{SnippetRepository, SqliteSnippetRepository},
    template::render_template,
    transfer,
};
use uuid::Uuid;

/// A user typing a trigger into "an application": feeds the matcher exactly
/// as the hook would, then returns the injection plan the worker would type.
fn type_trigger(index: &SharedSnippetIndex, trigger: &str, delimiter: char) -> Option<String> {
    let mut matcher = ExpansionMatcher::new(index.clone());
    for ch in trigger.chars() {
        assert!(matcher.feed_char(ch).is_none(), "{trigger} expanded early");
    }
    let expansion = matcher.feed_char(delimiter)?;
    let plan = plan_injection(&expansion);
    assert_eq!(plan.backspaces, trigger.chars().count());
    assert_eq!(plan.trailing, delimiter);
    Some(plan.text)
}

fn request(title: &str, category: &str, binding: &str, phrase: &str) -> SaveRequest {
    SaveRequest {
        id: None,
        title: title.into(),
        category: category.into(),
        binding: binding.into(),
        replacement: phrase.into(),
        enabled: true,
        favorite: false,
    }
}

struct FakeServer(Mutex<Result<Vec<EnterpriseRecord>, String>>);

impl EnterpriseSource for FakeServer {
    fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
        match &*self.0.lock().unwrap() {
            Ok(records) => Ok(records.clone()),
            Err(reason) => Err(anyhow!("{reason}")),
        }
    }
}

fn governed(title: &str, category: &str, trigger: &str, phrase: &str) -> EnterpriseRecord {
    let id = Uuid::new_v4();
    EnterpriseRecord {
        snippet: Snippet {
            id,
            title: title.into(),
            category: category.into(),
            trigger: String::new(),
            replacement: phrase.into(),
            scope: SnippetScope::Enterprise,
            enabled: true,
            favorite: false,
            version: 1,
            updated_at: 1,
        },
        binding: Some(Binding {
            id: Uuid::new_v4(),
            snippet_id: id,
            kind: BindingKind::Text,
            value: trigger.into(),
            enabled: true,
        }),
    }
}

#[test]
fn md_creates_template_snippet_and_expands_it_with_caret_placement() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let index = SharedSnippetIndex::default();

    let saved = app::save_snippet(
        &repo,
        request(
            "Peer to peer",
            "Clinical",
            ";p2p",
            "Peer-to-peer review completed on {{date}}. Rationale: {{cursor}}",
        ),
    )?;
    app::rebuild_index(&repo, &index)?;

    let typed = type_trigger(&index, ";p2p", ' ').expect("binding expands");
    assert!(typed.starts_with("Peer-to-peer review completed on "));
    assert!(
        !typed.contains("{{date}}"),
        "date must be rendered at expansion time"
    );
    assert!(!typed.contains("{{cursor}}"));
    assert!(typed.ends_with("Rationale: "));

    // The library shows it under its category with its binding.
    let bindings = app::primary_bindings(&repo)?;
    assert_eq!(bindings.get(&saved.id).map(String::as_str), Some(";p2p"));
    assert!(app::categories(&repo)?.contains(&"Clinical".to_string()));
    Ok(())
}

#[test]
fn rn_edits_binding_and_old_trigger_stops_working() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let index = SharedSnippetIndex::default();
    let saved = app::save_snippet(
        &repo,
        request("Outreach", "RN", ";out", "Outreach attempted."),
    )?;
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";out", '\t').is_some());

    let mut edit = request("Outreach", "RN", ";outreach", "Member outreach attempted.");
    edit.id = Some(saved.id);
    app::save_snippet(&repo, edit)?;
    app::rebuild_index(&repo, &index)?;

    assert!(
        type_trigger(&index, ";out", '\t').is_none(),
        "old binding must be gone"
    );
    assert_eq!(
        type_trigger(&index, ";outreach", '\n').as_deref(),
        Some("Member outreach attempted.")
    );
    Ok(())
}

#[test]
fn disabling_a_snippet_stops_expansion_until_re_enabled() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let index = SharedSnippetIndex::default();
    let saved = app::save_snippet(&repo, request("Sig", "Signatures", ";sig", "Dr. Example"))?;

    let mut off = request("Sig", "Signatures", ";sig", "Dr. Example");
    off.id = Some(saved.id);
    off.enabled = false;
    app::save_snippet(&repo, off.clone())?;
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";sig", ' ').is_none());

    off.enabled = true;
    app::save_snippet(&repo, off)?;
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";sig", ' ').is_some());
    Ok(())
}

#[test]
fn duplicate_binding_is_refused_with_a_clear_message() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    app::save_snippet(&repo, request("A", "Personal", ";same", "first"))?;
    let error = app::save_snippet(&repo, request("B", "Personal", ";same", "second")).unwrap_err();
    assert_eq!(error.to_string(), "Binding ;same is already in use");
    assert_eq!(
        repo.list()?.len(),
        1,
        "the refused snippet must not be saved"
    );
    Ok(())
}

#[test]
fn library_survives_restart() -> Result<()> {
    let dir = tempdir()?;
    let db = dir.path().join("scriblet.db");
    let id = {
        let repo = SqliteSnippetRepository::open(&db)?;
        app::save_snippet(
            &repo,
            request("Addr", "Administrative", ";addr", "123 Main St"),
        )?
        .id
    };

    // "Relaunch": open the same file and rebuild the index from disk.
    let repo = SqliteSnippetRepository::open(&db)?;
    let index = SharedSnippetIndex::default();
    app::rebuild_index(&repo, &index)?;
    assert_eq!(repo.get(id)?.unwrap().title, "Addr");
    assert_eq!(
        type_trigger(&index, ";addr", ' ').as_deref(),
        Some("123 Main St")
    );
    Ok(())
}

#[test]
fn upgrade_from_v0_4_database_keeps_bindings_without_duplicating_them() -> Result<()> {
    let dir = tempdir()?;
    let db = dir.path().join("scriblet.db");
    // A v0.4.x database: bindings table exists, user_version is still 0.
    {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute_batch(
            "CREATE TABLE snippets (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', category TEXT NOT NULL DEFAULT '',
                trigger TEXT NOT NULL UNIQUE, replacement TEXT NOT NULL, scope TEXT NOT NULL DEFAULT 'personal',
                enabled INTEGER NOT NULL DEFAULT 1, favorite INTEGER NOT NULL DEFAULT 0, version INTEGER NOT NULL DEFAULT 1,
                updated_at INTEGER NOT NULL);
             CREATE TABLE bindings (id TEXT PRIMARY KEY, snippet_id TEXT NOT NULL, kind TEXT NOT NULL, value TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1);
             CREATE UNIQUE INDEX idx_bindings_kind_value ON bindings(kind, value);
             INSERT INTO snippets(id, title, category, trigger, replacement, updated_at)
               VALUES ('5f4d3c2b-1a09-4f8e-9d7c-6b5a4f3e2d1c', 'Legacy', 'Clinical', '__scriblet_unbound_5f4d3c2b-1a09-4f8e-9d7c-6b5a4f3e2d1c', 'Legacy phrase', 1);
             INSERT INTO bindings(id, snippet_id, kind, value, enabled)
               VALUES ('0e1d2c3b-4a59-4687-9f0e-1d2c3b4a5968', '5f4d3c2b-1a09-4f8e-9d7c-6b5a4f3e2d1c', 'text', ';legacy', 1);",
        )?;
    }

    let repo = SqliteSnippetRepository::open(&db)?;
    let index = SharedSnippetIndex::default();
    app::rebuild_index(&repo, &index)?;
    assert_eq!(
        repo.list_bindings()?.len(),
        1,
        "no duplicate binding after upgrade"
    );
    assert_eq!(
        type_trigger(&index, ";legacy", ' ').as_deref(),
        Some("Legacy phrase")
    );
    Ok(())
}

#[test]
fn enterprise_library_arrives_is_read_only_and_survives_going_offline() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let index = SharedSnippetIndex::default();
    app::save_snippet(&repo, request("Mine", "Personal", ";mine", "my phrase"))?;

    let approved = governed(
        "Approved determination",
        "Medical Director",
        ";agree",
        "I agree with the determination.",
    );
    let server = FakeServer(Mutex::new(Ok(vec![approved.clone()])));

    // Day 1: on the network.
    let report = sync_enterprise(&server, &repo)?;
    assert_eq!(report.snippets_upserted, 1);
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";agree", ' ').is_some());
    assert!(type_trigger(&index, ";mine", ' ').is_some());

    // Governed content cannot be edited or deleted, but can be copied.
    let mut edit = request("Tampered", "x", ";agree", "changed");
    edit.id = Some(approved.snippet.id);
    assert!(matches!(
        app::save_snippet(&repo, edit),
        Err(SnippetError::EnterpriseReadOnly)
    ));
    assert!(matches!(
        app::delete_snippet(&repo, approved.snippet.id),
        Err(SnippetError::EnterpriseReadOnly)
    ));
    let copy = app::duplicate_snippet(&repo, approved.snippet.id)?;
    assert_eq!(copy.scope, SnippetScope::Personal);

    // Day 2: off the VPN. The cache keeps serving.
    *server.0.lock().unwrap() = Err("login timeout expired".into());
    assert!(sync_enterprise(&server, &repo).is_err());
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";agree", ' ').is_some());

    // Day 3: content retired centrally.
    *server.0.lock().unwrap() = Ok(vec![]);
    let report = sync_enterprise(&server, &repo)?;
    assert_eq!(report.snippets_removed, 1);
    app::rebuild_index(&repo, &index)?;
    assert!(type_trigger(&index, ";agree", ' ').is_none());
    assert!(
        type_trigger(&index, ";mine", ' ').is_some(),
        "personal library untouched"
    );
    assert!(!LibraryFilter::parse("enterprise").matches(&repo.get(copy.id)?.unwrap()));
    Ok(())
}

#[test]
fn enterprise_trigger_clashing_with_personal_one_leaves_personal_in_charge() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let index = SharedSnippetIndex::default();
    app::save_snippet(
        &repo,
        request("My sig", "Signatures", ";sig", "my signature"),
    )?;

    let server = FakeServer(Mutex::new(Ok(vec![governed(
        "Corporate sig",
        "Signatures",
        ";sig",
        "CORPORATE",
    )])));
    let report = sync_enterprise(&server, &repo)?;
    assert_eq!(report.skipped_bindings, vec![";sig".to_string()]);
    assert!(report.summary().contains(";sig"));

    app::rebuild_index(&repo, &index)?;
    assert_eq!(
        type_trigger(&index, ";sig", ' ').as_deref(),
        Some("my signature")
    );
    Ok(())
}

#[test]
fn saving_while_enterprise_sync_fails_keeps_the_save() -> Result<()> {
    let dir = tempdir()?;
    let repo = Arc::new(SqliteSnippetRepository::open(
        dir.path().join("scriblet.db"),
    )?);
    let gate = Arc::new(Barrier::new(2));

    struct SlowServer;
    impl EnterpriseSource for SlowServer {
        fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
            Ok(vec![governed("Slow", "Clinical", ";slow", "slow phrase")])
        }
    }

    // The sync writes, then fails, while a user save happens in between.
    let repo_sync = Arc::clone(&repo);
    let gate_sync = Arc::clone(&gate);
    let sync = thread::spawn(move || {
        repo_sync.transaction(&mut || {
            sync_enterprise(&SlowServer, repo_sync.as_ref())?;
            gate_sync.wait();
            thread::sleep(Duration::from_millis(200));
            Err(anyhow!("SQL Server dropped the connection"))
        })
    });

    let repo_user = Arc::clone(&repo);
    let gate_user = Arc::clone(&gate);
    let user = thread::spawn(move || {
        gate_user.wait();
        app::save_snippet(
            repo_user.as_ref(),
            request("Typed during sync", "Personal", ";during", "kept"),
        )
    });

    assert!(sync.join().unwrap().is_err());
    let saved = user.join().unwrap()?;

    assert!(
        repo.get(saved.id)?.is_some(),
        "user's save must survive the failed sync"
    );
    assert!(
        repo.find_by_trigger(";slow")?.is_none(),
        "failed sync leaves no enterprise rows"
    );
    Ok(())
}

#[test]
fn export_on_one_machine_imports_on_another() -> Result<()> {
    let dir = tempdir()?;
    let laptop = SqliteSnippetRepository::open_in_memory()?;
    app::save_snippet(
        &laptop,
        request("Addr", "Administrative", ";addr", "123 Main St"),
    )?;
    app::save_snippet(&laptop, request("Unbound", "Personal", "", "no trigger"))?;
    let server = FakeServer(Mutex::new(Ok(vec![governed(
        "Gov", "Clinical", ";gov", "governed",
    )])));
    sync_enterprise(&server, &laptop)?;

    let file = dir.path().join("scriblet-export.json");
    assert_eq!(
        transfer::export_to_file(&laptop, &file)?,
        2,
        "enterprise content is not exported"
    );

    let desktop = SqliteSnippetRepository::open_in_memory()?;
    app::save_snippet(
        &desktop,
        request("Existing", "Personal", ";addr", "different address"),
    )?;
    let report = transfer::import_from_file(&desktop, &file)?;
    assert_eq!(report.imported, 2);
    assert_eq!(report.skipped_bindings, vec![";addr".to_string()]);

    let index = SharedSnippetIndex::default();
    app::rebuild_index(&desktop, &index)?;
    assert_eq!(
        type_trigger(&index, ";addr", ' ').as_deref(),
        Some("different address")
    );
    assert_eq!(desktop.list()?.len(), 3);
    Ok(())
}

#[test]
fn clipboard_text_with_markers_is_typed_literally_and_caret_lands_once() {
    let rendered = render_template(
        "Member: {{clipboard}}\nNotes: {{cursor}}\nReviewed {{date}}{{cursor}}",
        Some("09/11/2026"),
        Some("Jane {{cursor}} Doe"),
    );
    assert_eq!(
        rendered.text,
        "Member: Jane {{cursor}} Doe\nNotes: \nReviewed 09/11/2026"
    );
    assert_eq!(
        rendered.cursor_offset_from_end,
        Some("\nReviewed 09/11/2026".chars().count())
    );
}

#[test]
fn search_and_filters_find_what_a_user_expects() -> Result<()> {
    let repo = SqliteSnippetRepository::open_in_memory()?;
    let fav = app::save_snippet(
        &repo,
        request(
            "Peer to peer",
            "Clinical",
            ";p2p",
            "Peer-to-peer review completed.",
        ),
    )?;
    let mut mark = request(
        "Peer to peer",
        "Clinical",
        ";p2p",
        "Peer-to-peer review completed.",
    );
    mark.id = Some(fav.id);
    mark.favorite = true;
    app::save_snippet(&repo, mark)?;
    app::save_snippet(
        &repo,
        request("Outreach", "RN", ";out", "Member outreach attempted."),
    )?;

    assert_eq!(repo.search("peer")?.len(), 1);
    assert_eq!(repo.search(";out")?.len(), 1);
    assert_eq!(repo.search("attempted")?.len(), 1);
    assert_eq!(repo.search("nothing here")?.len(), 0);

    let all = repo.list()?;
    assert_eq!(all[0].id, fav.id, "favorites sort first");
    assert_eq!(
        all.iter()
            .filter(|s| LibraryFilter::parse("favorites").matches(s))
            .count(),
        1
    );
    assert_eq!(
        all.iter()
            .filter(|s| LibraryFilter::parse("category:rn").matches(s))
            .count(),
        1
    );
    assert_eq!(
        all.iter()
            .filter(|s| LibraryFilter::parse("personal").matches(s))
            .count(),
        2
    );
    assert_eq!(
        app::categories(&repo)?,
        vec!["Administrative", "Clinical", "Personal", "RN", "Signatures"]
    );
    Ok(())
}

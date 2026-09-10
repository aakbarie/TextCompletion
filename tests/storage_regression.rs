use textcompletion::model::{Binding, Snippet};
use textcompletion::storage::{SnippetRepository, SqliteSnippetRepository};
use tempfile::tempdir;

#[test]
fn persists_and_finds_snippet_by_trigger() {
    let dir = tempdir().unwrap();
    let db = SqliteSnippetRepository::open(dir.path().join("uat.db")).unwrap();
    let mut snippet = Snippet::personal("", "I agree with the determination.");
    snippet.title = "Agree with determination".into();
    snippet.category = "Medical Director".into();
    db.upsert(&snippet).unwrap();
    db.upsert_binding(&Binding::text(snippet.id, ";agree")).unwrap();

    let loaded = db.find_by_trigger(";agree").unwrap().unwrap();
    assert_eq!(loaded.title, "Agree with determination");
    assert_eq!(loaded.category, "Medical Director");
    assert_eq!(loaded.replacement, "I agree with the determination.");
}

#[test]
fn search_matches_title_category_trigger_and_replacement() {
    let dir = tempdir().unwrap();
    let db = SqliteSnippetRepository::open(dir.path().join("search.db")).unwrap();

    let mut md = Snippet::personal("", "Peer-to-peer review completed.");
    md.title = "Peer to peer".into();
    md.category = "Medical Director".into();
    db.upsert(&md).unwrap();
    db.upsert_binding(&Binding::text(md.id, ";p2p")).unwrap();

    let mut rn = Snippet::personal("", "Member outreach attempted.");
    rn.title = "Outreach".into();
    rn.category = "RN".into();
    db.upsert(&rn).unwrap();
    db.upsert_binding(&Binding::text(rn.id, ";outreach")).unwrap();

    assert_eq!(db.search("peer").unwrap().len(), 1);
    assert_eq!(db.search("RN").unwrap().len(), 1);
    assert_eq!(db.search(";outreach").unwrap().len(), 1);
}

#[test]
fn disabled_snippet_does_not_expand() {
    let dir = tempdir().unwrap();
    let db = SqliteSnippetRepository::open(dir.path().join("disabled.db")).unwrap();
    let mut snippet = Snippet::personal("", "Should not expand");
    snippet.enabled = false;
    db.upsert(&snippet).unwrap();
    db.upsert_binding(&Binding::text(snippet.id, ";off")).unwrap();

    assert!(db.find_by_trigger(";off").unwrap().is_none());
}

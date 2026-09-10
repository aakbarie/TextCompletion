use tempfile::tempdir;
use textcompletion::{
    model::{Binding, Snippet},
    storage::{SnippetRepository, SqliteSnippetRepository},
};

#[test]
fn snippet_can_exist_without_binding() {
    let dir = tempdir().unwrap();
    let repo = SqliteSnippetRepository::open(dir.path().join("scriblet.db")).unwrap();
    let snippet = Snippet::personal("", "Reusable phrase");
    repo.upsert(&snippet).unwrap();

    assert!(repo.bindings_for(snippet.id).unwrap().is_empty());
    assert_eq!(repo.list().unwrap().len(), 1);
}

#[test]
fn text_binding_is_separate_and_resolves_snippet() {
    let dir = tempdir().unwrap();
    let repo = SqliteSnippetRepository::open(dir.path().join("scriblet.db")).unwrap();
    let snippet = Snippet::personal("", "Peer to peer approved");
    repo.upsert(&snippet).unwrap();
    repo.upsert_binding(&Binding::text(snippet.id, ";p2p")).unwrap();

    let found = repo.find_by_trigger(";p2p").unwrap().unwrap();
    assert_eq!(found.id, snippet.id);
    assert_eq!(found.replacement, "Peer to peer approved");
}

#[test]
fn duplicate_binding_is_detected() {
    let dir = tempdir().unwrap();
    let repo = SqliteSnippetRepository::open(dir.path().join("scriblet.db")).unwrap();
    let first = Snippet::personal("", "First");
    let second = Snippet::personal("", "Second");
    repo.upsert(&first).unwrap();
    repo.upsert(&second).unwrap();
    repo.upsert_binding(&Binding::text(first.id, ";same")).unwrap();

    assert!(repo.binding_collision(";same", Some(second.id)).unwrap());
    assert!(!repo.binding_collision(";same", Some(first.id)).unwrap());
}

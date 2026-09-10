use tempfile::tempdir;
use textcompletion::{
    expansion::{ExpansionMatcher, SharedSnippetIndex},
    model::{Binding, Snippet},
    storage::{SnippetRepository, SqliteSnippetRepository},
};

#[test]
fn smoke_save_load_bind_expand() {
    let dir = tempdir().expect("create temporary directory");
    let repository = SqliteSnippetRepository::open(dir.path().join("scriblet.db"))
        .expect("open SQLite repository");

    let mut snippet = Snippet::personal("", "Peer-to-peer review completed.");
    snippet.title = "Peer-to-peer review".into();
    repository.upsert(&snippet).expect("save snippet");
    repository
        .upsert_binding(&Binding::text(snippet.id, ";p2p"))
        .expect("save binding");

    let loaded = repository
        .find_by_trigger(";p2p")
        .expect("query snippet")
        .expect("saved trigger exists");
    assert_eq!(loaded.replacement, "Peer-to-peer review completed.");

    let index = SharedSnippetIndex::default();
    index
        .write()
        .insert(";p2p".into(), loaded.replacement.clone());

    let mut matcher = ExpansionMatcher::new(index);
    for ch in ";p2p".chars() {
        assert!(matcher.feed_char(ch).is_none());
    }
    let expansion = matcher.feed_char(' ').expect("binding expands");

    assert_eq!(expansion.backspaces, 4);
    assert_eq!(expansion.replacement, "Peer-to-peer review completed.");
    assert_eq!(expansion.trailing, ' ');
}

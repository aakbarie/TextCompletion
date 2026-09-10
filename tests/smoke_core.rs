use tempfile::tempdir;
use textcompletion::{
    expansion::{ExpansionMatcher, SharedSnippetIndex},
    model::Snippet,
    storage::{SnippetRepository, SqliteSnippetRepository},
};

#[test]
fn smoke_save_load_bind_expand() {
    let dir = tempdir().expect("create temporary directory");
    let repository = SqliteSnippetRepository::open(dir.path().join("scriblet.db"))
        .expect("open SQLite repository");

    let snippet = Snippet::personal(";p2p", "Peer-to-peer review completed.");
    repository.upsert(&snippet).expect("save snippet");

    let loaded = repository
        .find_by_trigger(";p2p")
        .expect("query snippet")
        .expect("saved trigger exists");
    assert_eq!(loaded.replacement, "Peer-to-peer review completed.");

    let index = SharedSnippetIndex::default();
    index
        .write()
        .insert(loaded.trigger.clone(), loaded.replacement.clone());

    let mut matcher = ExpansionMatcher::new(index);
    for ch in ";p2p".chars() {
        assert!(matcher.feed_char(ch).is_none());
    }
    let expansion = matcher.feed_char(' ').expect("binding expands");

    assert_eq!(expansion.backspaces, 4);
    assert_eq!(expansion.replacement, "Peer-to-peer review completed.");
    assert_eq!(expansion.trailing, ' ');
}

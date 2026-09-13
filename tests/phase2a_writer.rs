use textcompletion::completion::{CompletionProvider, CompletionRequest, CompletionSession, MockCompletionProvider};
use textcompletion::note_index::NoteIndex;
use textcompletion::provenance::{ProvenanceEvent, ProvenanceStore};
use textcompletion::workspace::Workspace;

#[test]
fn native_writer_flow_is_open_local_and_auditable() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(dir.path().join("scriblet-workspace")).unwrap();
    let mut note = workspace.new_note("Clinical Review").unwrap();

    note.body.push_str("The member has completed conservative therapy.");
    workspace.save_note(&note).unwrap();

    let provider = MockCompletionProvider::new(" The available evidence supports the determination.");
    let request = CompletionRequest::new(note.id.clone(), note.body.clone(), "");
    let mut session = CompletionSession::default();
    let token = session.begin(&request);
    let candidate = provider.complete(&request, &token).unwrap();
    assert!(session.show(candidate));

    // A proposal remains outside canonical Markdown until the person accepts it.
    let before_accept = workspace.open_note(&note.path).unwrap();
    assert!(!before_accept.body.contains("supports the determination"));

    let accepted = session.accept().unwrap();
    note.body.push_str(&accepted.text);
    workspace.save_note(&note).unwrap();

    let events = ProvenanceStore::open(&workspace.events_path()).unwrap();
    events
        .append(
            &ProvenanceEvent::new("completion.accepted", "human")
                .with_note(note.id.0.to_string())
                .with_generated_content(&accepted.provider, &accepted.model, &accepted.text),
        )
        .unwrap();

    let mut index = NoteIndex::open(&workspace.index_path()).unwrap();
    index.rebuild(&workspace).unwrap();
    let hits = index.search("supports the determination", 10).unwrap();
    assert_eq!(hits.len(), 1);

    let reopened = workspace.open_note(&note.path).unwrap();
    assert!(reopened.body.contains("supports the determination"));
    let audit = events.events_for_note(&note.id.0.to_string()).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].event_type, "completion.accepted");
    assert!(audit[0].content_hash.is_some());
}
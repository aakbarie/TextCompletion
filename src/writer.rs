use crate::completion::{CompletionCandidate, CompletionProvider, CompletionRequest, CompletionSession};
use crate::note::NoteDocument;
use crate::note_index::{NoteIndex, NoteSearchHit};
use crate::provenance::{ProvenanceEvent, ProvenanceStore};
use crate::workspace::Workspace;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::Arc;

/// Application service for the native work surface. UI code talks to this
/// layer rather than reaching into Markdown IO, SQLite, or model providers.
pub struct WriterSession {
    workspace: Workspace,
    index: NoteIndex,
    provenance: ProvenanceStore,
    provider: Arc<dyn CompletionProvider>,
    current: Option<NoteDocument>,
    completion: CompletionSession,
}

impl WriterSession {
    pub fn open(workspace: Workspace, provider: Arc<dyn CompletionProvider>) -> Result<Self> {
        let mut index = NoteIndex::open(&workspace.index_path())?;
        index.rebuild(&workspace)?;
        let provenance = ProvenanceStore::open(&workspace.events_path())?;
        Ok(Self {
            workspace,
            index,
            provenance,
            provider,
            current: None,
            completion: CompletionSession::default(),
        })
    }

    pub fn current(&self) -> Option<&NoteDocument> {
        self.current.as_ref()
    }

    pub fn visible_completion(&self) -> Option<&CompletionCandidate> {
        self.completion.visible()
    }

    pub fn create_note(&mut self, title: &str) -> Result<&NoteDocument> {
        self.completion.cancel();
        let note = self.workspace.new_note(title)?;
        self.provenance.append(
            &ProvenanceEvent::new("note.created", "human").with_note(note.id.0.clone()),
        )?;
        self.current = Some(note);
        self.index.rebuild(&self.workspace)?;
        Ok(self.current.as_ref().expect("current note was just assigned"))
    }

    pub fn open_note(&mut self, path: impl AsRef<Path>) -> Result<&NoteDocument> {
        self.completion.cancel();
        self.current = Some(self.workspace.open_note(path)?);
        Ok(self.current.as_ref().expect("current note was just assigned"))
    }

    pub fn replace_body(&mut self, body: impl Into<String>) -> Result<()> {
        self.completion.cancel();
        self.require_current_mut()?.body = body.into();
        Ok(())
    }

    pub fn save(&mut self) -> Result<()> {
        self.completion.cancel();
        let note = self.require_current()?.clone();
        self.workspace.save_note(&note)?;
        self.provenance.append(
            &ProvenanceEvent::new("note.saved", "human").with_note(note.id.0.clone()),
        )?;
        self.index.rebuild(&self.workspace)?;
        Ok(())
    }

    pub fn rename(&mut self, new_name: &str) -> Result<()> {
        self.completion.cancel();
        let old_path = self.require_current()?.path.to_string_lossy().to_string();
        let workspace = self.workspace.clone();
        workspace.rename_note(self.require_current_mut()?, new_name)?;
        let note = self.require_current()?;
        let metadata = serde_json::json!({
            "from": old_path,
            "to": note.path.to_string_lossy()
        })
        .to_string();
        let mut event = ProvenanceEvent::new("note.renamed", "human").with_note(note.id.0.clone());
        event.metadata_json = Some(metadata);
        self.provenance.append(&event)?;
        self.index.rebuild(&self.workspace)?;
        Ok(())
    }

    pub fn delete_current(&mut self) -> Result<()> {
        self.completion.cancel();
        let note = self.require_current()?.clone();
        self.workspace.delete_note(&note)?;
        self.provenance.append(
            &ProvenanceEvent::new("note.deleted", "human").with_note(note.id.0.clone()),
        )?;
        self.current = None;
        self.index.rebuild(&self.workspace)?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<NoteSearchHit>> {
        self.index.search(query, limit)
    }

    /// Request an inline proposal for the exact caret position. The proposal is
    /// held outside the Markdown until `accept_completion` is called.
    pub fn request_completion(&mut self, cursor_byte: usize) -> Result<Option<&CompletionCandidate>> {
        let note = self.require_current()?.clone();
        validate_cursor(&note.body, cursor_byte)?;
        let request = CompletionRequest::new(
            note.id.clone(),
            &note.body[..cursor_byte],
            &note.body[cursor_byte..],
        );
        let token = self.completion.begin(&request);
        let candidate = self.provider.complete(&request, &token)?;
        if candidate.text.is_empty() || !self.completion.show(candidate) {
            return Ok(None);
        }
        let visible = self.completion.visible().expect("candidate was just made visible");
        let mut event = ProvenanceEvent::new("completion.proposed", "model")
            .with_note(note.id.0.clone());
        event.provider = Some(visible.provider.clone());
        event.model = Some(visible.model.clone());
        self.provenance.append(&event)?;
        Ok(self.completion.visible())
    }

    pub fn accept_completion(&mut self, cursor_byte: usize) -> Result<String> {
        let note_id = self.require_current()?.id.0.clone();
        validate_cursor(&self.require_current()?.body, cursor_byte)?;
        let candidate = self.completion.accept().context("no visible completion to accept")?;
        self.require_current_mut()?.body.insert_str(cursor_byte, &candidate.text);
        let note = self.require_current()?.clone();
        self.workspace.save_note(&note)?;
        self.provenance.append(
            &ProvenanceEvent::new("completion.accepted", "human")
                .with_note(note_id)
                .with_generated_content(&candidate.provider, &candidate.model, &candidate.text),
        )?;
        self.index.rebuild(&self.workspace)?;
        Ok(candidate.text)
    }

    pub fn reject_completion(&mut self) -> Result<bool> {
        let Some(candidate) = self.completion.reject() else {
            return Ok(false);
        };
        let note_id = self.require_current()?.id.0.clone();
        let mut event = ProvenanceEvent::new("completion.rejected", "human").with_note(note_id);
        event.provider = Some(candidate.provider);
        event.model = Some(candidate.model);
        self.provenance.append(&event)?;
        Ok(true)
    }

    pub fn cancel_completion(&mut self) {
        self.completion.cancel();
    }

    fn require_current(&self) -> Result<&NoteDocument> {
        self.current.as_ref().context("no note is open")
    }

    fn require_current_mut(&mut self) -> Result<&mut NoteDocument> {
        self.current.as_mut().context("no note is open")
    }
}

fn validate_cursor(text: &str, cursor_byte: usize) -> Result<()> {
    if cursor_byte > text.len() {
        bail!("caret is beyond the end of the note");
    }
    if !text.is_char_boundary(cursor_byte) {
        bail!("caret is not on a UTF-8 character boundary");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::MockCompletionProvider;

    #[test]
    fn completion_is_not_persisted_until_accept() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path().join("workspace")).unwrap();
        let mut writer = WriterSession::open(
            workspace.clone(),
            Arc::new(MockCompletionProvider::new(" predicted")),
        )
        .unwrap();
        writer.create_note("Test").unwrap();
        writer.replace_body("Human text".to_string()).unwrap();
        writer.save().unwrap();
        let cursor = writer.current().unwrap().body.len();
        writer.request_completion(cursor).unwrap().unwrap();

        let disk = workspace.open_note(writer.current().unwrap().path.clone()).unwrap();
        assert_eq!(disk.body, "Human text");

        writer.accept_completion(cursor).unwrap();
        let disk = workspace.open_note(writer.current().unwrap().path.clone()).unwrap();
        assert_eq!(disk.body, "Human text predicted");
    }

    #[test]
    fn rejection_never_mutates_document() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path().join("workspace")).unwrap();
        let mut writer = WriterSession::open(
            workspace.clone(),
            Arc::new(MockCompletionProvider::new(" predicted")),
        )
        .unwrap();
        writer.create_note("Test").unwrap();
        writer.replace_body("Human text".to_string()).unwrap();
        writer.save().unwrap();
        writer.request_completion(10).unwrap().unwrap();
        assert!(writer.reject_completion().unwrap());
        assert_eq!(writer.current().unwrap().body, "Human text");
    }
}

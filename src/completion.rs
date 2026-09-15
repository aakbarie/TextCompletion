use crate::note::NoteId;
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub id: Uuid,
    pub note_id: NoteId,
    pub text_before_cursor: String,
    pub text_after_cursor: String,
    pub document_type: Option<String>,
    pub optional_context: Option<String>,
}

impl CompletionRequest {
    pub fn new(note_id: NoteId, before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            note_id,
            text_before_cursor: before.into(),
            text_after_cursor: after.into(),
            document_type: None,
            optional_context: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCandidate {
    pub id: Uuid,
    pub request_id: Uuid,
    pub text: String,
    pub provider: String,
    pub model: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionCapabilities {
    pub inline_completion: bool,
    pub rewrite: bool,
    pub summarize: bool,
}

impl CompletionCapabilities {
    pub const INLINE_ONLY: Self = Self {
        inline_completion: true,
        rewrite: false,
        summarize: false,
    };
}

pub trait CompletionProvider: Send + Sync + 'static {
    fn complete(&self, request: &CompletionRequest, cancel: &CancellationToken) -> Result<CompletionCandidate>;
    fn capabilities(&self) -> CompletionCapabilities;
    fn name(&self) -> &'static str;
}

/// Lightweight cancellation primitive owned by the editor. Typing, moving the
/// caret or switching notes cancels the visible proposal without allowing the
/// provider to mutate the document.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct MockCompletionProvider {
    completion: String,
}

impl MockCompletionProvider {
    pub fn new(completion: impl Into<String>) -> Self {
        Self { completion: completion.into() }
    }
}

impl Default for MockCompletionProvider {
    fn default() -> Self {
        Self::new(" This is a deterministic Scriblet completion.")
    }
}

impl CompletionProvider for MockCompletionProvider {
    fn complete(&self, request: &CompletionRequest, cancel: &CancellationToken) -> Result<CompletionCandidate> {
        let text = if cancel.is_cancelled() { String::new() } else { self.completion.clone() };
        Ok(CompletionCandidate {
            id: Uuid::new_v4(),
            request_id: request.id,
            text,
            provider: self.name().to_string(),
            model: "deterministic-v1".to_string(),
            created_at: Utc::now().to_rfc3339(),
        })
    }

    fn capabilities(&self) -> CompletionCapabilities {
        CompletionCapabilities::INLINE_ONLY
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

/// Owns exactly one visible proposal. Replacing or clearing it cancels any
/// in-flight request so stale model output cannot be accepted into a new caret
/// position or note.
#[derive(Debug, Default)]
pub struct CompletionSession {
    active_request: Option<Uuid>,
    token: Option<CancellationToken>,
    visible: Option<CompletionCandidate>,
}

impl CompletionSession {
    pub fn begin(&mut self, request: &CompletionRequest) -> CancellationToken {
        self.cancel();
        let token = CancellationToken::default();
        self.active_request = Some(request.id);
        self.token = Some(token.clone());
        token
    }

    pub fn show(&mut self, candidate: CompletionCandidate) -> bool {
        if self.active_request == Some(candidate.request_id)
            && self.token.as_ref().is_some_and(|token| !token.is_cancelled())
        {
            self.visible = Some(candidate);
            true
        } else {
            false
        }
    }

    pub fn visible(&self) -> Option<&CompletionCandidate> {
        self.visible.as_ref()
    }

    pub fn accept(&mut self) -> Option<CompletionCandidate> {
        let accepted = self.visible.take();
        self.active_request = None;
        self.token = None;
        accepted
    }

    pub fn reject(&mut self) -> Option<CompletionCandidate> {
        let rejected = self.visible.take();
        self.cancel();
        rejected
    }

    pub fn cancel(&mut self) {
        if let Some(token) = &self.token {
            token.cancel();
        }
        self.active_request = None;
        self.token = None;
        self.visible = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_candidate_cannot_replace_new_request() {
        let note = NoteId::new();
        let first = CompletionRequest::new(note.clone(), "one", "");
        let second = CompletionRequest::new(note, "two", "");
        let provider = MockCompletionProvider::new(" done");
        let mut session = CompletionSession::default();
        let first_token = session.begin(&first);
        let first_candidate = provider.complete(&first, &first_token).unwrap();
        session.begin(&second);
        assert!(!session.show(first_candidate));
    }

    #[test]
    fn proposal_only_becomes_text_when_explicitly_accepted() {
        let request = CompletionRequest::new(NoteId::new(), "The member", "");
        let provider = MockCompletionProvider::new(" meets criteria.");
        let mut session = CompletionSession::default();
        let token = session.begin(&request);
        let candidate = provider.complete(&request, &token).unwrap();
        assert!(session.show(candidate));
        assert_eq!(session.visible().unwrap().text, " meets criteria.");
        assert_eq!(session.accept().unwrap().text, " meets criteria.");
        assert!(session.visible().is_none());
    }
}
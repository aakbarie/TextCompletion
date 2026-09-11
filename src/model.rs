use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnippetScope {
    Personal,
    Shared,
    Enterprise,
}

impl SnippetScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Shared => "shared",
            Self::Enterprise => "enterprise",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "shared" => Self::Shared,
            "enterprise" => Self::Enterprise,
            _ => Self::Personal,
        }
    }
}

/// How a binding is invoked. Only text triggers are implemented today; the
/// `kind` column stays in both schemas so other kinds can be added later
/// without a migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BindingKind {
    Text,
}

impl BindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
        }
    }

    pub fn parse(_value: &str) -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub id: Uuid,
    pub snippet_id: Uuid,
    pub kind: BindingKind,
    pub value: String,
    pub enabled: bool,
}

impl Binding {
    pub fn text(snippet_id: Uuid, value: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            snippet_id,
            kind: BindingKind::Text,
            value: value.into(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snippet {
    pub id: Uuid,
    pub title: String,
    pub category: String,
    /// Compatibility field for pre-v0.3 databases. New code stores invocation in `bindings`.
    pub trigger: String,
    pub replacement: String,
    pub scope: SnippetScope,
    pub enabled: bool,
    pub favorite: bool,
    pub version: i64,
    pub updated_at: i64,
}

impl Snippet {
    pub fn personal(trigger: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: String::new(),
            category: String::new(),
            trigger: trigger.into(),
            replacement: replacement.into(),
            scope: SnippetScope::Personal,
            enabled: true,
            favorite: false,
            version: 1,
            updated_at: now_epoch_seconds(),
        }
    }

    pub fn is_enterprise(&self) -> bool {
        self.scope == SnippetScope::Enterprise
    }
}

pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

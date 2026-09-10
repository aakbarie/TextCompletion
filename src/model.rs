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

    pub fn from_str(value: &str) -> Self {
        match value {
            "shared" => Self::Shared,
            "enterprise" => Self::Enterprise,
            _ => Self::Personal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: Uuid,
    pub trigger: String,
    pub replacement: String,
    pub scope: SnippetScope,
    pub enabled: bool,
    pub version: i64,
    pub updated_at: i64,
}

impl Snippet {
    pub fn personal(trigger: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            trigger: trigger.into(),
            replacement: replacement.into(),
            scope: SnippetScope::Personal,
            enabled: true,
            version: 1,
            updated_at: now_epoch_seconds(),
        }
    }
}

pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

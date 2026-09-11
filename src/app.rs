//! Snippet service: the editing rules the UI enforces, kept out of the window
//! code so they can be tested without a display.

use crate::expansion::SharedSnippetIndex;
use crate::model::{now_epoch_seconds, Binding, Snippet, SnippetScope};
use crate::storage::SnippetRepository;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;
use uuid::Uuid;

pub const DEFAULT_CATEGORY: &str = "Personal";
pub const DEFAULT_TITLE: &str = "Untitled snippet";
/// Categories always offered in the editor, even before any snippet uses them.
pub const BUILTIN_CATEGORIES: &[&str] = &["Clinical", "Administrative", "Signatures", "Personal"];

#[derive(Debug, Error)]
pub enum SnippetError {
    #[error("Phrase cannot be empty")]
    EmptyPhrase,
    #[error("Enterprise snippets are read-only")]
    EnterpriseReadOnly,
    #[error("Binding {0} is already in use")]
    BindingInUse(String),
    #[error("Snippet no longer exists")]
    NotFound,
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Default)]
pub struct SaveRequest {
    /// `None` creates a new personal snippet.
    pub id: Option<Uuid>,
    pub title: String,
    pub category: String,
    pub binding: String,
    pub replacement: String,
    pub enabled: bool,
    pub favorite: bool,
}

/// Validates and persists a snippet plus its optional text binding in one
/// transaction, then returns the stored snippet.
pub fn save_snippet<R: SnippetRepository + ?Sized>(
    repository: &R,
    request: SaveRequest,
) -> Result<Snippet, SnippetError> {
    if request.replacement.trim().is_empty() {
        return Err(SnippetError::EmptyPhrase);
    }

    let binding_value = request.binding.trim().to_string();
    let mut snippet = match request.id {
        Some(id) => {
            let existing = repository.get(id)?.ok_or(SnippetError::NotFound)?;
            if existing.is_enterprise() {
                return Err(SnippetError::EnterpriseReadOnly);
            }
            existing
        }
        None => Snippet::personal("", ""),
    };

    if !binding_value.is_empty()
        && repository.binding_collision(&binding_value, Some(snippet.id))?
    {
        return Err(SnippetError::BindingInUse(binding_value));
    }

    snippet.title = non_empty(request.title.trim(), DEFAULT_TITLE);
    snippet.category = non_empty(request.category.trim(), DEFAULT_CATEGORY);
    snippet.trigger.clear();
    snippet.replacement = request.replacement;
    snippet.enabled = request.enabled;
    snippet.favorite = request.favorite;
    snippet.updated_at = now_epoch_seconds();
    if request.id.is_some() {
        snippet.version += 1;
    }

    repository.transaction(&mut || {
        repository.upsert(&snippet)?;
        repository.delete_bindings_for(snippet.id)?;
        if !binding_value.is_empty() {
            repository.upsert_binding(&Binding::text(snippet.id, binding_value.clone()))?;
        }
        Ok(())
    })?;

    Ok(snippet)
}

pub fn delete_snippet<R: SnippetRepository + ?Sized>(
    repository: &R,
    id: Uuid,
) -> Result<(), SnippetError> {
    let existing = repository.get(id)?.ok_or(SnippetError::NotFound)?;
    if existing.is_enterprise() {
        return Err(SnippetError::EnterpriseReadOnly);
    }
    repository.delete(id)?;
    Ok(())
}

/// Copies a snippet (personal or enterprise) into a new unbound personal snippet.
pub fn duplicate_snippet<R: SnippetRepository + ?Sized>(
    repository: &R,
    id: Uuid,
) -> Result<Snippet, SnippetError> {
    let source = repository.get(id)?.ok_or(SnippetError::NotFound)?;
    let mut duplicate = Snippet::personal("", source.replacement);
    duplicate.title = format!("{} copy", non_empty(source.title.trim(), DEFAULT_TITLE));
    duplicate.category = source.category;
    duplicate.enabled = source.enabled;
    duplicate.favorite = false;
    duplicate.scope = SnippetScope::Personal;
    repository.upsert(&duplicate)?;
    Ok(duplicate)
}

/// Rebuilds the in-memory trigger index from enabled snippets and bindings.
pub fn rebuild_index<R: SnippetRepository + ?Sized>(
    repository: &R,
    index: &SharedSnippetIndex,
) -> Result<()> {
    let snippets = repository.list()?;
    let bindings = repository.list_bindings()?;
    let by_id: HashMap<Uuid, Snippet> = snippets.into_iter().map(|s| (s.id, s)).collect();

    let mut fresh = HashMap::new();
    for binding in bindings {
        if !binding.enabled || binding.value.trim().is_empty() {
            continue;
        }
        if let Some(snippet) = by_id.get(&binding.snippet_id) {
            if snippet.enabled {
                fresh.insert(binding.value, snippet.replacement.clone());
            }
        }
    }

    *index.write() = fresh;
    Ok(())
}

/// The first enabled text binding for every snippet, keyed by snippet id.
pub fn primary_bindings<R: SnippetRepository + ?Sized>(
    repository: &R,
) -> Result<HashMap<Uuid, String>> {
    let mut map = HashMap::new();
    for binding in repository.list_bindings()? {
        if binding.enabled {
            map.entry(binding.snippet_id).or_insert(binding.value);
        }
    }
    Ok(map)
}

/// Built-in categories merged with every category in use, deduplicated
/// case-insensitively and sorted. A built-in spelling wins over a library one.
pub fn categories<R: SnippetRepository + ?Sized>(repository: &R) -> Result<Vec<String>> {
    let mut by_key: BTreeMap<String, String> = BTreeMap::new();
    for category in BUILTIN_CATEGORIES
        .iter()
        .map(|c| c.to_string())
        .chain(repository.list_categories()?)
    {
        by_key.entry(category.to_lowercase()).or_insert(category);
    }
    Ok(by_key.into_values().collect())
}

/// Library filter selected in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryFilter {
    All,
    Favorites,
    Personal,
    Enterprise,
    Category(String),
}

impl LibraryFilter {
    pub const ALL: &'static str = "all";
    pub const FAVORITES: &'static str = "favorites";
    pub const PERSONAL: &'static str = "personal";
    pub const ENTERPRISE: &'static str = "enterprise";
    const CATEGORY_PREFIX: &'static str = "category:";

    pub fn parse(value: &str) -> Self {
        match value {
            "" | Self::ALL => Self::All,
            Self::FAVORITES => Self::Favorites,
            Self::PERSONAL => Self::Personal,
            Self::ENTERPRISE => Self::Enterprise,
            other => match other.strip_prefix(Self::CATEGORY_PREFIX) {
                Some(category) => Self::Category(category.to_string()),
                None => Self::All,
            },
        }
    }

    pub fn key_for_category(category: &str) -> String {
        format!("{}{category}", Self::CATEGORY_PREFIX)
    }

    pub fn matches(&self, snippet: &Snippet) -> bool {
        match self {
            Self::All => true,
            Self::Favorites => snippet.favorite,
            Self::Personal => !snippet.is_enterprise(),
            Self::Enterprise => snippet.is_enterprise(),
            Self::Category(category) => snippet.category.eq_ignore_ascii_case(category),
        }
    }
}

fn non_empty(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteSnippetRepository;

    fn request(replacement: &str, binding: &str) -> SaveRequest {
        SaveRequest {
            id: None,
            title: String::new(),
            category: String::new(),
            binding: binding.into(),
            replacement: replacement.into(),
            enabled: true,
            favorite: false,
        }
    }

    #[test]
    fn save_applies_defaults_and_binding() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let saved = save_snippet(&repo, request("Hello", " ;hi "))?;
        assert_eq!(saved.title, DEFAULT_TITLE);
        assert_eq!(saved.category, DEFAULT_CATEGORY);
        assert_eq!(saved.version, 1);
        assert_eq!(repo.find_by_trigger(";hi")?.unwrap().id, saved.id);
        Ok(())
    }

    #[test]
    fn save_rejects_empty_phrase_and_duplicate_binding() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        assert!(matches!(
            save_snippet(&repo, request("   ", ";x")),
            Err(SnippetError::EmptyPhrase)
        ));
        save_snippet(&repo, request("first", ";same"))?;
        assert!(matches!(
            save_snippet(&repo, request("second", ";same")),
            Err(SnippetError::BindingInUse(value)) if value == ";same"
        ));
        Ok(())
    }

    #[test]
    fn editing_bumps_version_and_replaces_binding() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let saved = save_snippet(&repo, request("first", ";one"))?;
        let mut edit = request("second", ";two");
        edit.id = Some(saved.id);
        edit.title = "Renamed".into();
        let edited = save_snippet(&repo, edit)?;
        assert_eq!(edited.id, saved.id);
        assert_eq!(edited.version, 2);
        assert_eq!(edited.title, "Renamed");
        assert!(repo.find_by_trigger(";one")?.is_none());
        assert_eq!(repo.find_by_trigger(";two")?.unwrap().replacement, "second");
        Ok(())
    }

    #[test]
    fn enterprise_snippets_cannot_be_saved_or_deleted_but_can_be_duplicated() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let mut enterprise = Snippet::personal("", "Governed phrase");
        enterprise.scope = SnippetScope::Enterprise;
        enterprise.title = "Governed".into();
        repo.upsert(&enterprise)?;

        let mut edit = request("changed", "");
        edit.id = Some(enterprise.id);
        assert!(matches!(
            save_snippet(&repo, edit),
            Err(SnippetError::EnterpriseReadOnly)
        ));
        assert!(matches!(
            delete_snippet(&repo, enterprise.id),
            Err(SnippetError::EnterpriseReadOnly)
        ));

        let copy = duplicate_snippet(&repo, enterprise.id)?;
        assert_eq!(copy.scope, SnippetScope::Personal);
        assert_eq!(copy.title, "Governed copy");
        assert!(repo.bindings_for(copy.id)?.is_empty());
        Ok(())
    }

    #[test]
    fn index_only_contains_enabled_bound_snippets() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        save_snippet(&repo, request("on", ";on"))?;
        let mut off = request("off", ";off");
        off.enabled = false;
        save_snippet(&repo, off)?;
        save_snippet(&repo, request("unbound", ""))?;

        let index = SharedSnippetIndex::default();
        rebuild_index(&repo, &index)?;
        let guard = index.read();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard.get(";on").map(String::as_str), Some("on"));
        Ok(())
    }

    #[test]
    fn categories_merge_builtins_with_library() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let mut req = request("x", "");
        req.category = "Medical Director".into();
        save_snippet(&repo, req)?;
        let mut dup = request("y", "");
        dup.category = "clinical".into();
        save_snippet(&repo, dup)?;

        let list = categories(&repo)?;
        assert_eq!(
            list,
            vec![
                "Administrative",
                "Clinical",
                "Medical Director",
                "Personal",
                "Signatures"
            ]
        );
        Ok(())
    }

    #[test]
    fn filters_parse_and_match() {
        let mut snippet = Snippet::personal("", "x");
        snippet.category = "Clinical".into();
        snippet.favorite = true;
        assert!(LibraryFilter::parse("").matches(&snippet));
        assert!(LibraryFilter::parse("favorites").matches(&snippet));
        assert!(LibraryFilter::parse("personal").matches(&snippet));
        assert!(!LibraryFilter::parse("enterprise").matches(&snippet));
        assert!(
            LibraryFilter::parse(&LibraryFilter::key_for_category("clinical")).matches(&snippet)
        );
        assert!(!LibraryFilter::parse("category:Signatures").matches(&snippet));
        assert_eq!(LibraryFilter::parse("garbage"), LibraryFilter::All);
    }
}

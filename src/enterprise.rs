//! Enterprise library synchronization.
//!
//! An [`EnterpriseSource`] delivers centrally governed snippets. They are
//! cached in the local SQLite database with `SnippetScope::Enterprise` so
//! expansion keeps working offline. SQL Server is never queried while typing.

use crate::model::{Binding, Snippet, SnippetScope};
use crate::storage::SnippetRepository;
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use uuid::Uuid;

#[cfg(target_os = "windows")]
use crate::model::BindingKind;
#[cfg(target_os = "windows")]
use anyhow::Context;

/// Default ODBC driver. Driver 18 is the current Microsoft driver and supports
/// modern TLS; the legacy "SQL Server" driver that ships with Windows does not.
pub const DEFAULT_ODBC_DRIVER: &str = "ODBC Driver 18 for SQL Server";
pub const DEFAULT_PORT: u16 = 1433;
pub const DEFAULT_LOGIN_TIMEOUT_SECS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseConfig {
    pub server: String,
    pub database: String,
    pub driver: String,
    pub port: u16,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
    pub login_timeout_secs: u32,
}

impl EnterpriseConfig {
    /// Reads configuration from `SCRIBLET_*` environment variables.
    ///
    /// Sync is enabled only when both `SCRIBLET_SQL_SERVER` and
    /// `SCRIBLET_SQL_DATABASE` are set; there are no built-in server defaults.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let enabled = lookup_bool(&lookup, "SCRIBLET_ENTERPRISE_ENABLED", true);
        if !enabled {
            return None;
        }

        let server = lookup("SCRIBLET_SQL_SERVER")?.trim().to_string();
        let database = lookup("SCRIBLET_SQL_DATABASE")?.trim().to_string();
        if server.is_empty() || database.is_empty() {
            return None;
        }

        Some(Self {
            server,
            database,
            driver: lookup("SCRIBLET_ODBC_DRIVER")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_ODBC_DRIVER.to_string()),
            port: lookup("SCRIBLET_SQL_PORT")
                .and_then(|value| value.trim().parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
            encrypt: lookup_bool(&lookup, "SCRIBLET_SQL_ENCRYPT", true),
            trust_server_certificate: lookup_bool(
                &lookup,
                "SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE",
                false,
            ),
            login_timeout_secs: lookup("SCRIBLET_SQL_LOGIN_TIMEOUT_SECONDS")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(DEFAULT_LOGIN_TIMEOUT_SECS),
        })
    }

    pub fn connection_string(&self) -> String {
        format!(
            "Driver={{{}}};Server={},{};Database={};Trusted_Connection=Yes;Encrypt={};TrustServerCertificate={};",
            self.driver,
            self.server,
            self.port,
            self.database,
            yes_no(self.encrypt),
            yes_no(self.trust_server_certificate)
        )
    }
}

#[derive(Debug, Clone)]
pub struct EnterpriseRecord {
    pub snippet: Snippet,
    pub binding: Option<Binding>,
}

pub trait EnterpriseSource {
    fn fetch(&self) -> Result<Vec<EnterpriseRecord>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport {
    pub snippets_upserted: usize,
    pub bindings_upserted: usize,
    pub snippets_removed: usize,
    /// Enterprise triggers that were skipped because a personal snippet
    /// already uses the same text. The snippet itself is still cached.
    pub skipped_bindings: Vec<String>,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "Enterprise synced · {} snippets · {} bindings",
            self.snippets_upserted, self.bindings_upserted
        );
        if !self.skipped_bindings.is_empty() {
            text.push_str(&format!(
                " · {} skipped (in use: {})",
                self.skipped_bindings.len(),
                self.skipped_bindings.join(", ")
            ));
        }
        text
    }
}

/// Replaces the cached enterprise library with `source`'s content inside one
/// transaction. Personal snippets are never modified. A binding that collides
/// with a personal binding is skipped and reported instead of failing the sync.
pub fn sync_enterprise<S, R>(source: &S, repository: &R) -> Result<SyncReport>
where
    S: EnterpriseSource,
    R: SnippetRepository + ?Sized,
{
    let records = source.fetch()?;
    if let Some(bad) = records
        .iter()
        .find(|record| record.snippet.scope != SnippetScope::Enterprise)
    {
        return Err(anyhow!(
            "enterprise source returned a non-enterprise snippet: {}",
            bad.snippet.id
        ));
    }
    let incoming_ids: HashSet<Uuid> = records.iter().map(|record| record.snippet.id).collect();

    let mut report = SyncReport::default();
    repository.transaction(&mut || {
        report = SyncReport::default();

        for record in &records {
            repository.upsert(&record.snippet)?;
            report.snippets_upserted += 1;

            repository.delete_bindings_for(record.snippet.id)?;
            if let Some(binding) = &record.binding {
                if repository.binding_collision(&binding.value, Some(record.snippet.id))? {
                    log::warn!(
                        "enterprise binding {} skipped: already used by another snippet",
                        binding.value
                    );
                    report.skipped_bindings.push(binding.value.clone());
                    continue;
                }
                repository.upsert_binding(binding)?;
                report.bindings_upserted += 1;
            }
        }

        for snippet in repository.list()? {
            if snippet.is_enterprise() && !incoming_ids.contains(&snippet.id) {
                repository.delete(snippet.id)?;
                report.snippets_removed += 1;
            }
        }
        Ok(())
    })?;

    Ok(report)
}

#[cfg(target_os = "windows")]
pub struct SqlServerEnterpriseSource {
    config: EnterpriseConfig,
}

#[cfg(target_os = "windows")]
impl SqlServerEnterpriseSource {
    pub fn new(config: EnterpriseConfig) -> Self {
        Self { config }
    }
}

#[cfg(target_os = "windows")]
impl EnterpriseSource for SqlServerEnterpriseSource {
    fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
        use odbc_api::{ConnectionOptions, Cursor, Environment};

        let environment = Environment::new().context("failed to initialize ODBC")?;
        let connection_string = self.config.connection_string();
        let options = ConnectionOptions {
            login_timeout_sec: Some(self.config.login_timeout_secs),
            ..ConnectionOptions::default()
        };
        let connection = environment
            .connect_with_connection_string(&connection_string, options)
            .context("failed to connect to SQL Server using Windows Integrated Authentication")?;

        let sql = r#"
            SELECT
                CONVERT(varchar(36), s.id),
                s.title,
                s.category,
                s.replacement,
                CONVERT(varchar(5), s.enabled),
                CONVERT(varchar(20), s.version),
                CONVERT(varchar(20), s.updated_at),
                CONVERT(varchar(36), b.id),
                b.kind,
                b.value,
                CONVERT(varchar(5), b.enabled)
            FROM dbo.ScribletSnippets s
            OUTER APPLY (
                SELECT TOP (1) id, kind, value, enabled
                FROM dbo.ScribletBindings b
                WHERE b.snippet_id = s.id AND b.enabled = 1
                ORDER BY id
            ) b
            WHERE s.enabled = 1
            ORDER BY s.category, s.title;
        "#;

        let mut cursor = connection
            .execute(sql, (), None)?
            .ok_or_else(|| anyhow!("enterprise SQL query returned no result set"))?;

        let mut records = Vec::new();
        while let Some(mut row) = cursor.next_row()? {
            let id = parse_uuid(text_col(&mut row, 1)?, "snippet id")?;
            let title = text_col(&mut row, 2)?.unwrap_or_default();
            let category = text_col(&mut row, 3)?.unwrap_or_default();
            let replacement = text_col(&mut row, 4)?.unwrap_or_default();
            let enabled = parse_bool(text_col(&mut row, 5)?.as_deref()).unwrap_or(true);
            let version = parse_i64(text_col(&mut row, 6)?.as_deref()).unwrap_or(1);
            let updated_at = parse_i64(text_col(&mut row, 7)?.as_deref()).unwrap_or(0);

            let snippet = Snippet {
                id,
                title,
                category,
                trigger: String::new(),
                replacement,
                scope: SnippetScope::Enterprise,
                enabled,
                favorite: false,
                version,
                updated_at,
            };

            let binding = match text_col(&mut row, 8)? {
                Some(binding_id) if !binding_id.trim().is_empty() => {
                    let kind =
                        BindingKind::parse(text_col(&mut row, 9)?.as_deref().unwrap_or("text"));
                    let value = text_col(&mut row, 10)?.unwrap_or_default();
                    let enabled = parse_bool(text_col(&mut row, 11)?.as_deref()).unwrap_or(true);
                    Some(Binding {
                        id: parse_uuid(Some(binding_id), "binding id")?,
                        snippet_id: id,
                        kind,
                        value,
                        enabled,
                    })
                }
                _ => None,
            };

            records.push(EnterpriseRecord { snippet, binding });
        }
        Ok(records)
    }
}

#[cfg(target_os = "windows")]
fn text_col(row: &mut odbc_api::CursorRow<'_>, index: u16) -> Result<Option<String>> {
    let mut buf = Vec::new();
    if !row.get_text(index, &mut buf)? {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(buf).context("ODBC returned non-UTF8 text")?,
    ))
}

#[cfg(target_os = "windows")]
fn parse_uuid(value: Option<String>, field: &str) -> Result<Uuid> {
    let value = value.ok_or_else(|| anyhow!("missing {field}"))?;
    Uuid::parse_str(value.trim()).with_context(|| format!("invalid {field}: {value}"))
}

#[cfg(target_os = "windows")]
fn parse_i64(value: Option<&str>) -> Option<i64> {
    value.and_then(|v| v.trim().parse().ok())
}

fn parse_bool(value: Option<&str>) -> Option<bool> {
    value
        .map(str::trim)
        .and_then(|v| match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        })
}

fn lookup_bool(lookup: &impl Fn(&str) -> Option<String>, name: &str, default: bool) -> bool {
    lookup(name)
        .and_then(|value| parse_bool(Some(&value)))
        .unwrap_or(default)
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BindingKind;
    use crate::storage::SqliteSnippetRepository;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeSource(Mutex<Vec<EnterpriseRecord>>);
    impl EnterpriseSource for FakeSource {
        fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    struct FailingSource;
    impl EnterpriseSource for FailingSource {
        fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
            Err(anyhow!("network unreachable"))
        }
    }

    fn record(title: &str, trigger: &str) -> EnterpriseRecord {
        let snippet_id = Uuid::new_v4();
        EnterpriseRecord {
            snippet: Snippet {
                id: snippet_id,
                title: title.into(),
                category: "Clinical".into(),
                trigger: String::new(),
                replacement: "Approved enterprise phrase".into(),
                scope: SnippetScope::Enterprise,
                enabled: true,
                favorite: false,
                version: 3,
                updated_at: 100,
            },
            binding: Some(Binding {
                id: Uuid::new_v4(),
                snippet_id,
                kind: BindingKind::Text,
                value: trigger.into(),
                enabled: true,
            }),
        }
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn config_requires_server_and_database() {
        assert!(EnterpriseConfig::from_lookup(env(&[])).is_none());
        assert!(EnterpriseConfig::from_lookup(env(&[("SCRIBLET_SQL_SERVER", "db01")])).is_none());
        assert!(EnterpriseConfig::from_lookup(env(&[
            ("SCRIBLET_SQL_SERVER", "db01"),
            ("SCRIBLET_SQL_DATABASE", "Phrases"),
            ("SCRIBLET_ENTERPRISE_ENABLED", "false"),
        ]))
        .is_none());
    }

    #[test]
    fn connection_string_uses_integrated_auth_encryption_and_modern_driver() {
        let config = EnterpriseConfig::from_lookup(env(&[
            ("SCRIBLET_SQL_SERVER", "db01"),
            ("SCRIBLET_SQL_DATABASE", "Phrases"),
        ]))
        .unwrap();
        assert_eq!(config.login_timeout_secs, DEFAULT_LOGIN_TIMEOUT_SECS);
        let value = config.connection_string();
        assert!(value.contains("Driver={ODBC Driver 18 for SQL Server}"));
        assert!(value.contains("Server=db01,1433"));
        assert!(value.contains("Database=Phrases"));
        assert!(value.contains("Trusted_Connection=Yes"));
        assert!(value.contains("Encrypt=Yes"));
        assert!(value.contains("TrustServerCertificate=No"));
        assert!(!value.contains("PWD="));
    }

    #[test]
    fn config_honours_overrides() {
        let config = EnterpriseConfig::from_lookup(env(&[
            ("SCRIBLET_SQL_SERVER", "db01"),
            ("SCRIBLET_SQL_DATABASE", "Phrases"),
            ("SCRIBLET_SQL_PORT", "1500"),
            ("SCRIBLET_ODBC_DRIVER", "ODBC Driver 17 for SQL Server"),
            ("SCRIBLET_SQL_ENCRYPT", "no"),
            ("SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE", "yes"),
            ("SCRIBLET_SQL_LOGIN_TIMEOUT_SECONDS", "12"),
        ]))
        .unwrap();
        assert_eq!(config.port, 1500);
        assert_eq!(config.driver, "ODBC Driver 17 for SQL Server");
        assert!(!config.encrypt);
        assert!(config.trust_server_certificate);
        assert_eq!(config.login_timeout_secs, 12);
    }

    #[test]
    fn sync_preserves_personal_and_replaces_enterprise_cache() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let mut personal = Snippet::personal("", "personal");
        personal.title = "My personal phrase".into();
        personal.category = "Personal".into();
        repo.upsert(&personal)?;
        repo.upsert_binding(&Binding::text(personal.id, ";mine"))?;

        let first = record("Approved signature", ";asig");
        let source = FakeSource(Mutex::new(vec![first.clone()]));
        let report = sync_enterprise(&source, &repo)?;
        assert_eq!(report.snippets_upserted, 1);
        assert!(repo.find_by_trigger(";asig")?.is_some());
        assert!(repo.find_by_trigger(";mine")?.is_some());

        *source.0.lock().unwrap() = vec![];
        let report = sync_enterprise(&source, &repo)?;
        assert_eq!(report.snippets_removed, 1);
        assert!(repo.find_by_trigger(";asig")?.is_none());
        assert!(repo.find_by_trigger(";mine")?.is_some());
        Ok(())
    }

    #[test]
    fn colliding_enterprise_binding_is_skipped_not_fatal() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let personal = Snippet::personal("", "personal");
        repo.upsert(&personal)?;
        repo.upsert_binding(&Binding::text(personal.id, ";sig"))?;

        let clash = record("Enterprise signature", ";sig");
        let fine = record("Enterprise approval", ";approve");
        let source = FakeSource(Mutex::new(vec![clash.clone(), fine]));
        let report = sync_enterprise(&source, &repo)?;

        assert_eq!(report.snippets_upserted, 2);
        assert_eq!(report.bindings_upserted, 1);
        assert_eq!(report.skipped_bindings, vec![";sig".to_string()]);
        assert_eq!(repo.find_by_trigger(";sig")?.unwrap().id, personal.id);
        assert!(repo.get(clash.snippet.id)?.is_some());
        assert!(repo.find_by_trigger(";approve")?.is_some());
        assert!(report.summary().contains("skipped"));
        Ok(())
    }

    #[test]
    fn failed_fetch_leaves_cache_untouched() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let source = FakeSource(Mutex::new(vec![record("Cached", ";cached")]));
        sync_enterprise(&source, &repo)?;

        assert!(sync_enterprise(&FailingSource, &repo).is_err());
        assert!(repo.find_by_trigger(";cached")?.is_some());
        Ok(())
    }

    #[test]
    fn non_enterprise_record_is_rejected_before_writing() -> Result<()> {
        let repo = SqliteSnippetRepository::open_in_memory()?;
        let mut bad = record("Sneaky", ";sneaky");
        bad.snippet.scope = SnippetScope::Personal;
        let source = FakeSource(Mutex::new(vec![bad]));
        assert!(sync_enterprise(&source, &repo).is_err());
        assert!(repo.list()?.is_empty());
        Ok(())
    }
}

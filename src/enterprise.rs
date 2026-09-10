use crate::model::{Binding, BindingKind, Snippet, SnippetScope};
use crate::storage::SnippetRepository;
use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseConfig {
    pub server: String,
    pub database: String,
    pub driver: String,
    pub port: u16,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
}

impl EnterpriseConfig {
    pub fn from_env() -> Option<Self> {
        let enabled = env_bool("SCRIBLET_ENTERPRISE_ENABLED", true);
        if !enabled {
            return None;
        }

        let server = std::env::var("SCRIBLET_SQL_SERVER")
            .unwrap_or_else(|_| "RPTPRODDB".to_string())
            .trim()
            .to_string();
        let database = std::env::var("SCRIBLET_SQL_DATABASE")
            .unwrap_or_else(|_| "Alliance_RPT".to_string())
            .trim()
            .to_string();
        if server.is_empty() || database.is_empty() {
            return None;
        }

        Some(Self {
            server,
            database,
            driver: std::env::var("SCRIBLET_ODBC_DRIVER")
                .unwrap_or_else(|_| "SQL Server".to_string()),
            port: std::env::var("SCRIBLET_SQL_PORT")
                .ok()
                .and_then(|value| value.trim().parse::<u16>().ok())
                .unwrap_or(1433),
            encrypt: env_bool("SCRIBLET_SQL_ENCRYPT", true),
            trust_server_certificate: env_bool("SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE", false),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub snippets_upserted: usize,
    pub bindings_upserted: usize,
    pub snippets_removed: usize,
}

pub fn sync_enterprise<S, R>(source: &S, repository: &R) -> Result<SyncReport>
where
    S: EnterpriseSource,
    R: SnippetRepository,
{
    let records = source.fetch()?;
    let incoming_ids: HashSet<Uuid> = records.iter().map(|record| record.snippet.id).collect();

    let mut snippets_upserted = 0;
    let mut bindings_upserted = 0;

    for record in &records {
        if record.snippet.scope != SnippetScope::Enterprise {
            return Err(anyhow!("enterprise source returned a non-enterprise snippet"));
        }
        repository.upsert(&record.snippet)?;
        snippets_upserted += 1;

        repository.delete_bindings_for(record.snippet.id)?;
        if let Some(binding) = &record.binding {
            if repository.binding_collision(&binding.value, Some(record.snippet.id))? {
                return Err(anyhow!("enterprise binding collision for {}", binding.value));
            }
            repository.upsert_binding(binding)?;
            bindings_upserted += 1;
        }
    }

    let existing = repository.list()?;
    let mut snippets_removed = 0;
    for snippet in existing {
        if snippet.scope == SnippetScope::Enterprise && !incoming_ids.contains(&snippet.id) {
            repository.delete(snippet.id)?;
            snippets_removed += 1;
        }
    }

    Ok(SyncReport {
        snippets_upserted,
        bindings_upserted,
        snippets_removed,
    })
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
        let connection = environment
            .connect_with_connection_string(&connection_string, ConnectionOptions::default())
            .context("failed to connect to Scriblet SQL Server using Windows Integrated Authentication")?;

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
                    let kind = BindingKind::from_str(text_col(&mut row, 9)?.as_deref().unwrap_or("text"));
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
    Ok(Some(String::from_utf8(buf).context("ODBC returned non-UTF8 text")?))
}

fn parse_uuid(value: Option<String>, field: &str) -> Result<Uuid> {
    let value = value.ok_or_else(|| anyhow!("missing {field}"))?;
    Uuid::parse_str(value.trim()).with_context(|| format!("invalid {field}: {value}"))
}

fn parse_i64(value: Option<&str>) -> Option<i64> {
    value.and_then(|v| v.trim().parse().ok())
}

fn parse_bool(value: Option<&str>) -> Option<bool> {
    value.map(str::trim).and_then(|v| match v.to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .and_then(|value| parse_bool(Some(&value)))
        .unwrap_or(default)
}

fn yes_no(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteSnippetRepository;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct FakeSource(Mutex<Vec<EnterpriseRecord>>);
    impl EnterpriseSource for FakeSource {
        fn fetch(&self) -> Result<Vec<EnterpriseRecord>> {
            Ok(self.0.lock().unwrap().clone())
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

    #[test]
    fn connection_string_uses_alliance_defaults_integrated_auth_and_encryption() {
        let config = EnterpriseConfig {
            server: "RPTPRODDB".into(),
            database: "Alliance_RPT".into(),
            driver: "SQL Server".into(),
            port: 1433,
            encrypt: true,
            trust_server_certificate: false,
        };
        let value = config.connection_string();
        assert!(value.contains("Driver={SQL Server}"));
        assert!(value.contains("Server=RPTPRODDB,1433"));
        assert!(value.contains("Database=Alliance_RPT"));
        assert!(value.contains("Trusted_Connection=Yes"));
        assert!(value.contains("Encrypt=Yes"));
        assert!(value.contains("TrustServerCertificate=No"));
        assert!(!value.contains("PWD="));
    }

    #[test]
    fn sync_preserves_personal_and_replaces_enterprise_cache() -> Result<()> {
        let dir = tempdir()?;
        let repo = SqliteSnippetRepository::open(dir.path().join("scriblet.db"))?;
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
}

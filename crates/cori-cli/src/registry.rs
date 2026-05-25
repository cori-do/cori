//! SQLite-backed registry of compiled workflows and runs.
//!
//! The registry currently needs the `workflows` table; the `runs` table is
//! created eagerly because the run loop already writes trace rows into it
//! and we want to avoid a later schema migration.
//!
//! The on-disk file lives at [`crate::paths::registry_db`] and is opened
//! lazily — every command that touches the registry calls [`open`] which
//! ensures the directory tree and schema exist.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cori_protocol::CompiledWorkflow;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::paths;

/// A handle to the registry. Owning the [`Connection`] keeps the file
/// locked for the duration of the operation; commands are short-lived so we
/// just open and drop.
pub struct Registry {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: u32,
    pub source_path: String,
    pub registered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowDetail {
    pub id: String,
    pub source_path: String,
    pub version: u32,
    pub registered_at: DateTime<Utc>,
    pub manifest_yaml: String,
    pub compiled: CompiledWorkflow,
}

/// Outcome of a `register` call. Mirrors what the CLI prints.
#[derive(Debug)]
pub enum RegisterOutcome {
    /// No previous row existed.
    Created { version: u32 },
    /// Row existed but with identical content — version unchanged.
    Unchanged { version: u32 },
    /// Row existed with different content — version bumped.
    Updated { version: u32 },
}

impl RegisterOutcome {
    #[allow(dead_code)]
    pub fn version(&self) -> u32 {
        match self {
            Self::Created { version } | Self::Unchanged { version } | Self::Updated { version } => {
                *version
            }
        }
    }
}

/// Open (and if needed create) the registry at the default location.
pub fn open() -> Result<Registry> {
    let path = paths::registry_db()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create state directory `{}`", parent.display()))?;
    }
    Registry::open_at(&path)
}

impl Registry {
    pub fn open_at(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("could not open registry `{}`", path.display()))?;
        // Reasonable defaults for a single-writer local registry.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let reg = Self { conn };
        reg.init_schema()?;
        Ok(reg)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS workflows (
                id              TEXT PRIMARY KEY,
                source_path     TEXT NOT NULL,
                manifest_json   TEXT NOT NULL,
                compiled_json   TEXT NOT NULL,
                content_hash    TEXT NOT NULL,
                version         INTEGER NOT NULL,
                registered_at   TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS runs (
                run_id          TEXT PRIMARY KEY,
                workflow_id     TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                ended_at        TEXT,
                status          TEXT NOT NULL,
                trace_json      TEXT NOT NULL,
                FOREIGN KEY (workflow_id) REFERENCES workflows(id)
            );
            CREATE INDEX IF NOT EXISTS runs_by_workflow
              ON runs(workflow_id, started_at DESC);",
        )?;
        Ok(())
    }

    /// Upsert a compiled workflow. Bumps `version` only when the content
    /// hash differs from what was previously stored.
    pub fn register(
        &mut self,
        source_path: &Path,
        compiled: &CompiledWorkflow,
    ) -> Result<RegisterOutcome> {
        let id = compiled.manifest.id.clone();
        let manifest_json =
            serde_json::to_string(&compiled.manifest).context("serializing manifest")?;
        let compiled_json =
            serde_json::to_string(compiled).context("serializing compiled workflow")?;
        let content_hash = hash(&compiled_json);
        let source = source_path.to_string_lossy().into_owned();
        let now = Utc::now();

        let existing: Option<(u32, String)> = self
            .conn
            .query_row(
                "SELECT version, content_hash FROM workflows WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let (new_version, outcome) = match existing {
            None => (1u32, RegisterOutcome::Created { version: 1 }),
            Some((v, h)) if h == content_hash => {
                // Source path may still legitimately move; update it but
                // keep the version.
                self.conn.execute(
                    "UPDATE workflows SET source_path = ?1 WHERE id = ?2",
                    params![source, id],
                )?;
                return Ok(RegisterOutcome::Unchanged { version: v });
            }
            Some((v, _)) => (v + 1, RegisterOutcome::Updated { version: v + 1 }),
        };

        self.conn.execute(
            "INSERT INTO workflows (id, source_path, manifest_json, compiled_json, content_hash, version, registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               source_path = excluded.source_path,
               manifest_json = excluded.manifest_json,
               compiled_json = excluded.compiled_json,
               content_hash = excluded.content_hash,
               version = excluded.version,
               registered_at = excluded.registered_at",
            params![
                id,
                source,
                manifest_json,
                compiled_json,
                content_hash,
                new_version,
                now.to_rfc3339(),
            ],
        )?;
        Ok(outcome)
    }

    /// List every registered workflow, ordered by id.
    pub fn list(&self) -> Result<Vec<WorkflowRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, manifest_json, compiled_json, version, source_path, registered_at
             FROM workflows
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let id: String = r.get(0)?;
                let manifest_json: String = r.get(1)?;
                let _compiled_json: String = r.get(2)?;
                let version: u32 = r.get(3)?;
                let source_path: String = r.get(4)?;
                let registered_at: String = r.get(5)?;
                Ok(RawRow {
                    id,
                    manifest_json,
                    version,
                    source_path,
                    registered_at,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(RawRow::into_summary).collect()
    }

    pub fn get(&self, id: &str) -> Result<Option<WorkflowDetail>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, manifest_json, compiled_json, version, source_path, registered_at
                 FROM workflows WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, manifest_json, compiled_json, version, source_path, registered_at)) = row
        else {
            return Ok(None);
        };
        let compiled: CompiledWorkflow =
            serde_json::from_str(&compiled_json).context("decoding compiled_json")?;
        let manifest_value: serde_json::Value =
            serde_json::from_str(&manifest_json).context("decoding manifest_json")?;
        let manifest_yaml =
            serde_yaml::to_string(&manifest_value).context("re-emitting manifest as YAML")?;
        let registered_at = DateTime::parse_from_rfc3339(&registered_at)
            .with_context(|| format!("invalid registered_at `{registered_at}`"))?
            .with_timezone(&Utc);
        Ok(Some(WorkflowDetail {
            id,
            source_path,
            version,
            registered_at,
            manifest_yaml,
            compiled,
        }))
    }

    /// Append (or replace) a run row. The trace JSON is opaque to the
    /// registry — the CLI's `run` command builds it and round-trips it
    /// back out via `cori runs show` in a later command update.
    pub fn record_run(
        &mut self,
        run_id: &str,
        workflow_id: &str,
        started_at: DateTime<Utc>,
        ended_at: Option<DateTime<Utc>>,
        status: &str,
        trace_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (run_id, workflow_id, started_at, ended_at, status, trace_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(run_id) DO UPDATE SET
               ended_at = excluded.ended_at,
               status = excluded.status,
               trace_json = excluded.trace_json",
            params![
                run_id,
                workflow_id,
                started_at.to_rfc3339(),
                ended_at.map(|t| t.to_rfc3339()),
                status,
                trace_json,
            ],
        )?;
        Ok(())
    }

    /// List recent runs, most recent first.
    pub fn list_runs(&self, workflow_id: Option<&str>, limit: u32) -> Result<Vec<RunRow>> {
        let (sql, has_filter) = match workflow_id {
            Some(_) => (
                "SELECT run_id, workflow_id, started_at, ended_at, status
                 FROM runs WHERE workflow_id = ?1
                 ORDER BY started_at DESC LIMIT ?2",
                true,
            ),
            None => (
                "SELECT run_id, workflow_id, started_at, ended_at, status
                 FROM runs ORDER BY started_at DESC LIMIT ?1",
                false,
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<RawRunRow> {
            Ok(RawRunRow {
                run_id: r.get(0)?,
                workflow_id: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get::<_, Option<String>>(3)?,
                status: r.get(4)?,
            })
        };
        let raw: Vec<RawRunRow> = if has_filter {
            stmt.query_map(params![workflow_id.unwrap(), limit], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![limit], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        raw.into_iter().map(RawRunRow::into_row).collect()
    }

    /// Fetch a single run's trace JSON. Returns `None` if no such run.
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunDetail>> {
        let row = self
            .conn
            .query_row(
                "SELECT run_id, workflow_id, started_at, ended_at, status, trace_json
                 FROM runs WHERE run_id = ?1",
                params![run_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((run_id, workflow_id, started_at, ended_at, status, trace_json)) = row else {
            return Ok(None);
        };
        Ok(Some(RunDetail {
            run_id,
            workflow_id,
            started_at: parse_dt(&started_at)?,
            ended_at: ended_at.as_deref().map(parse_dt).transpose()?,
            status,
            trace_json,
        }))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    pub run_id: String,
    pub workflow_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunDetail {
    pub run_id: String,
    pub workflow_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
    pub trace_json: String,
}

struct RawRunRow {
    run_id: String,
    workflow_id: String,
    started_at: String,
    ended_at: Option<String>,
    status: String,
}

impl RawRunRow {
    fn into_row(self) -> Result<RunRow> {
        Ok(RunRow {
            run_id: self.run_id,
            workflow_id: self.workflow_id,
            started_at: parse_dt(&self.started_at)?,
            ended_at: self.ended_at.as_deref().map(parse_dt).transpose()?,
            status: self.status,
        })
    }
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("invalid timestamp `{s}`"))?
        .with_timezone(&Utc))
}

struct RawRow {
    id: String,
    manifest_json: String,
    version: u32,
    source_path: String,
    registered_at: String,
}

impl RawRow {
    fn into_summary(self) -> Result<WorkflowRow> {
        let manifest: cori_protocol::Manifest =
            serde_json::from_str(&self.manifest_json).context("decoding manifest_json")?;
        let registered_at = DateTime::parse_from_rfc3339(&self.registered_at)
            .with_context(|| format!("invalid registered_at `{}`", self.registered_at))?
            .with_timezone(&Utc);
        Ok(WorkflowRow {
            id: self.id,
            name: manifest.name,
            description: manifest.description,
            version: self.version,
            source_path: self.source_path,
            registered_at,
        })
    }
}

fn hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_protocol::{CompiledStep, StepKind};
    use tempfile::tempdir;

    fn fake_workflow(id: &str) -> CompiledWorkflow {
        CompiledWorkflow {
            manifest: cori_protocol::Manifest {
                id: id.into(),
                name: format!("name-{id}"),
                description: "desc".into(),
                created: chrono::NaiveDate::from_ymd_opt(2026, 5, 25).unwrap(),
                version: 1,
                updated: None,
                parameters: vec![],
                tools_required: vec![],
                mcp_servers: vec![],
                tags: vec![],
                route_default: None,
                schedule: None,
                schedule_tz: None,
                body: String::new(),
            },
            steps: vec![CompiledStep {
                activity_id: "01_x".into(),
                index: 0,
                source_path: "steps/01_x.ts".into(),
                kind: StepKind::Code,
                name: "x".into(),
                description: "x".into(),
                route: None,
                depends_on: vec![],
                metadata: Default::default(),
            }],
            required_cli_binaries: vec![],
            required_mcp_servers: vec![],
            required_llm_providers: vec![],
        }
    }

    #[test]
    fn register_and_list() {
        let tmp = tempdir().unwrap();
        let db = tmp.path().join("r.db");
        let mut reg = Registry::open_at(&db).unwrap();
        let wf = fake_workflow("a");
        let outcome = reg.register(Path::new("/some/path"), &wf).unwrap();
        assert!(matches!(outcome, RegisterOutcome::Created { version: 1 }));
        let outcome = reg.register(Path::new("/some/path"), &wf).unwrap();
        assert!(matches!(outcome, RegisterOutcome::Unchanged { version: 1 }));

        let mut wf2 = wf.clone();
        wf2.manifest.description = "changed".into();
        let outcome = reg.register(Path::new("/some/path"), &wf2).unwrap();
        assert!(matches!(outcome, RegisterOutcome::Updated { version: 2 }));

        let list = reg.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
        assert_eq!(list[0].version, 2);

        let detail = reg.get("a").unwrap().unwrap();
        assert_eq!(detail.version, 2);
        assert!(detail.manifest_yaml.contains("changed"));
    }
}

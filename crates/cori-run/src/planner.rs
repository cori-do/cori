//! Per-step queue assignment.
//!
//! Pure function: given a [`CompiledWorkflow`], the requesting user's
//! [`WorkerIdentity`], and a [`ClusterView`] of who advertises which
//! capabilities, fill in each step's `task_queue` field.

use std::path::Path;

use anyhow::{Context, Result};
use cori_broker::capabilities::CapabilityReport;
use cori_protocol::{CompiledWorkflow, Placement, WorkerIdentity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::paths;

/// Snapshot of every capability report known to this machine.
#[derive(Debug, Clone, Default)]
pub struct ClusterView {
    pub reports: Vec<CapabilityReport>,
}

impl ClusterView {
    pub fn load() -> Result<Self> {
        let dir = paths::cluster_dir()?;
        Self::load_from(&dir)
    }

    pub fn load_from(dir: &Path) -> Result<Self> {
        let mut reports = Vec::new();
        if !dir.is_dir() {
            return Ok(Self { reports });
        }
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("reading `{}`", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<CapabilityReport>(&bytes) {
                    Ok(r) => reports.push(r),
                    Err(e) => tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping malformed cluster report"
                    ),
                },
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping unreadable cluster report"
                ),
            }
        }
        Ok(Self { reports })
    }

    pub fn add_self(&mut self, report: CapabilityReport) {
        self.reports.insert(0, report);
    }

    fn first_service_with(&self, cap_id: &str) -> Option<&CapabilityReport> {
        self.reports
            .iter()
            .find(|r| matches!(r.identity, WorkerIdentity::Service { .. }) && r.advertises(cap_id))
    }

    fn first_user_with(&self, user_id: &str, cap_id: &str) -> Option<&CapabilityReport> {
        self.reports.iter().find(|r| match &r.identity {
            WorkerIdentity::Person { user_id: u } => u == user_id && r.advertises(cap_id),
            _ => false,
        })
    }

    fn person_report(&self, user_id: &str) -> Option<&CapabilityReport> {
        self.reports.iter().find(|r| match &r.identity {
            WorkerIdentity::Person { user_id: u } => u == user_id,
            _ => false,
        })
    }
}

#[derive(Debug, Error)]
pub enum PlacementError {
    #[error(
        "step `{step}` requires the requesting user's local filesystem, but the requesting identity is a service pool `{pool}`. Service workers cannot read user-local files."
    )]
    LocalFsFromService { step: String, pool: String },

    #[error(
        "step `{step}` requires capability `{capability}`, but no worker on the cluster advertises it ready. Run `cori work` (or `cori work --shared <pool>`) on a machine that has it, or `cori login {capability}`."
    )]
    MissingCapability { step: String, capability: String },
}

/// Fill `task_queue` on every step in `compiled` according to its
/// [`Placement`] and the cluster view.
pub fn assign_queues(
    compiled: &mut CompiledWorkflow,
    requesting: &WorkerIdentity,
    cluster: &ClusterView,
) -> Result<Vec<StepAssignment>, PlacementError> {
    let mut summary: Vec<StepAssignment> = Vec::with_capacity(compiled.steps.len());

    for step in compiled.steps.iter_mut() {
        let (queue, reason) = match &step.placement {
            Placement::Anywhere => match requesting {
                WorkerIdentity::Person { user_id } => {
                    (format!("cori.user.{user_id}"), AssignReason::RequestingUser)
                }
                WorkerIdentity::Service { pool } => (
                    format!("cori.service.{pool}"),
                    AssignReason::RequestingService,
                ),
            },
            Placement::RequiresLocalFs => match requesting {
                WorkerIdentity::Person { user_id } => {
                    (format!("cori.user.{user_id}"), AssignReason::LocalFsForUser)
                }
                WorkerIdentity::Service { pool } => {
                    return Err(PlacementError::LocalFsFromService {
                        step: step.activity_id.clone(),
                        pool: pool.clone(),
                    });
                }
            },
            Placement::RequiresCapability { id } => {
                if let Some(r) = cluster.first_service_with(id) {
                    (r.task_queue.clone(), AssignReason::ServicePool)
                } else if let WorkerIdentity::Person { user_id } = requesting {
                    if let Some(r) = cluster.first_user_with(user_id, id) {
                        (r.task_queue.clone(), AssignReason::RequestingUser)
                    } else if cluster
                        .person_report(user_id)
                        .map(|_| true)
                        .unwrap_or(false)
                    {
                        return Err(PlacementError::MissingCapability {
                            step: step.activity_id.clone(),
                            capability: id.clone(),
                        });
                    } else {
                        (format!("cori.user.{user_id}"), AssignReason::RequestingUser)
                    }
                } else {
                    return Err(PlacementError::MissingCapability {
                        step: step.activity_id.clone(),
                        capability: id.clone(),
                    });
                }
            }
        };

        step.task_queue = Some(queue.clone());
        summary.push(StepAssignment {
            activity_id: step.activity_id.clone(),
            step_name: step.name.clone(),
            placement: step.placement.clone(),
            task_queue: queue,
            reason,
        });
    }

    Ok(summary)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepAssignment {
    pub activity_id: String,
    pub step_name: String,
    pub placement: Placement,
    pub task_queue: String,
    pub reason: AssignReason,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssignReason {
    RequestingUser,
    RequestingService,
    LocalFsForUser,
    ServicePool,
}

// ---------------------------------------------------------------------------
// Write / delete a worker's own report under `~/.cori/cluster/`.
// ---------------------------------------------------------------------------

pub fn publish_report(report: &CapabilityReport) -> Result<std::path::PathBuf> {
    let dir = paths::cluster_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;
    let path = dir.join(format!("{}.json", report.task_queue));
    let bytes = serde_json::to_vec_pretty(report).context("serializing capability report")?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}.json",
        std::process::id(),
        report.task_queue
    ));
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(path)
}

pub fn unpublish_report(task_queue: &str) -> Result<()> {
    let path = paths::cluster_dir()?.join(format!("{task_queue}.json"));
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing `{}`", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (mirror of cori-cli tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use cori_broker::capabilities::{Capability, CapabilityKind};
    use cori_manifest::Manifest;
    use cori_protocol::{CompiledStep, StepKind, task_queue_for};

    fn manifest() -> Manifest {
        Manifest {
            id: "t".into(),
            name: "t".into(),
            description: String::new(),
            created: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
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
        }
    }

    fn step(id: &str, kind: StepKind, placement: Placement) -> CompiledStep {
        CompiledStep {
            activity_id: id.into(),
            index: 0,
            source_path: format!("steps/{id}.ts"),
            kind,
            name: id.into(),
            description: String::new(),
            route: None,
            depends_on: vec![],
            metadata: Default::default(),
            placement,
            task_queue: None,
        }
    }

    fn person(user: &str) -> WorkerIdentity {
        WorkerIdentity::person(user).unwrap()
    }
    fn service(pool: &str) -> WorkerIdentity {
        WorkerIdentity::service(pool).unwrap()
    }

    fn report_with(
        identity: WorkerIdentity,
        caps: Vec<(&str, CapabilityKind)>,
    ) -> CapabilityReport {
        let task_queue = task_queue_for(&identity);
        CapabilityReport {
            identity,
            task_queue,
            capabilities: caps
                .into_iter()
                .map(|(id, kind)| Capability {
                    id: id.to_string(),
                    kind,
                    authed: true,
                    detail: None,
                })
                .collect(),
        }
    }

    #[test]
    fn three_step_mixed_routing() {
        let me = person("jean");
        let mut cluster = ClusterView::default();
        cluster.add_self(report_with(
            me.clone(),
            vec![
                ("local_fs", CapabilityKind::LocalFs),
                ("openai", CapabilityKind::Llm),
            ],
        ));
        cluster.reports.push(report_with(
            service("notion-pool"),
            vec![("notion", CapabilityKind::McpStatic)],
        ));

        let mut compiled = CompiledWorkflow {
            manifest: manifest(),
            steps: vec![
                step("01_local", StepKind::Cli, Placement::RequiresLocalFs),
                step(
                    "02_notion",
                    StepKind::McpTool,
                    Placement::RequiresCapability {
                        id: "notion".into(),
                    },
                ),
                step("03_pure", StepKind::Code, Placement::Anywhere),
            ],
            required_cli_binaries: vec![],
            required_mcp_servers: vec![],
            required_llm_providers: vec![],
        };

        let summary = assign_queues(&mut compiled, &me, &cluster).unwrap();
        assert_eq!(summary[0].task_queue, "cori.user.jean");
        assert_eq!(summary[1].task_queue, "cori.service.notion-pool");
        assert_eq!(summary[2].task_queue, "cori.user.jean");
        assert_eq!(
            compiled.steps[1].task_queue.as_deref(),
            Some("cori.service.notion-pool"),
        );
    }

    #[test]
    fn service_requesting_local_fs_is_rejected() {
        let svc = service("billing");
        let cluster = ClusterView::default();
        let mut compiled = CompiledWorkflow {
            manifest: manifest(),
            steps: vec![step("01_x", StepKind::Cli, Placement::RequiresLocalFs)],
            required_cli_binaries: vec![],
            required_mcp_servers: vec![],
            required_llm_providers: vec![],
        };
        let err = assign_queues(&mut compiled, &svc, &cluster).unwrap_err();
        assert!(matches!(err, PlacementError::LocalFsFromService { .. }));
    }

    #[test]
    fn missing_capability_when_user_worker_lacks_it() {
        let me = person("alice");
        let mut cluster = ClusterView::default();
        cluster.add_self(report_with(
            me.clone(),
            vec![("local_fs", CapabilityKind::LocalFs)],
        ));
        let mut compiled = CompiledWorkflow {
            manifest: manifest(),
            steps: vec![step(
                "01_notion",
                StepKind::McpTool,
                Placement::RequiresCapability {
                    id: "notion".into(),
                },
            )],
            required_cli_binaries: vec![],
            required_mcp_servers: vec![],
            required_llm_providers: vec![],
        };
        let err = assign_queues(&mut compiled, &me, &cluster).unwrap_err();
        assert!(matches!(err, PlacementError::MissingCapability { .. }));
    }

    #[test]
    fn solo_no_cluster_falls_through_to_user_queue() {
        let me = person("solo");
        let cluster = ClusterView::default();
        let mut compiled = CompiledWorkflow {
            manifest: manifest(),
            steps: vec![
                step(
                    "01_notion",
                    StepKind::McpTool,
                    Placement::RequiresCapability {
                        id: "notion".into(),
                    },
                ),
                step("02_pure", StepKind::Code, Placement::Anywhere),
            ],
            required_cli_binaries: vec![],
            required_mcp_servers: vec![],
            required_llm_providers: vec![],
        };
        let summary = assign_queues(&mut compiled, &me, &cluster).unwrap();
        assert_eq!(summary[0].task_queue, "cori.user.solo");
        assert_eq!(summary[1].task_queue, "cori.user.solo");
    }
}

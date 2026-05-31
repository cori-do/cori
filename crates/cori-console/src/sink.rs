//! `cori_run::ProgressSink` impl that forwards events into a [`RunChannel`].

use std::sync::Arc;

use cori_protocol::StepKind;
use cori_run::ProgressSink;
use cori_run::planner;
use cori_worker::workflow::ActivitySummary;

use crate::runs::{PlanStep, RunChannel, RunEvent};

pub struct ConsoleProgressSink {
    pub channel: Arc<RunChannel>,
}

impl ProgressSink for ConsoleProgressSink {
    fn on_plan(&self, plan: &[planner::StepAssignment]) {
        let assignments = plan
            .iter()
            .map(|a| PlanStep {
                activity_id: a.activity_id.clone(),
                step_name: a.step_name.clone(),
                task_queue: a.task_queue.clone(),
            })
            .collect();
        self.channel.push(RunEvent::Plan { assignments });
    }

    fn on_step_start(&self, s: &ActivitySummary) {
        self.channel.push(RunEvent::StepStart {
            activity_id: s.activity_id.clone(),
            step_name: s.step_name.clone(),
            kind: kind_label(s.kind).to_string(),
            task_queue: s.route.clone(),
        });
    }

    fn on_step_finish(&self, s: &ActivitySummary) {
        self.channel.push(RunEvent::StepFinish {
            activity_id: s.activity_id.clone(),
            step_name: s.step_name.clone(),
            status: s.status.clone(),
            duration_ms: s.duration_ms,
            error: s.error.clone(),
        });
    }
}

fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Cli => "cli",
        StepKind::McpTool => "mcp_tool",
        StepKind::Code => "code",
        StepKind::Llm => "llm",
        StepKind::Builtin => "builtin",
    }
}

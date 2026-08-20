//! Compiled effect surface (v1).
//!
//! One [`StepEffect`] per compiled step, derived from the step kind and
//! its frozen metadata — never self-declared by an agent. This is the
//! source for the Console's "Touches" block and, later, the effects
//! diff between workflow versions.
//!
//! v1 is deliberately conservative about writes: an operation that
//! cannot be *proven* read-only is classified [`EffectAccess::MayWrite`].
//! Two classifications are proofs, not heuristics:
//!
//! - `code` steps are effect-free: the runner sandbox grants read-only
//!   access to the workflow folder and nothing else — no network, no
//!   write, no subprocess (see the Deno flags in `cori_broker::dispatch`).
//! - `llm` steps mutate nothing but do send the accumulated step input
//!   to the active AI provider — an exfiltration surface worth naming,
//!   distinct from a write.
//!
//! MCP tools get a narrow read-only allowlist over the frozen tool
//! name's leading verb; CLI steps freeze only the binary (argv is built
//! at run time from the input), so they are always `MayWrite`.

use cori_protocol::{CompiledStep, CompiledWorkflow, StepKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectAccess {
    /// Provably no effect beyond pure compute (sandboxed `code`).
    None,
    /// External read, proven by the tool name's leading verb.
    Read,
    /// Sends the step input to an AI provider; mutates nothing.
    Prompt,
    /// Could not be proven read-only — treated as a write.
    MayWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepEffect {
    pub activity_id: String,
    pub step_name: String,
    /// `cli` | `mcp_tool` | `code` | `llm` | `builtin`.
    pub kind: String,
    /// Human-facing target: `gh` · `gws · sheets.update` · `AI provider`.
    pub target: String,
    pub access: EffectAccess,
    /// May touch the world beyond this machine's workflow folder.
    /// Conservative: unknown ⇒ true.
    pub external: bool,
}

/// Leading verbs that make an MCP tool name read-only. Anything else —
/// including a name this list has never seen — counts as a write.
const READ_VERBS: &[&str] = &[
    "get", "list", "read", "search", "find", "describe", "show", "query", "count", "fetch",
    "retrieve", "status", "peek", "lookup",
];

fn tool_reads_only(tool: &str) -> bool {
    let head = tool.rsplit(['.', '/', ':']).next().unwrap_or(tool);
    let verb = head
        .split(['_', '-'])
        .next()
        .unwrap_or(head)
        .to_ascii_lowercase();
    READ_VERBS.contains(&verb.as_str())
}

fn meta_str<'a>(s: &'a CompiledStep, key: &str) -> Option<&'a str> {
    s.metadata.get(key).and_then(|v| v.as_str())
}

/// Conservative severity order for merging nested builtin effects.
fn access_rank(access: EffectAccess) -> u8 {
    match access {
        EffectAccess::None => 0,
        EffectAccess::Read => 1,
        EffectAccess::Prompt => 2,
        EffectAccess::MayWrite => 3,
    }
}

/// The whole workflow's effect surface, in step order.
pub fn compute_effects(w: &CompiledWorkflow) -> Vec<StepEffect> {
    w.steps.iter().map(step_effect).collect()
}

fn step_effect(s: &CompiledStep) -> StepEffect {
    let base = |kind: &str, target: String, access: EffectAccess, external: bool| StepEffect {
        activity_id: s.activity_id.clone(),
        step_name: s.name.clone(),
        kind: kind.to_string(),
        target,
        access,
        external,
    };
    match s.kind {
        StepKind::Code => base(
            "code",
            "deno sandbox · read-only, no network".to_string(),
            EffectAccess::None,
            false,
        ),
        StepKind::Builtin => {
            let sub = meta_str(s, "builtin").unwrap_or("builtin");
            // The control flow itself is pure workflow code, but its
            // nested steps carry real effects. Surface the strongest
            // nested access so the proposal card can never under-report
            // what a branch case or loop body may do.
            let nested: Vec<(String, EffectAccess, bool)> = s
                .metadata
                .get("nested")
                .and_then(|v| v.as_object())
                .map(|slots| {
                    slots
                        .iter()
                        .filter_map(|(slot, meta)| {
                            let meta = meta.as_object()?;
                            let get = |key: &str| meta.get(key).and_then(|v| v.as_str());
                            // A routing path dispatches nothing — name the
                            // target so the review card shows where it goes.
                            if let Some(target) = get("goto_name").or_else(|| get("goto")) {
                                return Some((
                                    format!("{slot} → goto {target}"),
                                    EffectAccess::None,
                                    false,
                                ));
                            }
                            Some(match get("kind")? {
                                "cli" => (
                                    format!("{slot} → {}", get("binary").unwrap_or("?")),
                                    EffectAccess::MayWrite,
                                    true,
                                ),
                                "mcp_tool" => {
                                    let tool = get("tool").unwrap_or("?");
                                    let access = if tool_reads_only(tool) {
                                        EffectAccess::Read
                                    } else {
                                        EffectAccess::MayWrite
                                    };
                                    (
                                        format!(
                                            "{slot} → {} · {tool}",
                                            get("server").unwrap_or("?")
                                        ),
                                        access,
                                        true,
                                    )
                                }
                                "llm" => (
                                    format!(
                                        "{slot} → AI provider · {}",
                                        get("level").unwrap_or("medium")
                                    ),
                                    EffectAccess::Prompt,
                                    true,
                                ),
                                _ => (
                                    format!("{slot} → sandboxed code"),
                                    EffectAccess::None,
                                    false,
                                ),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if nested.is_empty() {
                base(
                    "builtin",
                    format!("cori runtime · {sub}"),
                    EffectAccess::None,
                    false,
                )
            } else {
                let access = nested
                    .iter()
                    .map(|(_, access, _)| *access)
                    .max_by_key(|access| access_rank(*access))
                    .unwrap_or(EffectAccess::None);
                let external = nested.iter().any(|(_, _, external)| *external);
                let targets = nested
                    .iter()
                    .map(|(target, _, _)| target.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                base("builtin", format!("{sub}: {targets}"), access, external)
            }
        }
        StepKind::Llm => {
            let level = meta_str(s, "level").unwrap_or("medium");
            base(
                "llm",
                format!("AI provider · {level}"),
                EffectAccess::Prompt,
                true,
            )
        }
        StepKind::Cli => {
            let binary = meta_str(s, "binary").unwrap_or("?");
            base(
                "cli",
                binary.to_string(),
                // Only the binary is frozen; argv is built at run time,
                // so nothing about this call is provably read-only.
                EffectAccess::MayWrite,
                true,
            )
        }
        StepKind::McpTool => {
            let server = meta_str(s, "server").unwrap_or("?");
            let tool = meta_str(s, "tool").unwrap_or("?");
            let access = if tool_reads_only(tool) {
                EffectAccess::Read
            } else {
                EffectAccess::MayWrite
            };
            base("mcp_tool", format!("{server} · {tool}"), access, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_verbs_are_conservative() {
        assert!(tool_reads_only("list_rows"));
        assert!(tool_reads_only("sheets.get_values"));
        assert!(tool_reads_only("search-issues"));
        // Unknown or mutating verbs fall to MayWrite.
        assert!(!tool_reads_only("update_values"));
        assert!(!tool_reads_only("send_message"));
        assert!(!tool_reads_only("frobnicate"));
        assert!(!tool_reads_only("delete"));
    }
}

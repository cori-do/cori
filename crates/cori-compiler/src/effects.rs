// SPDX-License-Identifier: Apache-2.0
//! Effect classification derived from the compiled step, never declared by the author.
use crate::steps::StepDecl;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access { None, Read, Prompt, MayWrite }

const READ_VERBS: &[&str] = &["get", "list", "read", "search", "find", "describe", "show", "query", "count", "fetch", "retrieve", "status", "peek", "lookup", "select"];

/// Whether a tool name reads by its verb (`getTicket`, `list_items`, `crm.search`): the effect rule of mcp and store steps.
pub fn verb_reads(tool: &str) -> bool {
    let head = tool.rsplit(['.', '/', ':']).next().unwrap_or(tool);
    let verb: String = head.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    // camelCase: `getTicket` → `get`
    let verb = if verb.is_empty() { head.split(['_', '-']).next().unwrap_or(head).to_ascii_lowercase() } else {
        let mut v = String::new();
        for (i, ch) in head.chars().enumerate() { if i > 0 && ch.is_ascii_uppercase() { break; } if ch == '_' || ch == '-' { break; } v.push(ch.to_ascii_lowercase()); }
        v
    };
    READ_VERBS.contains(&verb.as_str())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Effect {
    pub access: Access,
    /// Human-facing target: `helpdesk · getTicket`, `slack · POST /api/chat.postMessage`, `model (small)`.
    pub target: String,
    pub external: bool,
}

pub fn classify(d: &StepDecl) -> Effect {
    let lit = |k: &str| d.literals.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match d.kind.as_str() {
        "code" => Effect { access: Access::None, target: "pure".into(), external: false },
        "llm" => Effect { access: Access::Prompt, target: format!("model ({})", if lit("tier").is_empty() { "medium".into() } else { lit("tier") }), external: true },
        "http" => {
            let m = lit("method");
            let access = if matches!(m.as_str(), "GET" | "HEAD") { Access::Read } else { Access::MayWrite };
            Effect { access, target: format!("{} · {}", lit("connector"), if m.is_empty() { "?".into() } else { m }), external: true }
        }
        "mcp" => {
            let tool = lit("tool");
            Effect { access: if verb_reads(&tool) { Access::Read } else { Access::MayWrite }, target: format!("{} · {tool}", lit("connector")), external: true }
        }
        "store" => {
            let connector = if lit("connector").is_empty() { "default store".into() } else { lit("connector") };
            if d.literals.contains_key("query") {
                // the store runs a query as one SELECT in a read-only transaction and refuses anything else, whether its
                // text is written out or built when the step runs (`cori check` refuses a written one that is not a
                // SELECT): a query step reads
                Effect { access: Access::Read, target: format!("{connector} · query"), external: true }
            } else {
                let tool = lit("tool");
                Effect { access: if verb_reads(&tool) { Access::Read } else { Access::MayWrite }, target: format!("{connector} · {tool}"), external: true }
            }
        }
        "activity" => Effect { access: Access::MayWrite, target: format!("{} · {}@{}", lit("connector"), lit("activity"), d.literals.get("version").and_then(|v| v.as_f64()).map(|v| format!("{}", v as i64)).unwrap_or("?".into())), external: true },
        "cli" => Effect { access: Access::MayWrite, target: format!("{} · {}", lit("connector"), lit("argv0")), external: true },
        "human" => Effect { access: Access::None, target: format!("{} → {}", lit("kind"), d.literals.get("to").map(|v| v.to_string()).unwrap_or_default()), external: false },
        "workflow" => Effect { access: Access::MayWrite, target: format!("workflow {}", lit("id")), external: true },
        "wait" => Effect { access: Access::None, target: "timer".into(), external: false },
        "branch" | "switch" | "map" | "loop" | "try" => {
            let mut strongest = Effect { access: Access::None, target: d.kind.clone(), external: false };
            for n in &d.nested {
                let e = classify(n);
                if e.access > strongest.access { strongest.access = e.access; }
                strongest.external |= e.external;
            }
            strongest
        }
        _ => Effect { access: Access::MayWrite, target: "unknown".into(), external: true },
    }
}

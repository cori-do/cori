// SPDX-License-Identifier: Apache-2.0
//! Dataflow graph, ordering, conditional execution, schedule.
use crate::effects::{self, Access};
use crate::steps::StepDecl;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: String,
    /// The step's `description`, its author's words for what it does (empty in a DAG compiled before it was kept): what a
    /// run's inspection shows of the step, never its code.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub file: String,
    pub line: usize,
    pub index: usize,
    pub nested_in: Option<String>,
    pub slot: Option<String>,
    /// May not have run when a later step reads it (nested, or skippable by a goto).
    pub conditional: bool,
    pub depends_on: BTreeSet<String>,
    pub after: BTreeSet<String>,
    pub refs: BTreeSet<String>,
    pub effect: effects::Effect,
    pub literals: serde_json::Map<String, serde_json::Value>,
    pub goto_targets: Vec<(String, String)>,
    pub nested: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dag {
    pub nodes: Vec<Node>,
    /// Data and ordering edges among top-level steps: (from, to, kind)
    pub edges: Vec<(String, String, String)>,
    /// Parallel schedule of top-level steps: each inner list can run concurrently.
    pub schedule: Vec<Vec<String>>,
    /// Names that may be referenced from step `k` (declared order; nested scoping applied).
    pub predecessors: BTreeMap<String, Vec<String>>,
    /// The top-level steps a goto skips, by the top-level step holding the goto and its target (`end`: every later
    /// step), in declared order. The engine skips exactly these; a DAG compiled before this field has none, and the
    /// engine keeps its earlier rule for it (runs in flight replay unchanged).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub goto_skips: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

pub fn build(decls: &[StepDecl], ordering: &str) -> Result<Dag, Vec<String>> {
    let mut errors = Vec::new();
    // Flatten
    let mut nodes: Vec<Node> = Vec::new();
    fn flatten(d: &StepDecl, out: &mut Vec<Node>) {
        let effect = effects::classify(d);
        out.push(Node {
            name: d.name.clone(), kind: d.kind.clone(), description: d.description.clone(), file: d.file.clone(), line: d.line, index: d.index, nested_in: d.nested_in.clone(), slot: d.slot.clone(),
            conditional: d.nested_in.is_some(), depends_on: d.depends_on(), after: d.literals.get("after").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
            refs: d.refs.clone(), effect, literals: d.literals.clone(), goto_targets: d.goto_targets.clone(), nested: d.nested.iter().map(|n| n.name.clone()).collect(),
        });
        for n in &d.nested { flatten(n, out); }
    }
    for d in decls { flatten(d, &mut nodes); }
    // Unique names
    let mut seen = BTreeSet::new();
    for n in &nodes { if !seen.insert(n.name.clone()) { errors.push(format!("{}:{}: duplicate step name `{}`", n.file, n.line, n.name)); } }
    let top_names: Vec<String> = nodes.iter().filter(|n| n.nested_in.is_none()).map(|n| n.name.clone()).collect();
    let pos: BTreeMap<String, usize> = top_names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect();
    // A nested step's top-level anchor
    let anchor = |n: &Node| -> String { match &n.nested_in { Some(p) => p.clone(), None => n.name.clone() } };
    let mut skippable: BTreeSet<String> = BTreeSet::new();
    let mut goto_skips: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    let mut predecessors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
    let by_name: BTreeMap<&str, &Node> = nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    // Predecessors by declared order: everything strictly before the anchor, plus nested steps of earlier anchors
    for n in &nodes {
        let a = anchor(n); let ai = pos[a.as_str()];
        let mut preds = Vec::new();
        for m in &nodes { let ma = anchor(m); if pos[ma.as_str()] < ai && !matches!(m.kind.as_str(), "branch" | "switch" | "wait") { preds.push(m.name.clone()); } }
        predecessors.insert(n.name.clone(), preds);
    }
    // Validate refs and gotos
    for n in &nodes {
        let allowed: BTreeSet<&str> = predecessors[&n.name].iter().map(|s| s.as_str()).collect();
        for d in &n.depends_on {
            if !by_name.contains_key(d.as_str()) {
                let candidates: Vec<&str> = allowed.iter().copied().filter(|c| c.starts_with(&d[..d.len().min(3)])).collect();
                errors.push(format!("{}:{}: step `{}` reads `steps.{}` which does not exist{}", n.file, n.line, n.name, d, if candidates.is_empty() { String::new() } else { format!(" (did you mean {}?)", candidates.join(", ")) }));
            } else if !allowed.contains(d.as_str()) {
                errors.push(format!("{}:{}: step `{}` reads `steps.{}` which runs after it (only earlier steps may be read)", n.file, n.line, n.name, d));
            }
        }
        for a in &n.after { if !allowed.contains(a.as_str()) { errors.push(format!("{}:{}: `after: [{}]` must name an earlier step", n.file, n.line, a)); } }
        for (slot, t) in &n.goto_targets {
            if t == "end" { continue; }
            match pos.get(t.as_str()) { Some(&ti) if ti > pos[anchor(n).as_str()] => {}, Some(_) => errors.push(format!("{}:{}: goto(\"{t}\") in `{slot}` must point forward", n.file, n.line)), None => errors.push(format!("{}:{}: goto(\"{t}\") in `{slot}` targets an unknown top-level step", n.file, n.line)) }
        }
        if n.kind == "cli" && n.literals.get("connector").is_none() { errors.push(format!("{}:{}: `cli` steps must name a `connector`", n.file, n.line)); }
        if matches!(n.kind.as_str(), "http" | "mcp" | "activity") && n.literals.get("connector").is_none() { errors.push(format!("{}:{}: `{}` steps must name a `connector`", n.file, n.line, n.kind)); }
        if n.kind == "activity" && (n.literals.get("activity").is_none() || n.literals.get("version").is_none()) { errors.push(format!("{}:{}: `activity` steps need `activity` and `version` literals", n.file, n.line)); }
    }
    // Skippable by goto ⇒ conditional
    if errors.is_empty() {
    for n in &nodes { for (_, t) in &n.goto_targets {
        let from = pos[anchor(n).as_str()]; let to = if t == "end" { top_names.len() } else { pos[t.as_str()] };
        for name in &top_names[from + 1..to] { skippable.insert(name.clone()); for k in &by_name[name.as_str()].nested { skippable.insert(k.clone()); } }
        goto_skips.entry(anchor(n)).or_default().insert(t.clone(), top_names[from + 1..to].to_vec());
    } }
    // Edges among top-level steps
    let mut edge_set: BTreeSet<(String, String)> = BTreeSet::new();
    let mut add = |from: &str, to: &str, kind: &'static str, edges: &mut Vec<(String, String, String)>| { if from != to && edge_set.insert((from.to_string(), to.to_string())) { edges.push((from.to_string(), to.to_string(), kind.to_string())); } };
    for n in &nodes {
        let to = anchor(n);
        for d in &n.depends_on { let from = anchor(by_name[d.as_str()]); add(&from, &to, "data", &mut edges); }
        for a in &n.after { let from = anchor(by_name[a.as_str()]); add(&from, &to, "after", &mut edges); }
    }
    if ordering == "writes_by_declared_order" {
        let mut last_write: Option<String> = None;
        for name in &top_names {
            let n = by_name[name.as_str()];
            let writes = n.effect.access == Access::MayWrite;
            if writes { if let Some(prev) = &last_write { add(prev, &n.name, "write-order", &mut edges); } last_write = Some(n.name.clone()); }
        }
    }
    // a goto decides before the steps it may skip and its target run: ordering edges from the step holding it to each
    for (from, targets) in &goto_skips { for (t, skipped) in targets {
        for name in skipped { add(from, name, "goto", &mut edges); }
        if t != "end" { add(from, t, "goto", &mut edges); }
    } }
    }
    }
    // `try` bodies and loop/map bodies are conditional too (nested), branches are nested: already marked.
    for n in nodes.iter_mut() { if skippable.contains(&n.name) || n.kind == "try" { n.conditional = true; } }
    if !errors.is_empty() { return Err(errors); }
    // Schedule (Kahn, levels)
    let mut indeg: BTreeMap<&str, usize> = top_names.iter().map(|n| (n.as_str(), 0)).collect();
    let mut succ: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (f, t, _) in &edges { *indeg.get_mut(t.as_str()).unwrap() += 1; succ.entry(f.as_str()).or_default().push(t.as_str()); }
    let mut schedule = Vec::new();
    let mut ready: Vec<&str> = top_names.iter().filter(|n| indeg[n.as_str()] == 0).map(|n| n.as_str()).collect();
    let mut done = 0;
    while !ready.is_empty() {
        ready.sort_by_key(|n| pos[*n]);
        let level: Vec<String> = ready.iter().map(|s| s.to_string()).collect();
        let mut next = Vec::new();
        for r in &ready { done += 1; if let Some(ss) = succ.get(r) { for s in ss { let d = indeg.get_mut(s).unwrap(); *d -= 1; if *d == 0 { next.push(*s); } } } }
        schedule.push(level); ready = next;
    }
    if done != top_names.len() { return Err(vec!["dependency cycle detected among steps".into()]); }
    Ok(Dag { nodes, edges, schedule, predecessors, goto_skips })
}

#[cfg(test)]
mod tests {
    use crate::steps::{StepDecl, parse_step_file};

    fn decl(name: &str, body: &str, index: usize) -> StepDecl {
        let src = format!("import {{ step, goto }} from \"@cori-do/sdk\";\nexport default {body};\n");
        parse_step_file(&format!("steps/{name}.ts"), &src, index).map_err(|e| e.join("; ")).expect("parses").decl
    }

    #[test]
    fn a_goto_decides_before_the_steps_it_may_skip() {
        let decls = vec![
            decl("check", r#"step.code({ name: "check", run: () => ({ ok: true }) })"#, 0),
            decl("gate", r#"step.branch({ name: "gate", if: (ctx) => ctx.steps.check.ok, then: goto("done"), else: goto("end") })"#, 1),
            decl("send_offer", r#"step.code({ name: "send_offer", run: () => ({}) })"#, 2),
            decl("done", r#"step.code({ name: "done", run: () => ({}) })"#, 3),
        ];
        let dag = super::build(&decls, "writes_by_declared_order").unwrap();
        assert_eq!(dag.goto_skips["gate"]["done"], vec!["send_offer".to_string()]);
        assert_eq!(dag.goto_skips["gate"]["end"], vec!["send_offer".to_string(), "done".to_string()]);
        // `send_offer` reads nothing, yet waits for the gate: it may be skipped
        let level = |n: &str| dag.schedule.iter().position(|l| l.iter().any(|x| x == n)).unwrap();
        assert!(level("send_offer") > level("gate") && level("done") > level("gate"), "{:?}", dag.schedule);
    }

    /// What a run's inspection says a step does: its author's words, nested steps included.
    #[test]
    fn every_step_keeps_its_description() {
        let decls = vec![
            decl("check", r#"step.code({ name: "check", description: "Check the amount", run: () => ({ ok: true }) })"#, 0),
            decl("gate", r#"step.branch({ name: "gate", description: "Ask a manager above the limit", if: (ctx) => ctx.steps.check.ok, then: step.human({ name: "approve", description: "Approve or decline", kind: "approval", to: { role: "manager" } }) })"#, 1),
        ];
        let dag = super::build(&decls, "writes_by_declared_order").unwrap();
        let said = |n: &str| dag.nodes.iter().find(|x| x.name == n).unwrap().description.clone();
        assert_eq!((said("check"), said("gate"), said("approve")), ("Check the amount".into(), "Ask a manager above the limit".into(), "Approve or decline".into()));
        // a DAG stored before it was kept reads with none
        let mut old = serde_json::to_value(&dag.nodes[0]).unwrap();
        old.as_object_mut().unwrap().remove("description");
        assert_eq!(serde_json::from_value::<super::Node>(old).unwrap().description, "");
    }
}

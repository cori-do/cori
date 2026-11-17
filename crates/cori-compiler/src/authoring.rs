// SPDX-License-Identifier: Apache-2.0
//! Authoring helpers shared by the CLI (`cori workflow new`, `cori scaffold`, `cori connectors find`) and the
//! server's MCP tools (`scaffold`, `find_tool`): pure functions of the connector registry and their arguments.
//! Nothing here touches the network; file writes stay in the caller.
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

/// A generated step file for a connector tool, with what the caller needs to place it.
#[derive(Debug, Clone)]
pub struct Scaffold { pub connector: String, pub tool: String, pub step: String, pub kind: String, pub effect: String, pub file: String, pub input_schema: Value, pub output_schema: Option<Value> }

/// A ready-to-edit step file for `connector.tool` from the registry rows (`GET /v1/connectors` shape): typed input
/// from the tool's input schema, Zod output from its output schema, effect from its annotations.
pub fn scaffold(rows: &[Value], connector: &str, tool: &str, name: Option<&str>) -> Result<Scaffold> {
    let c = rows.iter().find(|c| c["name"].as_str() == Some(connector)).ok_or_else(|| anyhow!("connector `{connector}` is not declared (see `cori connectors list`)"))?;
    let kind = c["kind"].as_str().unwrap_or("mcp").to_string();
    let t = c["tools"].as_array().and_then(|a| a.iter().find(|t| t["name"].as_str() == Some(tool))).cloned();
    // a store's tools are always known (derived from its migrations and roles): an unknown name is an error even when none are
    if t.is_none() && (kind == "store" || !c["tools"].as_array().map(|a| a.is_empty()).unwrap_or(true)) {
        let names = c["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        return Err(anyhow!("connector `{connector}` has no tool `{tool}` (tools: {}){}", if names.is_empty() { "none" } else { &names }, if kind == "store" { format!("; `cori store tools {connector}` lists them per role") } else { String::new() }));
    }
    let step_name = name.map(String::from).unwrap_or_else(|| snake(tool));
    let input_schema = t.as_ref().map(|t| t["inputSchema"].clone()).unwrap_or(json!({ "type": "object" }));
    let output_schema = t.as_ref().and_then(|t| t.get("outputSchema").cloned()).filter(|o| !o.is_null());
    let input_lit = input_skeleton(&input_schema);
    let (output_zod, output_note) = match &output_schema { Some(o) => (indent(&crate::zodgen::zod(o), 1), if kind == "store" { "the store's result, typed from the role's readable columns".to_string() } else { "from the tool's published outputSchema".to_string() }), None => ("z.object({\n    // TODO: describe the fields this step exposes; `parse` maps the raw result onto them\n    result: z.unknown(),\n  })".into(), "the tool publishes no outputSchema: declare what the step exposes and map it in `parse`".into()) };
    let annotations = t.as_ref().and_then(|t| t.get("annotations").cloned()).unwrap_or(json!({}));
    // without annotations, the verb rule `cori check` applies (`getTicket` reads)
    let effect = if annotations["readOnlyHint"].as_bool() == Some(true) { "Read" } else if annotations["destructiveHint"].as_bool() == Some(true) { "Destructive (approval on first deploy)" } else if annotations["readOnlyHint"].is_null() && crate::effects::verb_reads(tool) { "Read" } else { "MayWrite" };
    let step_kind = match kind.as_str() { "store" => "store", "activity" => "activity", "http" => "http", "cli" => "cli", _ => "mcp" };
    // a command has no tool annotations: `cori check` counts every `cli` step as a write
    let effect = if step_kind == "cli" { "MayWrite" } else { effect };
    let desc = t.as_ref().and_then(|t| t["description"].as_str()).unwrap_or("").replace('"', "'");
    let file = if step_kind == "mcp" || step_kind == "store" {
        format!("import {{ step }} from \"@cori-do/sdk\";\nimport type {{ Ctx }} from \"../.cori/context\";\nimport {{ z }} from \"zod\";\n\n// {connector}.{tool}: {desc}\n// effect {effect} · output {output_note}\nexport default step.{step_kind}({{\n  name: \"{step_name}\",\n  description: \"{desc}\",\n  connector: \"{connector}\",\n  tool: \"{tool}\",\n  input: (ctx: Ctx<\"{step_name}\">) => {input_lit},\n  output: {output_zod},{parse}\n}});\n", parse = if output_schema.is_some() { String::new() } else { "\n  parse: (result: unknown) => ({ result }),".to_string() })
    } else if step_kind == "cli" {
        // `tool` names the subcommand; argv[0] is frozen at compile time and must be the connector's allow-listed binary
        let binary = c["transport"]["command"].as_array().and_then(|a| a.first()).and_then(|b| b.as_str()).or_else(|| c["transport"]["command"].as_str()).unwrap_or(connector);
        format!("import {{ step }} from \"@cori-do/sdk\";\nimport type {{ Ctx }} from \"../.cori/context\";\nimport {{ z }} from \"zod\";\n\n// {connector}: `{binary} {tool}` on the sandbox worker; argv[0] must stay `{binary}`, the connector's allow-listed binary\n// effect {effect}\nexport default step.cli({{\n  name: \"{step_name}\",\n  description: \"{desc}\",\n  connector: \"{connector}\",\n  input: (ctx: Ctx<\"{step_name}\">) => {input_lit},\n  output: z.object({{}}),   // TODO the fields `parse` returns\n  command: (input) => [\"{binary}\", \"{tool}\"],   // TODO the arguments, from `input`\n  parse: (stdout) => JSON.parse(stdout),   // parse(stdout, {{ stderr, exitCode }}, input)\n}});\n")
    } else if step_kind == "activity" {
        format!("import {{ step }} from \"@cori-do/sdk\";\nimport type {{ Ctx }} from \"../.cori/context\";\nimport {{ z }} from \"zod\";\n\n// {connector}.{tool}@{version}: {desc}\n// effect {effect}\nexport default step.activity({{\n  name: \"{step_name}\",\n  description: \"{desc}\",\n  connector: \"{connector}\",\n  activity: \"{tool}\",\n  version: {version},\n  input: (ctx: Ctx<\"{step_name}\">) => {input_lit},\n  output: {output_zod},\n}});\n", version = t.as_ref().and_then(|t| t.get("version")).and_then(|v| v.as_i64()).unwrap_or(1))
    } else {
        format!("import {{ step }} from \"@cori-do/sdk\";\nimport type {{ Ctx }} from \"../.cori/context\";\nimport {{ z }} from \"zod\";\n\nexport default step.http({{\n  name: \"{step_name}\",\n  description: \"{desc}\",\n  connector: \"{connector}\",\n  input: (ctx: Ctx<\"{step_name}\">) => ({{}}),\n  output: z.object({{ ok: z.boolean() }}),\n  request: () => ({{ method: \"GET\", path: \"/{tool}\" }}),\n  parse: (res) => ({{ ok: res.status === 200 }}),\n}});\n")
    };
    Ok(Scaffold { connector: connector.to_string(), tool: tool.to_string(), step: step_name, kind: step_kind.to_string(), effect: effect.to_string(), file, input_schema, output_schema })
}

/// The `input` object of a scaffolded step, typed from the tool's input schema: a placeholder of the right type per
/// required field (so the file type-checks as generated), optional fields commented out; every line says what to bind.
fn input_skeleton(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()).filter(|p| !p.is_empty()) else { return "({})".to_string() };
    let required: Vec<&str> = schema.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let mut names: Vec<&String> = props.keys().collect();
    names.sort_by_key(|n| !required.contains(&n.as_str()));
    let lines = names.iter().map(|f| {
        let p = &props[*f];
        let ty = p["type"].as_str().unwrap_or("string");
        let placeholder = match p.get("enum").and_then(|e| e.as_array()).and_then(|e| e.first()) { Some(v) => v.to_string(), None => match ty { "integer" | "number" => "0".into(), "boolean" => "false".into(), "object" => "{}".into(), "array" => "[]".into(), _ => "\"\"".into() } };
        let about = [Some(ty.to_string()), p.get("format").and_then(|x| x.as_str()).map(String::from), p.get("enum").map(|e| format!("one of {e}")), p.get("default").map(|d| format!("default {d}")), p.get("description").and_then(|d| d.as_str()).map(String::from)].into_iter().flatten().collect::<Vec<_>>().join(", ");
        if required.contains(&f.as_str()) { format!("    {f}: {placeholder},   // TODO required ({about}): bind from ctx.params.<p> or ctx.steps.<step>.<field>") }
        else { format!("    // {f}: {placeholder},   // optional ({about})") }
    }).collect::<Vec<_>>().join("\n");
    format!("({{\n{lines}\n  }})")
}

/// Ranked tool candidates for a few words (name, description, schema fields); at most 20.
pub fn find_tools(rows: &[Value], query: &str) -> Vec<Value> {
    let q = query.to_lowercase();
    let mut hits = Vec::new();
    for c in rows {
        for t in c["tools"].as_array().cloned().unwrap_or_default() {
            let hay = format!("{} {} {}", t["name"].as_str().unwrap_or(""), t["description"].as_str().unwrap_or(""), t["inputSchema"].to_string()).to_lowercase();
            let score = q.split_whitespace().filter(|w| hay.contains(w)).count();
            if score > 0 { hits.push(json!({ "connector": c["name"], "tool": t["name"], "description": t["description"], "score": score, "inputSchema": t["inputSchema"], "annotations": t["annotations"] })); }
        }
    }
    hits.sort_by(|a, b| b["score"].as_u64().cmp(&a["score"].as_u64()));
    hits.into_iter().take(20).collect()
}

/// A one-line-per-tool view of the registry: what an agent reads first (names, kinds, effects), no schemas.
pub fn summarize_connectors(rows: &[Value]) -> Vec<Value> {
    rows.iter().map(|c| json!({
        "name": c["name"], "kind": c["kind"], "description": c["description"], "source": c.get("source").cloned().unwrap_or(json!("registry")),
        "tools": c["tools"].as_array().cloned().unwrap_or_default().iter().map(|t| { let a = &t["annotations"]; let effect = if a["readOnlyHint"].as_bool() == Some(true) { "Read" } else if a["destructiveHint"].as_bool() == Some(true) { "Destructive" } else { "MayWrite" }; json!({ "name": t["name"], "description": t["description"].as_str().unwrap_or("").chars().take(120).collect::<String>(), "effect": effect }) }).collect::<Vec<_>>(),
    })).collect()
}

/// One parameter of `cori workflow new --params name:type[:description]`.
#[derive(Debug, Clone)]
pub struct Param { pub name: String, pub ty: String, pub description: Option<String>, pub required: bool }

impl Param {
    /// `ticket_id:string`, `amount:number:Amount in EUR`, `note:string?` (optional).
    pub fn parse(s: &str) -> Result<Param> {
        let mut parts = s.splitn(3, ':');
        let name = parts.next().unwrap_or("").trim().to_string();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return Err(anyhow!("bad parameter `{s}`: name:type[:description]")); }
        let mut ty = parts.next().unwrap_or("string").trim().to_string();
        let required = !ty.ends_with('?'); if !required { ty.pop(); }
        if !matches!(ty.as_str(), "string" | "number" | "boolean" | "enum" | "list" | "object") { return Err(anyhow!("bad parameter type `{ty}` for `{name}` (string, number, boolean, enum, list, object)")); }
        Ok(Param { name, ty, description: parts.next().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()), required })
    }
}

/// Endpoint of a new workflow: `sync` or `async`, with the roles allowed to call it.
#[derive(Debug, Clone)]
pub struct Endpoint { pub mode: String, pub roles: Vec<String> }

/// The text of a fresh `workflow.yaml`: pure YAML, `steps: []` to be filled by `cori scaffold` or by hand.
pub fn workflow_yaml(id: &str, name: &str, description: &str, params: &[Param], connectors: &[String], endpoint: Option<&Endpoint>, schedule: Option<(&str, &str)>) -> Result<String> {
    if id.is_empty() || !id.chars().next().unwrap().is_ascii_lowercase() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') { return Err(anyhow!("workflow id must be snake_case (got `{id}`)")); }
    let param_lines: Vec<String> = params.iter().map(|p| format!("  - {{ name: {}, type: {}{}{} }}", p.name, p.ty, p.description.as_ref().map(|d| format!(", description: {}", serde_json::to_string(d).unwrap_or_default())).unwrap_or_default(), if p.required { String::new() } else { ", required: false".to_string() })).collect();
    let endpoint_y = match endpoint { Some(e) => format!("endpoint:\n  auth: sso\n  allow: {{ roles: [{}] }}\n  mode: {}\n  # idempotency: none (reads, toggles, anything that must run on every call) · parameters (a form submit: same caller + parameters within 24 h replays) · header (the client's Idempotency-Key)\n  # unset: none without writes, parameters with writes (and `cori check` warns)\n", e.roles.join(", "), e.mode), None => String::new() };
    let schedule_y = match schedule { Some((cron, tz)) => format!("schedule: \"{cron}\"\nschedule_tz: {tz}\n"), None => String::new() };
    let (name_y, description_y) = (serde_json::to_string(name).unwrap_or_default(), serde_json::to_string(&description.replace('\n', " ")).unwrap_or_default());
    // the headline names the first parameter when there is one; `{{ steps.<step>.<field> }}` once the steps exist
    let headline = match params.first() { Some(p) => format!("{name}: {{{{ params.{} }}}}", p.name), None => name.to_string() };
    Ok(format!("id: {id}\nname: {name_y}\ndescription: {description_y}\nparameters:\n{}\nconnectors: [{}]\nsteps: []\n{endpoint_y}{schedule_y}result:\n  headline: {}\n  fields: []   # [{{ label: Status, path: steps.<step>.<field> }}]\ntags: []\n", if param_lines.is_empty() { "  []".to_string() } else { param_lines.join("\n") }, connectors.join(", "), serde_json::to_string(&headline).unwrap_or_default()))
}

/// Splits `steps:` out of a workflow.yaml: (the list, the text with a placeholder). Flow (`steps: [a, b]`) and block
/// (`steps:\n  - a`) forms are both read; `join_steps` writes the flow form back at the same place.
pub fn split_steps(text: &str) -> Result<(Vec<String>, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(i) = lines.iter().position(|l| l.starts_with("steps:")) else { return Err(anyhow!("workflow.yaml has no `steps:` entry")) };
    let head = lines[i]["steps:".len()..].trim();
    let mut steps = Vec::new(); let mut end = i + 1;
    if let Some(inner) = head.strip_prefix('[').and_then(|h| h.strip_suffix(']')) { steps.extend(inner.split(',').map(|x| x.trim().trim_matches(['"', '\'']).to_string()).filter(|x| !x.is_empty())); }
    else if head.is_empty() { while end < lines.len() { let l = lines[end].trim_start(); if let Some(item) = l.strip_prefix("- ") { steps.push(item.trim().trim_matches(['"', '\'']).to_string()); end += 1; } else if l.is_empty() { end += 1; } else { break; } } }
    else { return Err(anyhow!("workflow.yaml: `steps:` must be a list ([a, b] or `- a` lines)")); }
    let mut rest: Vec<String> = lines[..i].iter().map(|l| l.to_string()).collect();
    rest.push("steps: __STEPS__".into());
    rest.extend(lines[end..].iter().map(|l| l.to_string()));
    Ok((steps, rest.join("\n") + "\n"))
}

pub fn join_steps(steps: &[String], rest: &str) -> String { rest.replace("steps: __STEPS__", &format!("steps: [{}]", steps.join(", "))) }

/// Puts `name` into `steps:` of a workflow.yaml text: after `after` (`start` = first), or last. A name already listed
/// moves. Returns the new text and the list.
pub fn insert_step(text: &str, name: &str, after: Option<&str>) -> Result<(String, Vec<String>)> {
    if name.is_empty() || !name.chars().next().unwrap().is_ascii_lowercase() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') { return Err(anyhow!("step name must be snake_case (got `{name}`)")); }
    let (mut steps, rest) = split_steps(text)?;
    if let Some(i) = steps.iter().position(|s| s == name) { steps.remove(i); }
    let pos = match after { Some("start") => 0, Some(a) => steps.iter().position(|s| s == a).map(|i| i + 1).ok_or_else(|| anyhow!("`--after {a}` names no step (steps: {})", steps.join(", ")))?, None => steps.len() };
    steps.insert(pos, name.to_string());
    Ok((join_steps(&steps, &rest), steps))
}

/// The step kinds as data (what `cori check --conventions` prints and the MCP instructions cite). Option names
/// are the SDK's (`packages/sdk/index.ts`); a test keeps them in sync.
pub fn step_kinds() -> Value {
    json!([
        { "kind": "code", "purpose": "pure TypeScript over typed inputs; the place for rules and decisions", "options": "name, description, input(ctx), output (zod), run(input)", "effect": "None (pure)", "retries": 3, "output": "what `run` returns, validated by `output`" },
        { "kind": "llm", "purpose": "one model call with a fixed schema; tier small|medium|large; distillable", "options": "name, description, tier, input(ctx), output (zod), prompt(input), files?(input) → the files its model reads, fallback?", "effect": "Prompt (its inputs reach a model)", "retries": 3, "output": "the model's answer parsed by `output`; under `cori run` fixtures/llm.json answers by step name" },
        { "kind": "mcp", "purpose": "one tool of a declared MCP connector", "options": "name, description, connector, tool, input(ctx), output (zod), parse?(result)", "effect": "Read when the tool's name starts with a read verb (get, list, search, find, query, …), else MayWrite", "retries": "3 for reads, else 1", "output": "the tool result (structuredContent), or what `parse(result)` returns" },
        { "kind": "store", "purpose": "a Cori Store tool (getX/listX/createX/updateX) or a guarded SQL `query`", "options": "name, description, connector, tool | query {text, params}, input(ctx), output (zod), parse?(result) — parse?(rows) for a query", "effect": "Read for a tool named with a read verb (get, list, search, find, query, …) and for every query (the store runs one SELECT, read-only, and `cori check` refuses a written query that is anything else), else MayWrite; a `requires_approval` column parks the change for an approver", "retries": "3 for reads, 1 for writes", "output": "tool: the row(s); query: `parse(rows)` over the rows of `text` (positional `$1, $2…` bound from `params`, e.g. params: [\"$1\"] ← input.id)" },
        { "kind": "http", "purpose": "an HTTP API behind a declared connector (base_url + credential from the broker)", "options": "name, description, connector, input(ctx), output (zod), request(input) → {method, path, body?, query?}, parse?(res)", "effect": "GET/HEAD ⇒ Read, else MayWrite", "retries": "3 for GET/HEAD or idempotent, else 1", "output": "`parse(res)` over {status, headers, json, text}" },
        { "kind": "activity", "purpose": "a native Temporal activity on a team's own worker (Go, C#, Java…), by name@version", "options": "name, description, connector, activity, version, input(ctx), output (zod), heartbeat?", "effect": "MayWrite", "retries": "3, or the declaration's retry.max_attempts (activities.yaml); the step's retries win", "output": "the activity's result" },
        { "kind": "cli", "purpose": "a command on the sandbox worker (allow-listed binary)", "options": "name, description, connector, input(ctx), output (zod), command(input) → argv, parse(stdout)", "effect": "MayWrite", "retries": 1, "output": "`parse(stdout, {stderr, exitCode})`" },
        { "kind": "human", "purpose": "approval or input routed to a role; pauses the run; timeout with on_timeout default", "options": "name, description, kind: approval|input, to: {role} | {user: \"initiator\"}, input(ctx), output (zod), timeout, on_timeout", "effect": "None (the run pauses for the decision)", "retries": 0, "output": "the decision, validated by `output`; nested in a `branch`, it is addressable as steps.<name> (typed T | undefined)" },
        { "kind": "workflow", "purpose": "call another workflow of the app (child)", "options": "name, description, id, input(ctx), output (zod)", "effect": "MayWrite (the child's own steps carry their effects in its review)", "retries": "0: the child is not started again; its steps retry as their kinds do", "output": "the child's outputs" },
        { "kind": "builtins", "purpose": "control flow, not steps: branch {if, then, else}, switch {on, cases, default}, map {over, each, max_concurrency, on_item_error}, loop {body, until, max_iterations}, wait {for}, try {body, on_error}, goto(name)", "options": "see the conventions", "effect": "none", "retries": 0,
          "output": "map → { items: [...] } (one per element of `over`, in order); loop → the body's last output; try → the body's output, undefined when on_error ran; branch, switch and wait produce nothing: their nested steps keep their own name in ctx.steps, typed `T | undefined` because they may not have run" }
    ])
}

/// How `result:` in workflow.yaml renders (the run headline and fields the console and the client show).
pub fn result_conventions() -> Value {
    json!({
        "headline": "a string with `{{ path }}` placeholders; a path is `params.<name>` or `steps.<step>.<field>[.<field>…]` (dotted only: no filters, no `.length`, no `[0]`); a missing value renders as an empty string",
        "fields": "[{ label, path: steps.<step>.<field>, optional?: true }] — `path` is bare (no braces); `sections` has the same shape",
        "check": "`cori check` rejects unknown parameters and steps in these paths and warns when a field is not in the step's output schema"
    })
}

/// `AGENTS.md` written by `cori init` and `cori pull`: everything an agent needs to work in the repository with the
/// CLI alone. The remote MCP is optional (discovery, login, the console widgets).
pub const AGENTS_MD: &str = r#"# Working in this Cori app

This repository **is** the app. `cori.toml` at the root names it and its roles; each workflow is a folder under
`workflows/`. Deploying publishes the whole repository as one version after a change review. Edit files with your
own tools and run the `cori` commands below; nothing here goes through an MCP tool.

## Layout
```
cori.toml                    # app id, name, front, auth roles (strongest first), llm; [connectors.<x>] uses an org connector, [connectors.<x>.dev] mock = "fixtures/<x>.json" answers it locally; [[dev.personas]] sub, roles, name = the users the dev widget offers under `cori up`
AGENTS.md                    # this file
.cori/session                # the server (and organization) the cli works with here, and an agent's token (written by `cori login`, `cori init`, `cori pull`); ignored by git, never deployed
workflows/<id>/
  workflow.yaml              # id (= folder name), name, description, parameters, connectors, steps (ORDERED), endpoint | schedule, result, budget, redact, tags
  README.md                  # goal and how to verify (prose, optional)
  types.ts                   # shared zod schemas of the workflow
  steps/<name>.ts            # one step per file; the file name IS the step name (`name: "<name>"`)
  .cori/                     # generated by `cori workflow new` / `cori check` (context.ts, deno.json); never edit, never commit
lib/                         # optional TypeScript shared by several workflows (import "../../../lib/<file>.ts" from a step, "../../lib/<file>.ts" from types.ts); part of every workflow's hash
stores/<name>/               # optional Cori Store: rules.yaml, roles/<role>.yaml, migrations/*.sql, seed/*.sql (`cori store new <name>` scaffolds them)
                             # bound to steps by `[connectors.<x>] kind = "store", ref = "<name>"` in cori.toml; steps say `connector: "<x>"`
fixtures/<connector>.json    # dev mocks for `cori run`: {"tools": {"<tool>": <result>}, "query": {"<step>": [rows]} | [rows]}; llm.json: {"<step>": <output>}
                             # under `cori up` (and `cori run` while it runs) store connectors hit their real Postgres, not the fixture
front/                       # Vite + React + TypeScript + Tailwind (written by `cori init`); generated client front/src/lib/cori.ts, @cori-do/client vendored in front/vendor/
dist/                        # the built front served by Cori: `npm run build` in front/ before `cori deploy` (ignored by git; committed only for a plain front)
```

## The loop
```
cori connectors list                              # what the org declared: use these names in steps, never configure servers
cori connectors find "post a message"            # ranked tools with their schemas
cori workflow new <id> --params ticket_id:string --connectors helpdesk --endpoint sync --roles support_agent
cori scaffold <connector> <tool> --workflow <id> [--name <step>] [--after <step>]   # writes steps/<step>.ts, updates `steps:`
<edit workflows/<id>/steps/*.ts, types.ts, workflow.yaml with your own tools>
cori check workflows/<id>                         # dataflow proof, effects, lineage; `cori check .` for every workflow (+ stores vs migrations)
cori run workflows/<id> k=v                       # local run on Temporal with the fixtures; --decide step=json for human gates
cori run workflows/<id> k=v --user <sub>:<role>   # run as a user holding an endpoint role (default: the first `allow` role); --mock-llm forces fixtures/llm.json
cori deploy .                                     # a deployed version: change review, seeded sandbox stores, preview url; a human promotes it (`cori promote`)
```
`cori check` and `cori run` fetch the connector registry themselves from `.cori/session`; no MCP server is needed.
When the session has expired the cli says so: ask the human (or the `login` tool of the Cori MCP) for a new one.
On a server hosting several organizations the folder belongs to one: its commands never act in another (a token of another
organization is refused); `cori login --org <slug>` signs in to the folder's.
An agent session (from the `login` tool) deploys and pulls versions of this app and reads their sandbox stores
(`cori store query|tools <store> --version <hash>`); promotion, approvals, live data and live runs need a person: the
server answers 403 naming the human path, so report the command instead of retrying.
Before writing the first step, read a finished app once: `cori init /tmp/ref --template support-desk --no-register`
holds one store step (tool and `query`), one `llm` step, one `human` gate inside a `branch`, one `map`, fixtures for
every connector kind, a store with roles, and a front that calls the endpoint. `--template team-requests` is a smaller
one that runs on any server as it is (a store, an `llm` step, an approval and a React front with its screens).

## Rules
- `steps:` in `workflow.yaml` is the ordered list of top-level steps. A file `steps/x.ts` that is not listed fails
  `cori check`; a listed name without its file fails too. Steps nested in `branch`, `map`, `try`… stay inside their
  parent's file. Writes execute in `steps:` order; reads run as early as their inputs allow (parallel when independent).
- A step file: `import { step } from "@cori-do/sdk"; import type { Ctx } from "../.cori/context"; import { z } from "zod";`
  then `export default step.<kind>({ name: "<file name>", description, input: (ctx: Ctx<"<name>">) => ({ ... }), output: z.object({ ... }), ...kind options })`.
  `input` may only read `ctx.params.*`, `ctx.user.*`, `ctx.run.*`, `ctx.steps.<earlier step>.<field>`, `ctx.item` (inside `map`).
  Those references are the dataflow; independent steps run in parallel automatically.
- Step kinds: `code` (pure rules and decisions), `llm` (fixed zod `output`, a `tier`), `mcp` / `store` / `http` /
  `activity` / `cli` (one declared connector each), `human` (routed to a role, with `timeout` and `on_timeout`),
  `workflow` (a child). Control flow: `branch`, `switch`, `map` (with `ctx.item`), `loop`, `wait`, `try`, `goto`.
  `cori check --conventions` prints the option shapes.
- `connectors:` in `workflow.yaml` lists every connector the workflow calls. Never invent a connector: use the names
  the org declared; `cori.toml` only overrides or mocks them. If none fits, ask an admin (`request_connector`).
- Stores are the exception: an app's own data lives in `stores/<name>/`, created with `cori store new <name>`, no admin
  needed. A table is `global: true` (shared: every reader sees every row) or `tenant: <column>` (rows per user, filtered
  and filled from the caller: `cori store new <name> --tenant owner`); a role that sees every user's rows has
  `scope: all` on the table in its role file. `deletable: true` in a role file gives `deleteX`. Postgres enforces the
  role files (grants, row level security): a table `rules.yaml` does not name has no visible row.
  A `query` with values uses the function form `query: ({ x }) => ({ text: "… $1", params: [x] })`; a static `params`
  array is never bound.
- Roles: `[auth] roles` in `cori.toml` lists the app's roles **strongest first**
  (`roles = { manager = "group:managers", employee = "group:employees" }`). A person holding several acts in every store
  as the first of them listed, so that role's files grant everything the roles after it may do (a manager who logs
  time like an employee has the employee's tools in `manager.yaml` too). `cori check` warns when a role listed later
  may do everything an earlier one may, and more.
- Files people give a run: a `file` (or `files`) parameter with `accept: [application/pdf, image/*]` and `max_size: 10MB`.
  Steps see its reference (`ctx.params.x`: `{ id, name, size, content_type, sha256 }`), never the bytes; an `llm` step
  gives the file itself to its model with `files: ({ x }) => [x]`. The front passes a browser `File` to `run()`, which
  uploads it first; `cori run workflows/<id> x=./file.pdf` reads it from disk.
- Decisions live in `code` steps, never in prompts. Effects are derived; new writes show in the change review, and a
  version that adds writes needs an approver before it is promoted (a protected app: every promotion does). Approve and
  promote are human actions; `cori deploy` never changes what users see.
- `result:` — `headline: "Refund {{ params.amount }}: {{ steps.outcome.status }}"` and `fields: [{ label, path: steps.x.y }]`.
  Paths are dotted `params.<name>` / `steps.<step>.<field>` only (no filters, no `.length`); a missing value renders
  empty. `cori check` rejects unknown steps and parameters in them.
- `endpoint.idempotency`: `parameters` (same caller + same parameters within 24 h replays the run) for writes started
  from a form, `header` when the client sends its own `Idempotency-Key` header (the generated client's
  `start(input, { idempotencyKey })`; the same key with different parameters is a 422), `none` for reads and for
  workflows that must run on every call, like a toggle (open → done → open repeats the first call's parameters). Unset,
  it is `none` for a workflow without writes and `parameters` with writes, and `cori check` warns. Previews honor it too (keyed per version). Only a running or succeeded run
  is replayed: after a failed run the same request runs again.
- `redact:` lists `steps.<step>.<field>` paths that never reach stored outputs or captured LLM inputs. A store column
  tagged `pii` in rules.yaml that a step exposes without such an entry is reported by `cori check`.
- Builtin outputs: `map` → `{ items: [...] }`; `loop`/`try` → the body's output; `branch`/`switch`/`wait` → nothing, the
  steps nested in them keep their own name in `ctx.steps`, typed `T | undefined`.

## Pitfalls
- `cori check` generates `.cori/context.ts`: run it after adding or renaming a step, before editing bindings against `Ctx`.
- Shared code goes in `lib/` at the app root; nothing outside the app folder can be imported by a step.
- `cori run --user` is `<sub>:<role,role>` (the bearer under `cori up` is `dev:<sub>:<roles>`; the `dev:` prefix is stripped).
  Without `--user` the run takes the endpoint's first `allow` role.
- Store fixtures do not run SQL: a `query` fixture is returned as is. Key it by step (`"query": {"<step>": [rows]}`) so
  each query gets its rows, and run against the real store (`cori up`, or `--store-database-url`) before deploying.
- `[llm] provider` other than `mock` calls the provider from `cori run` too; keep `[llm.dev] provider = "mock"` (or
  pass `--mock-llm`) for local runs.
- A store `create` on an existing key fails the step; wrap it in `try` when a duplicate is expected, and give the
  endpoint `idempotency: parameters` or `header` so a double submit replays instead of writing twice.
- The change review compares a version with the promoted one, never with an earlier version; every deployed version
  stays testable and promotable.
- Every deployed version gets its own sandbox database per store, migrated from scratch and seeded from
  `stores/<name>/seed/*.sql`: its preview url writes there, never in the live database, which only `cori promote`
  migrates (never seeds). `cori run` uses a local database of its own (`cori run --reset-store` recreates it after a
  migration or seed edit). Put demo rows in `seed/`, only DDL in `migrations/`, and keep migrations additive: a
  promotion applies them to live data and a rollback undoes none.
"#;

fn snake(s: &str) -> String { let mut out = String::new(); for (i, ch) in s.chars().enumerate() { if ch.is_uppercase() { if i > 0 { out.push('_'); } out.extend(ch.to_lowercase()); } else if ch == '-' || ch == '.' { out.push('_'); } else { out.push(ch); } } out }
fn indent(s: &str, n: usize) -> String { s.lines().enumerate().map(|(i, l)| if i == 0 { l.to_string() } else { format!("{}{l}", "  ".repeat(n)) }).collect::<Vec<_>>().join("\n") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steps_insert_and_move() {
        let y = "id: t\nsteps: []\nresult: {}\n";
        let (y, s) = insert_step(y, "a", None).unwrap(); assert_eq!(s, ["a"]);
        let (y, s) = insert_step(&y, "b", None).unwrap(); assert_eq!(s, ["a", "b"]);
        let (y, s) = insert_step(&y, "c", Some("start")).unwrap(); assert_eq!(s, ["c", "a", "b"]);
        let (y, s) = insert_step(&y, "a", Some("b")).unwrap(); assert_eq!(s, ["c", "b", "a"]);
        assert!(y.contains("steps: [c, b, a]")); assert!(insert_step(&y, "x", Some("nope")).is_err());
        let block = "id: t\nsteps:\n  - a\n  - b\nresult: {}\n"; let (_, s) = insert_step(block, "z", Some("a")).unwrap(); assert_eq!(s, ["a", "z", "b"]);
    }
    #[test]
    fn scaffold_from_registry() {
        let rows = vec![json!({ "name": "helpdesk", "kind": "mcp", "tools": [{ "name": "getTicket", "description": "Read a ticket", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } } }, "outputSchema": { "type": "object", "properties": { "status": { "type": "string", "enum": ["open", "closed"] } }, "required": ["status"] }, "annotations": { "readOnlyHint": true } }] })];
        let s = scaffold(&rows, "helpdesk", "getTicket", None).unwrap();
        assert_eq!(s.step, "get_ticket"); assert_eq!(s.effect, "Read"); assert!(s.file.contains("status: z.enum")); assert!(s.file.contains("// id: \"\","), "{}", s.file);
        // a store connector: unknown tools are rejected even with no tool rows; typed input; `parse` annotated; verb-based effect
        let store = vec![json!({ "name": "todos", "kind": "store", "tools": [{ "name": "getItem", "inputSchema": { "type": "object", "properties": { "id": { "type": "string", "description": "Item primary key column (id)" } }, "required": ["id"] }, "annotations": { "readOnlyHint": true } }] })];
        let e = scaffold(&store, "todos", "totallyBogus", None).unwrap_err().to_string(); assert!(e.contains("tools: getItem") && e.contains("cori store tools todos"), "{e}");
        assert!(scaffold(&[json!({ "name": "todos", "kind": "store", "tools": [] })], "todos", "getItem", None).is_err());
        let s = scaffold(&store, "todos", "getItem", None).unwrap();
        assert_eq!(s.effect, "Read"); assert!(s.file.contains("    id: \"\",   // TODO required (string"), "{}", s.file); assert!(s.file.contains("parse: (result: unknown) =>"), "{}", s.file);
        let bare = vec![json!({ "name": "legacy", "kind": "mcp", "tools": [] })];
        assert_eq!(scaffold(&bare, "legacy", "listThings", None).unwrap().effect, "Read"); assert_eq!(scaffold(&bare, "legacy", "createThing", None).unwrap().effect, "MayWrite");
        assert!(scaffold(&rows, "helpdesk", "nope", None).is_err()); assert!(scaffold(&rows, "crm", "x", None).is_err());
        // a command: a `cli` step whose argv[0] is the connector's binary, counted as a write
        let cli = vec![json!({ "name": "gws", "kind": "cli", "tools": [], "transport": { "command": ["gws"] } })];
        let s = scaffold(&cli, "gws", "sheets", Some("append_sheet")).unwrap();
        assert_eq!((s.kind.as_str(), s.effect.as_str()), ("cli", "MayWrite")); assert!(s.file.contains("step.cli({") && s.file.contains("command: (input) => [\"gws\", \"sheets\"]"), "{}", s.file);
        assert_eq!(find_tools(&rows, "read ticket")[0]["tool"], "getTicket");
    }
    #[test]
    fn workflow_manifest() {
        let p = vec![Param::parse("ticket_id:string").unwrap(), Param::parse("note:string?:Free text").unwrap()];
        let y = workflow_yaml("triage", "Triage", "Sort tickets", &p, &["helpdesk".into()], Some(&Endpoint { mode: "sync".into(), roles: vec!["support_agent".into()] }), None).unwrap();
        assert!(y.contains("steps: []")); assert!(y.contains("required: false")); assert!(y.contains("allow: { roles: [support_agent] }"));
        assert!(y.contains("headline: \"Triage: {{ params.ticket_id }}\""), "{y}");
        assert!(workflow_yaml("Bad", "", "", &[], &[], None, None).is_err()); assert!(Param::parse("x:blob").is_err());
        // no parameter: a static headline, never `{{ params.id }}`
        let y = workflow_yaml("get_dashboard", "get_dashboard", "", &[], &[], Some(&Endpoint { mode: "sync".into(), roles: vec![] }), None).unwrap();
        assert!(y.contains("headline: \"get_dashboard\"\n"), "{y}"); assert!(!y.contains("params.id"));
        assert!(crate::manifest::parse(&y).is_ok(), "{y}");
    }
    /// Every option name the conventions print exists on the SDK's option types (`packages/sdk/index.ts`).
    #[test]
    fn step_kind_options_match_the_sdk() {
        let sdk = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/sdk/index.ts")).expect("packages/sdk/index.ts");
        for k in step_kinds().as_array().unwrap() {
            if k["kind"] == "builtins" { continue; }
            for opt in k["options"].as_str().unwrap().split(", ") {
                for alt in opt.split(" | ") {
                    let ident: String = alt.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                    if ident.is_empty() { continue; }
                    assert!(sdk.contains(&format!("readonly {ident}:")) || sdk.contains(&format!("readonly {ident}?:")), "`{}` option `{ident}` is not in the SDK", k["kind"]);
                }
            }
        }
        assert!(sdk.contains("{ readonly user: \"initiator\" }") && !step_kinds().to_string().contains("initiator: true"));
    }
}

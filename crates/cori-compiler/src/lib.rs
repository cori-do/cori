// SPDX-License-Identifier: Apache-2.0
//! Cori compiler v2: `workflow.yaml` + oxc step extraction + dataflow proof + effects.
//! A workflow is `workflows/<id>/` (id = folder name) with `workflow.yaml` (`steps:` ordered), `types.ts` and `steps/<name>.ts`.
pub mod zodgen;
pub mod runtime;
pub mod authoring;
pub mod tsgen;
pub mod check;
pub mod context;
pub mod dag;
pub mod effects;
pub mod files;
pub mod manifest;
pub mod roles;
pub mod steps;
pub mod stores;
pub mod timing;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compiled {
    pub id: String,
    pub content_hash: String,
    pub manifest: manifest::Manifest,
    pub dag: dag::Dag,
    /// External steps and the context fields that reach them (direct + through pure steps).
    pub lineage: BTreeMap<String, Vec<String>>,
    pub warnings: Vec<String>,
    pub type_check: Option<TypeCheck>,
    /// JSON Schema of every step's `output` (top-level and nested), when the Deno passes ran.
    #[serde(default)]
    pub schemas: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeCheck { pub ok: bool, pub diagnostics: Vec<String> }

pub struct Options { pub run_deno: bool, pub sdk_path: Option<String>,
    /// Declared connectors with their tool declarations (registry rows: `{ name, tools: [{ name, inputSchema }] }`):
    /// the generated context types every `step.mcp` input against the tool's schema (a `step.store` input is checked by
    /// the store when it runs).
    pub connectors: Vec<serde_json::Value>,
    /// Where the Deno passes run (`run_deno`): on this machine when None (`deno_checks`).
    pub deno: Option<std::sync::Arc<dyn DenoRunner>> }

/// What runs the compiler's Deno passes, which load the steps' modules (builder code): the type check, then, when it
/// passes, the export of the steps' output schemas. A server whose builders' code runs in sandboxes runs them there.
pub trait DenoRunner: Send + Sync {
    /// `deno_checks` for `workflow_dir` (its `.cori/` generated here).
    fn check(&self, workflow_dir: &Path, step_files: &[String]) -> Result<(TypeCheck, BTreeMap<String, serde_json::Value>), String>;
    /// `deno_checks_all` for workflows of one app (their `.cori/` generated here). A runner that cannot answers None, and
    /// the compiler runs `check` per workflow.
    fn check_all(&self, _workflows: &[(PathBuf, Vec<String>)]) -> Result<Option<Vec<BTreeMap<String, serde_json::Value>>>, String> { Ok(None) }
}

/// The Deno passes on this machine: the type check of `step_files`, and when it passes their output schemas (none when
/// it fails); Err when the schemas could not be exported. `.cori/` must be generated first (`compile`).
pub fn deno_checks(workflow_dir: &Path, step_files: &[String]) -> Result<(TypeCheck, BTreeMap<String, serde_json::Value>), String> {
    let out = check::deno_check(workflow_dir, step_files);
    let tc = TypeCheck { ok: out.ok, diagnostics: out.diagnostics };
    if !tc.ok { return Ok((tc, BTreeMap::new())); }
    let schemas = check::dump_schemas(workflow_dir, step_files).map_err(|e| format!("schema export: {e}"))?;
    Ok((tc, schemas))
}

/// The Deno passes of several workflows of one app on this machine, all at once: one type check of every step file, then,
/// when it passes, one export of the output schemas (each workflow's, in the order given). Most of a type check is
/// loading its libraries, so one for the app costs about what one workflow's does. None when the workflows do not pass
/// together, or cannot be checked together (generated with other settings): `deno_checks` per workflow then says which
/// fails and why. `.cori/` must be generated first (`compile`).
pub fn deno_checks_all(workflows: &[(PathBuf, Vec<String>)]) -> Result<Option<Vec<BTreeMap<String, serde_json::Value>>>, String> {
    let Some((first, _)) = workflows.first() else { return Ok(Some(Vec::new())) };
    // one configuration for every file: the generated ones must be the same
    let config = |d: &Path| std::fs::read_to_string(d.join(".cori").join("deno.json")).ok();
    let shared = config(first);
    if shared.is_none() || workflows.iter().any(|(d, _)| config(d) != shared) { return Ok(None); }
    // checked in one program, a file that changes what every file sees would let another workflow pass that fails alone
    if declares_for_all(workflows) { return Ok(None); }
    let files: Vec<PathBuf> = workflows.iter().flat_map(|(d, fs)| fs.iter().map(move |f| d.join("steps").join(f))).collect();
    if !check::deno_check_paths(first, first, &files).ok { return Ok(None); }
    check::dump_schemas_all(workflows).map(Some).map_err(|e| format!("schema export: {e}"))
}

/// Whether a TypeScript file of these workflows or of their app's `lib/` changes what other files see (`declare global`,
/// `declare module`, a `/// <reference …>` such as `lib="dom"`): their types then hold for every workflow checked in the
/// same program, not only for those that import the file.
fn declares_for_all(workflows: &[(PathBuf, Vec<String>)]) -> bool {
    let mut roots: Vec<PathBuf> = workflows.iter().map(|(d, _)| d.clone()).collect();
    if let Some((first, _)) = workflows.first() { roots.push(app_root_of(first).join("lib")); }
    roots.iter().any(|root| walkdir::WalkDir::new(root).into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && matches!(e.file_name().to_str(), Some(".cori" | "node_modules"))))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x, "ts" | "mts" | "cts" | "tsx")))
        .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("declare global") || t.contains("declare module") || t.contains("/// <reference"))))
}

pub fn compile(dir: &Path, opts: &Options) -> Result<Compiled, Vec<String>> {
    let s = stage(dir, opts)?;
    let deno = deno_passes(&s, opts)?;
    finish(s, deno)
}

/// A workflow compiled up to its Deno passes, its `.cori/` generated: what `finish` completes.
struct Staged { dir: PathBuf, m: manifest::Manifest, d: dag::Dag, files: Vec<String>, warnings: Vec<String> }

/// What a workflow's Deno passes found: its type check, and its steps' output schemas.
type DenoFound = (TypeCheck, BTreeMap<String, serde_json::Value>);

/// The first part of `compile`: everything before the Deno passes.
fn stage(dir: &Path, opts: &Options) -> Result<Staged, Vec<String>> {
    let dir = dir.canonicalize().map_err(|e| vec![format!("{}: {e}", dir.display())])?;
    if !dir.is_dir() { return Err(vec![format!("{}: not a folder; pass the workflow folder <app>/workflows/<id>/ (workflow.yaml, types.ts, steps/<name>.ts)", dir.display())]); }
    // 1. workflow.yaml and the step files it lists, parsed
    let (mut m, files, decls, mut warnings) = load(&dir, opts)?;
    // 2. the dataflow graph, its checks, and what the IR carries resolved (budget, native activities' declarations)
    let mut d = dag::build(&decls, &m.ordering)?;
    let errors = check(&dir, opts, &mut m, &mut d, &mut warnings);
    if !errors.is_empty() { return Err(errors); }
    // 3. the generated context the Deno passes check the steps against
    generate(&dir, opts, &m, &d, &files)?;
    Ok(Staged { dir, m, d, files, warnings })
}

/// A staged workflow's own Deno passes (`opts.deno`, else this machine's), when they run (`opts.run_deno`).
fn deno_passes(s: &Staged, opts: &Options) -> Result<Option<DenoFound>, Vec<String>> {
    if !opts.run_deno { return Ok(None); }
    match &opts.deno { Some(r) => r.check(&s.dir, &s.files), None => deno_checks(&s.dir, &s.files) }.map(Some).map_err(|e| vec![e])
}

/// The rest of `compile`, with what the Deno passes found (None when they did not run).
fn finish(s: Staged, deno: Option<DenoFound>) -> Result<Compiled, Vec<String>> {
    let Staged { dir, m, d, mut warnings, .. } = s;
    let (type_check, schemas) = match deno {
        Some((tc, _)) if !tc.ok => return Err(tc.diagnostics.iter().map(|d| format!("type check: {d}")).collect()),
        Some((tc, schemas)) => (Some(tc), schemas),
        None => (None, BTreeMap::new()),
    };
    warnings.extend(schema_warnings(&dir, &m, &d, &schemas));
    let lineage = lineage(&d);
    let content_hash = content_hash(&dir).map_err(|e| vec![e])?;
    Ok(Compiled { id: m.id.clone(), content_hash, manifest: m, dag: d, lineage, warnings, type_check, schemas })
}

/// `workflow.yaml` (its id is the folder's name) and `steps/<name>.ts`: every listed step has its file and every file is
/// listed, in the declared order. Returns the manifest, the step files, their parsed declarations and the warnings.
fn load(dir: &Path, opts: &Options) -> Result<(manifest::Manifest, Vec<String>, Vec<steps::StepDecl>, Vec<String>), Vec<String>> {
    let mut errors = Vec::new();
    let manifest_path = dir.join("workflow.yaml");
    let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| vec![format!("{}: {e} (a workflow folder is workflows/<id>/ with workflow.yaml, types.ts and steps/<name>.ts)", manifest_path.display())])?;
    let m = manifest::parse(&manifest_text).map_err(|e| vec![e])?;
    // the workflow id is the folder name
    let folder = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if m.id != folder { return Err(vec![format!("workflow.yaml: id `{}` must equal the folder name `{folder}` (workflows/<id>/)", m.id)]); }
    // step files: every listed step has its file, every file is listed; the order is the declared one
    let steps_dir = dir.join("steps");
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(&steps_dir).map_err(|e| vec![format!("steps/: {e}")])?
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|f| f.ends_with(".ts") && !f.ends_with(".d.ts")).collect();
    if m.steps.is_empty() {
        // a fresh workflow still gets its `.cori/context.ts` so the first step file's `import type { Ctx }` resolves
        let _ = write_context_stub(dir, &m, opts.sdk_path.as_deref());
        return Err(vec!["workflow.yaml: `steps:` is empty; list the top-level steps in execution order (one steps/<name>.ts each)".into()]);
    }
    let files: Vec<String> = m.steps.iter().map(|n| format!("{n}.ts")).collect();
    for (n, f) in m.steps.iter().zip(&files) { if !on_disk.contains(f) { errors.push(format!("workflow.yaml: step `{n}` is listed in `steps:` but steps/{f} is missing")); } }
    for f in &on_disk { if !files.contains(f) { let n = f.trim_end_matches(".ts"); errors.push(format!("steps/{f} is not listed in workflow.yaml `steps:` (add `{n}` at its position in the list, or delete the file)")); } }
    if !errors.is_empty() { return Err(errors); }
    let mut decls = Vec::new();
    let mut warnings = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let src = std::fs::read_to_string(steps_dir.join(f)).map_err(|e| vec![format!("steps/{f}: {e}")])?;
        match steps::parse_step_file(&format!("steps/{f}"), &src, i) {
            Ok(p) => { errors.extend(p.errors.clone()); warnings.extend(p.warnings); decls.push(p.decl); }
            Err(es) => errors.extend(es),
        }
    }
    if !errors.is_empty() { return Err(errors); }
    Ok((m, files, decls, warnings))
}

/// The checks of a workflow's graph against its manifest, its app and the registry (endpoint idempotency, durations,
/// connectors, llm outputs, store tools, result paths), and what the compiler resolves into the IR for the engine (the
/// budget's duration, a native activity's declared timeouts and retries). Returns the errors.
fn check(dir: &Path, opts: &Options, m: &mut manifest::Manifest, d: &mut dag::Dag, warnings: &mut Vec<String>) -> Vec<String> {
    let mut errors = Vec::new();
    // endpoint idempotency: unset means `none` without writes (a replayed read is stale for 24 h) and `parameters` with
    // writes, where the author is told once: a call repeating earlier parameters (a status toggled back) is replayed
    if let Some(e) = m.endpoint.as_mut() {
        if !matches!(e.idempotency.as_str(), "" | "parameters" | "header" | "none") { errors.push(format!("workflow.yaml: endpoint.idempotency `{}` must be parameters, header or none", e.idempotency)); }
        if e.idempotency.is_empty() {
            let writes = d.nodes.iter().any(|n| n.effect.access == effects::Access::MayWrite);
            e.idempotency = if writes { "parameters" } else { "none" }.into();
            if writes { warnings.push("endpoint.idempotency is not set: this workflow writes, so it defaults to `parameters` (the same caller and parameters within 24 h replays the run instead of running it; a call that repeats earlier parameters, like setting a status back, does nothing). Set `idempotency: parameters`, `header` or `none` in workflow.yaml".into()); }
        }
    }
    // durations, rates and the schedule: one grammar each (`timing`), checked now rather than misread when a run executes
    errors.extend(timing_errors(m, d));
    // the engine reads the budget's duration resolved here; versions compiled before it read `max_duration` their own way
    if let Some(b) = m.budget.as_mut().and_then(|b| b.as_object_mut())
        && let Some(ms) = b.get("max_duration").and_then(|v| v.as_str()).and_then(|s| timing::duration(s).ok())
    {
        b.insert("max_duration_ms".into(), serde_json::json!(ms.as_millis() as u64));
    }
    // a native activity's declaration (its team's activities.yaml, as registered) sets the step's timeout, attempts,
    // non-retryable errors and heartbeat, unless the step sets them itself
    for n in d.nodes.iter_mut().filter(|n| n.kind == "activity") {
        let (Some(conn), Some(act)) = (n.literals.get("connector").and_then(|v| v.as_str()).map(String::from), n.literals.get("activity").and_then(|v| v.as_str()).map(String::from)) else { continue };
        let declared = opts.connectors.iter().find(|c| c["name"].as_str() == Some(conn.as_str())).and_then(|c| c["tools"].as_array()).and_then(|ts| ts.iter().find(|t| t["name"].as_str() == Some(act.as_str())));
        if let Some(decl) = declared { warnings.extend(apply_activity_declaration(n, &conn, decl)); }
    }
    // connectors declared vs used
    let used: std::collections::BTreeSet<String> = d.nodes.iter().filter_map(|n| n.literals.get("connector").and_then(|v| v.as_str()).map(String::from)).collect();
    for c in &used { if !m.connectors.contains(c) { errors.push(format!("step uses connector `{c}` which is not listed in manifest `connectors`")); } }
    for c in &m.connectors { if !used.contains(c) { warnings.push(format!("connector `{c}` is declared but no step uses it")); } }
    for n in &d.nodes { if n.kind == "llm" && !n.literals.contains_key("has_output") { errors.push(format!("{}:{}: llm step `{}` must declare an `output` schema", n.file, n.line, n.name)); } }
    for n in &d.nodes { if n.kind != "llm" && n.literals.contains_key("has_files") { errors.push(format!("{}:{}: step `{}`: `files` is for `llm` steps (the files its model reads); a {} step reads a file's reference only", n.file, n.line, n.name, n.kind)); } }
    // store steps: `tool` against the typed tools the store's roles expose (migrations + roles/*.yaml)
    let allow_roles: Vec<String> = m.endpoint.as_ref().and_then(|e| e.allow.as_ref()).and_then(|a| a.get("roles")).and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
    let (tool_errors, tool_warnings) = stores::check_workflow_tools(&app_root_of(dir), &allow_roles, &d.nodes);
    errors.extend(tool_errors); warnings.extend(tool_warnings);
    // result templates: `{{ params.x }}` / `{{ steps.s.f }}` in the headline, `path:` in fields and sections
    errors.extend(result_paths(m).iter().filter_map(|(where_, p)| result_path_error(p, m, d).map(|e| format!("workflow.yaml: result.{where_} `{p}`: {e}"))));
    errors
}

/// Writes `.cori/context.ts` and `.cori/deno.json`, which the Deno passes check the steps against.
fn generate(dir: &Path, opts: &Options, m: &manifest::Manifest, d: &dag::Dag, files: &[String]) -> Result<(), Vec<String>> {
    let cori_dir = dir.join(".cori");
    std::fs::create_dir_all(&cori_dir).map_err(|e| vec![format!(".cori: {e}")])?;
    let sdk_path = match &opts.sdk_path { Some(p) => p.clone(), None => default_sdk_path().map_err(|e| vec![e])? };
    let indexed: Vec<(usize, String)> = files.iter().enumerate().map(|(i, f)| (i, f.clone())).collect();
    std::fs::write(cori_dir.join("context.ts"), context::context_dts(m, d, &indexed, "@cori-do/sdk", &opts.connectors)).map_err(|e| vec![e.to_string()])?;
    std::fs::write(cori_dir.join("deno.json"), context::deno_json(&sdk_path)).map_err(|e| vec![e.to_string()])?;
    Ok(())
}

/// What only the steps' output schemas tell: a result path naming a field the step's output does not have (it renders
/// empty), and the store columns tagged pii that a step exposes without `redact`.
fn schema_warnings(dir: &Path, m: &manifest::Manifest, d: &dag::Dag, schemas: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    let mut warnings = Vec::new();
    if !schemas.is_empty() {
        // a result path naming a field the step's output schema does not have renders empty at run time
        for (where_, p) in result_paths(m) {
            let mut parts = p.split('.'); if parts.next() != Some("steps") { continue; }
            let Some(step) = parts.next() else { continue };
            let Some(mut cur) = schemas.get(step).cloned() else { continue };
            for f in parts {
                if cur.get("type").and_then(|t| t.as_str()) == Some("array") { warnings.push(format!("result.{where_} `{p}`: `{f}` follows an array; templates cannot index arrays or read `.length` (compute it in a code step)")); break; }
                let Some(props) = cur.get("properties").and_then(|x| x.as_object()) else { break };
                match props.get(f) { Some(next) => cur = next.clone(), None => { warnings.push(format!("result.{where_} `{p}`: `{f}` is not a field of step `{step}`'s output (fields: {})", props.keys().cloned().collect::<Vec<_>>().join(", "))); break; } }
            }
        }
        warnings.extend(stores::check_workflow(&app_root_of(dir), &m.redact, &d.nodes, schemas));
    }
    warnings
}

/// The workflow folders of an app: `<app>/workflows/<id>/` holding a workflow.yaml, sorted.
pub fn workflow_dirs(app_root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(app_root.join("workflows")).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("workflow.yaml").exists()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// An app folder compiled: its stores (role files and rules against the migrations), each workflow of `workflows/`, and
/// what its cori.toml sets that changes nothing (`app_warnings`).
pub struct AppCompiled { pub stores: stores::Report, pub workflows: Vec<(PathBuf, Result<Compiled, Vec<String>>)>, pub warnings: Vec<String> }

/// What `cori check <app>` shows and what a deploy requires to be free of errors.
pub fn compile_app(app_root: &Path, opts: &Options) -> AppCompiled {
    let staged: Vec<(PathBuf, Result<Staged, Vec<String>>)> = workflow_dirs(app_root).into_iter().map(|d| { let s = stage(&d, opts); (d, s) }).collect();
    // the Deno passes of every workflow at once (a deploy waits on them), else one workflow at a time, which also says
    // which workflow fails and why
    let ready: Vec<(PathBuf, Vec<String>)> = staged.iter().filter_map(|(_, s)| s.as_ref().ok().map(|s| (s.dir.clone(), s.files.clone()))).collect();
    let together = if opts.run_deno && ready.len() > 1 {
        match &opts.deno { Some(r) => r.check_all(&ready), None => deno_checks_all(&ready) }.ok().flatten().filter(|s| s.len() == ready.len())
    } else { None };
    let mut together = together.map(|s| s.into_iter());
    let workflows: Vec<(PathBuf, Result<Compiled, Vec<String>>)> = staged.into_iter().map(|(d, s)| {
        let r = s.and_then(|s| {
            let deno = match together.as_mut() { Some(each) => Some((TypeCheck { ok: true, diagnostics: vec![] }, each.next().unwrap_or_default())), None => deno_passes(&s, opts)? };
            finish(s, deno)
        });
        (d, r)
    }).collect();
    let mut warnings = app_warnings(app_root);
    // the roles the app uses without declaring them: the organization's identity provider would not know them
    let compiled: Vec<&Compiled> = workflows.iter().filter_map(|(_, r)| r.as_ref().ok()).collect();
    let cori_toml = std::fs::read_to_string(app_root.join("cori.toml")).unwrap_or_default();
    let platform: Vec<String> = roles::PLATFORM_DEFAULTS.iter().map(|r| r.to_string()).collect();
    warnings.extend(roles::Roles::of(&cori_toml, &compiled, &roles::store_roles(app_root)).warnings(&platform));
    AppCompiled { stores: stores::check_app(app_root), workflows, warnings }
}

/// The `[auth]` keys of cori.toml that are read: the app's roles, and the token claims a run's user carries besides its
/// subject, roles and email.
pub const AUTH_KEYS: &[&str] = &["roles", "principal_claims"];

/// What an app's cori.toml sets that changes nothing: `[auth]` keys other than `AUTH_KEYS` (the platform's issuer signs
/// users in, and every app gets its own OIDC client from the platform).
pub fn app_warnings(app_root: &Path) -> Vec<String> {
    let Some(t) = std::fs::read_to_string(app_root.join("cori.toml")).ok().and_then(|s| toml::from_str::<toml::Value>(&s).ok()) else { return vec![] };
    let mut out = Vec::new();
    let ignored: Vec<String> = t.get("auth").and_then(|a| a.as_table()).map(|a| a.keys().filter(|k| !AUTH_KEYS.contains(&k.as_str())).map(|k| format!("`{k}`")).collect()).unwrap_or_default();
    if !ignored.is_empty() { out.push(format!("cori.toml [auth] {} ignored: the platform's issuer signs users in and gives the app its own OIDC client (only `roles` and `principal_claims` are read)", ignored.join(", "))); }
    if let Some(why) = t.get("app").and_then(|a| a.get("description")).and_then(description_problem) { out.push(format!("cori.toml [app] description: {why}")); }
    // a person holding several roles acts in a store as the first cori.toml lists
    let toml_text = std::fs::read_to_string(app_root.join("cori.toml")).unwrap_or_default();
    out.extend(stores::role_order_warnings(app_root, &roles::declared_order(&toml_text)));
    out
}

/// The longest `[app] description`: the line the apps portal shows under the app's name.
pub const DESCRIPTION_MAX: usize = 140;

/// Why an `[app] description` is not one the apps portal shows whole: one line of text, at most `DESCRIPTION_MAX` characters.
pub fn description_problem(v: &toml::Value) -> Option<String> {
    let Some(d) = v.as_str() else { return Some("a string, the line the apps portal shows under the app's name".into()) };
    if d.contains('\n') { return Some("one line: the apps portal shows its first line only".into()); }
    (d.trim().chars().count() > DESCRIPTION_MAX).then(|| format!("at most {DESCRIPTION_MAX} characters: the apps portal cuts the rest"))
}

/// The app folder of a workflow folder: the nearest ancestor holding `cori.toml`, else `<dir>/../..`.
pub fn app_root_of(workflow_dir: &Path) -> PathBuf {
    let mut cur = workflow_dir.to_path_buf();
    while cur.pop() { if cur.join("cori.toml").exists() { return cur; } }
    workflow_dir.parent().and_then(|p| p.parent()).unwrap_or(workflow_dir).to_path_buf()
}

/// Writes `.cori/context.ts` and `.cori/deno.json` for a workflow without steps (`cori workflow new`), so that the
/// first `import type { Ctx } from "../.cori/context"` resolves before the first successful `cori check`.
pub fn write_context_stub(dir: &Path, m: &manifest::Manifest, sdk_path: Option<&str>) -> Result<(), String> {
    let cori_dir = dir.join(".cori");
    std::fs::create_dir_all(&cori_dir).map_err(|e| format!(".cori: {e}"))?;
    let empty = dag::Dag { nodes: vec![], edges: vec![], schedule: vec![], predecessors: BTreeMap::new(), goto_skips: BTreeMap::new() };
    std::fs::write(cori_dir.join("context.ts"), context::context_dts(m, &empty, &[], "@cori-do/sdk", &[])).map_err(|e| e.to_string())?;
    std::fs::write(cori_dir.join("deno.json"), context::deno_json(&match sdk_path { Some(p) => p.to_string(), None => default_sdk_path()? })).map_err(|e| e.to_string())
}

/// Copies a native activity's declared `timeouts` and `retry` into its step, where the step sets none itself:
/// `timeouts.start_to_close` → `timeout_ms`, `timeouts.heartbeat` → `heartbeat_ms`, `retry.max_attempts` → `retries.max`,
/// `retry.non_retryable` → `non_retryable`. A declared duration that does not read is left out (a warning).
fn apply_activity_declaration(n: &mut dag::Node, connector: &str, decl: &serde_json::Value) -> Vec<String> {
    let mut warnings = Vec::new();
    for (key, literal) in [("start_to_close", "timeout_ms"), ("heartbeat", "heartbeat_ms")] {
        let Some(s) = decl["timeouts"][key].as_str() else { continue };
        match timing::duration(s) {
            Ok(d) => { n.literals.entry(literal.to_string()).or_insert(serde_json::json!(d.as_millis() as u64)); }
            Err(x) => warnings.push(format!("{}:{}: connector `{connector}` declares timeouts.{key} {x}; ignored", n.file, n.line)),
        }
    }
    if let Some(max) = decl["retry"]["max_attempts"].as_u64() && !n.literals.contains_key("retries") { n.literals.insert("retries".into(), serde_json::json!({ "max": max })); }
    if let Some(types) = decl["retry"]["non_retryable"].as_array() { n.literals.insert("non_retryable".into(), serde_json::json!(types)); }
    warnings
}

/// What a workflow's engine and server would misread: workflow.yaml's `endpoint.timeout`, `endpoint.rate_limit`,
/// `budget.max_duration`, `schedule` and `schedule_tz`, and the `timeout` of its human steps.
fn timing_errors(m: &manifest::Manifest, d: &dag::Dag) -> Vec<String> {
    use serde_json::Value;
    let mut errors = Vec::new();
    if let Some(e) = &m.endpoint {
        if let Some(t) = &e.timeout && let Err(x) = timing::duration(t) { errors.push(format!("workflow.yaml: endpoint.timeout {x}")); }
        if let Some(r) = &e.rate_limit && let Err(x) = timing::rate_limit(r) { errors.push(format!("workflow.yaml: endpoint.rate_limit {x}")); }
    }
    match m.budget.as_ref().and_then(|b| b.get("max_duration")) {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => { if let Err(x) = timing::duration(s) { errors.push(format!("workflow.yaml: budget.max_duration {x}")); } }
        Some(other) => errors.push(format!("workflow.yaml: budget.max_duration `{other}` is a duration such as `15m` (a whole number and a unit)")),
    }
    if let Some(expr) = &m.schedule && let Err(x) = timing::cron(expr) { errors.push(format!("workflow.yaml: schedule {x}")); }
    if let Some(tz) = &m.schedule_tz && let Err(x) = timing::time_zone(tz) { errors.push(format!("workflow.yaml: schedule_tz {x}")); }
    for n in d.nodes.iter().filter(|n| n.kind == "human") {
        match n.literals.get("timeout") {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s == "<expr>" => errors.push(format!("{}:{}: human step `{}`: timeout is a literal duration such as \"3d\", not an expression", n.file, n.line, n.name)),
            Some(Value::String(s)) => { if let Err(x) = timing::duration(s) { errors.push(format!("{}:{}: human step `{}`: timeout {x}", n.file, n.line, n.name)); } }
            Some(other) => errors.push(format!("{}:{}: human step `{}`: timeout `{other}` is a duration string such as \"3d\"", n.file, n.line, n.name)),
        }
    }
    errors
}

/// Every template path of `result:`: (`headline` | `fields[i]` | `sections[i]`, path).
fn result_paths(m: &manifest::Manifest) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(r) = &m.result else { return out };
    if let Some(h) = r.get("headline").and_then(|h| h.as_str()) {
        let mut rest = h;
        while let Some(s) = rest.find("{{") { let Some(e) = rest[s + 2..].find("}}") else { break }; out.push(("headline".to_string(), rest[s + 2..s + 2 + e].trim().to_string())); rest = &rest[s + 2 + e + 2..]; }
    }
    for key in ["fields", "sections"] {
        for (i, f) in r.get(key).and_then(|f| f.as_array()).cloned().unwrap_or_default().iter().enumerate() {
            if let Some(p) = f.get("path").and_then(|p| p.as_str()) { out.push((format!("{key}[{i}]"), p.trim().to_string())); }
        }
    }
    out
}

/// Static rules of a result path: dotted `params.<declared>` or `steps.<step>.<field>…`, nothing else.
fn result_path_error(p: &str, m: &manifest::Manifest, d: &dag::Dag) -> Option<String> {
    if p.contains('|') { return Some("filters are not supported; compute the value in a code step".into()); }
    if p.contains('[') { return Some("array indexing is not supported; compute the value in a code step".into()); }
    let mut parts = p.split('.');
    match parts.next() {
        Some("params") => { let n = parts.next().unwrap_or(""); if !m.parameters.iter().any(|x| x.name == n) { return Some(format!("`{n}` is not a declared parameter ({})", m.parameters.iter().map(|x| x.name.clone()).collect::<Vec<_>>().join(", "))); } }
        Some("steps") => { let n = parts.next().unwrap_or(""); if !d.nodes.iter().any(|x| x.name == n) { return Some(format!("`{n}` is not a step of this workflow ({})", d.nodes.iter().map(|x| x.name.clone()).collect::<Vec<_>>().join(", "))); } if parts.next().is_none() { return Some("name a field: steps.<step>.<field>".into()); } }
        _ => return Some("paths start with `params.` or `steps.`".into()),
    }
    None
}

/// The SDK entry point the generated `deno.json` maps `@cori-do/sdk` to (the embedded copy, see `runtime`).
fn default_sdk_path() -> Result<String, String> {
    runtime::runtime().map(|r| r.sdk_index.display().to_string())
}

/// For every external step, the context fields it consumes, following pure/code steps back to their sources.
fn lineage(d: &dag::Dag) -> BTreeMap<String, Vec<String>> {
    let by_name: BTreeMap<&str, &dag::Node> = d.nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    fn sources(name: &str, by_name: &BTreeMap<&str, &dag::Node>, depth: usize) -> Vec<String> {
        let n = by_name[name]; let mut out = Vec::new();
        for r in &n.refs {
            if let Some(rest) = r.strip_prefix("steps.") {
                let up = rest.split(['.', '[']).next().unwrap_or(rest);
                if let Some(u) = by_name.get(up) { if u.kind == "code" && depth < 8 { out.extend(sources(up, by_name, depth + 1)); continue; } }
            }
            out.push(r.clone());
        }
        out.sort(); out.dedup(); out
    }
    d.nodes.iter().filter(|n| n.effect.external).map(|n| (n.name.clone(), sources(&n.name, &by_name, 0))).collect()
}

/// The workflow's content, plus every file its TypeScript imports from elsewhere in the app (`lib/`, another
/// workflow's types): a change to shared code changes the hash of every workflow using it.
pub fn content_hash(dir: &Path) -> Result<String, String> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ts_files: Vec<PathBuf> = Vec::new();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !e.file_type().is_file() { continue; }
        let rel = e.path().strip_prefix(dir).unwrap().to_string_lossy().to_string();
        if rel.starts_with(".cori/") || rel.contains("node_modules/") { continue; }
        if rel.ends_with(".ts") { ts_files.push(e.path().to_path_buf()); }
        entries.push((rel, std::fs::read(e.path()).map_err(|x| x.to_string())?));
    }
    let app_root = app_root_of(dir).canonicalize().unwrap_or_else(|_| app_root_of(dir));
    let dir_c = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut seen: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    while let Some(f) = ts_files.pop() {
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        for spec in relative_imports(&src) {
            let mut target = f.parent().unwrap_or(dir).join(&spec);
            if !target.exists() && target.extension().is_none() { target.set_extension("ts"); }
            let Ok(t) = target.canonicalize() else { continue };
            if t.starts_with(&dir_c) || !t.starts_with(&app_root) || !seen.insert(t.clone()) { continue; }
            let rel = format!("@app/{}", t.strip_prefix(&app_root).unwrap().to_string_lossy());
            entries.push((rel, std::fs::read(&t).map_err(|x| x.to_string())?));
            ts_files.push(t);
        }
    }
    entries.sort();
    let mut h = Sha256::new();
    for (p, b) in entries { h.update(p.as_bytes()); h.update([0]); h.update(&b); h.update([0]); }
    Ok(format!("sha256:{}", hex::encode(h.finalize())))
}

/// `./x`, `../lib/y.ts` specifiers of `import … from "…"`, `export … from "…"`, `import("…")` in a TypeScript source.
fn relative_imports(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["from \"", "from '", "import(\"", "import('", "import \"", "import '"] {
        let mut rest = src;
        while let Some(i) = rest.find(marker) {
            let after = &rest[i + marker.len()..];
            let q = marker.chars().last().unwrap();
            if let Some(e) = after.find(q) { let spec = &after[..e]; if spec.starts_with("./") || spec.starts_with("../") { out.push(spec.to_string()); } }
            rest = after;
        }
    }
    out.sort(); out.dedup(); out
}

/// Control-flow steps (`branch`, `switch`, `map`, `loop`, `try`, `wait`): their effect is the union of their nested steps,
/// so effect summaries (check report, change review) skip them to avoid counting a nested write twice.
pub fn is_container_kind(kind: &str) -> bool { matches!(kind, "branch" | "switch" | "map" | "loop" | "try" | "wait") }

/// Human report in the shape of `cori check`.
pub fn render(c: &Compiled) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(s, "workflow      {} ({})", c.id, &c.content_hash[..19]).unwrap();
    let files = c.dag.nodes.iter().filter(|n| n.nested_in.is_none()).count();
    writeln!(s, "context.ts    generated ({} steps, {} nested)", files, c.dag.nodes.len() - files).unwrap();
    match &c.type_check { Some(t) if t.ok => writeln!(s, "deno check    ok").unwrap(), Some(_) => writeln!(s, "deno check    FAILED").unwrap(), None => writeln!(s, "deno check    skipped").unwrap() }
    let sched: Vec<String> = c.dag.schedule.iter().map(|l| if l.len() == 1 { l[0].clone() } else { format!("{{{}}}", l.join(" ∥ ")) }).collect();
    writeln!(s, "dataflow      {}", sched.join(" → ")).unwrap();
    let mut eff: Vec<String> = c.dag.nodes.iter().filter(|n| !is_container_kind(&n.kind)).filter(|n| n.effect.external).map(|n| format!("{}:{}", n.effect.target, match n.effect.access { effects::Access::None => "none", effects::Access::Read => "Read", effects::Access::Prompt => "Prompt", effects::Access::MayWrite => "MayWrite" })).collect();
    eff.dedup();
    writeln!(s, "effects       {}", eff.join("  ")).unwrap();
    for (step, fields) in &c.lineage { if !fields.is_empty() { writeln!(s, "lineage       {} ← {}", step, fields.join(", ")).unwrap(); } }
    if let Some(e) = &c.manifest.endpoint { writeln!(s, "endpoint      {} · {} · idempotency {} · allow {}", e.auth, e.mode, e.idempotency, e.allow.as_ref().map(|v| v.to_string()).unwrap_or("app roles".into())).unwrap(); }
    if let Some(sch) = &c.manifest.schedule { writeln!(s, "schedule      {} {}", sch, c.manifest.schedule_tz.clone().unwrap_or_default()).unwrap(); }
    writeln!(s, "warnings      {}", c.warnings.len()).unwrap();
    for w in &c.warnings { writeln!(s, "  - {w}").unwrap(); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_description_is_one_short_line() {
        let v = |s: &str| toml::Value::String(s.into());
        assert!(description_problem(&v("Submit and decide team requests")).is_none());
        assert!(description_problem(&v(&"a".repeat(141))).is_some());
        assert!(description_problem(&v("two\nlines")).is_some());
        assert!(description_problem(&toml::Value::Integer(1)).is_some());
    }
}

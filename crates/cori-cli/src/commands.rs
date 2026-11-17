// SPDX-License-Identifier: Apache-2.0
//! The authoring and transport commands: `workflow new`, `scaffold`, `apps`, `deploy` (tar.zst bundle + git
//! metadata), `pull`, `login` (PKCE, device grant, or a given token) and the app registration `init` does.
use crate::client::Api;
use crate::session::{self, Session};
use anyhow::{Context, Result, anyhow, bail};
use cori_compiler::authoring;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn base(server: &str) -> String { server.trim_end_matches('/').to_string() }

/// One error message for a failed server call; 401 explains how to get a new session.
pub fn server_error(status: reqwest::StatusCode, body: &Value, app_root: Option<&Path>, server: &str) -> anyhow::Error {
    let msg = body.get("error").and_then(|e| e.as_str()).map(String::from).unwrap_or_else(|| body.to_string());
    // the app folder the command runs in, when the caller named none
    let found = if app_root.is_none() { session::discover_app_root() } else { None };
    let app_root = app_root.or(found.as_deref());
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let kind = app_root.and_then(session::load).filter(|s| !s.token.is_empty()).map(|s| s.kind).unwrap_or_else(|| "human".into());
        return anyhow!("{status}: {msg}\n{}", session::sign_in_hint(app_root, &kind, server));
    }
    // a folder of one organization, a token of another (the server names it): a login to the folder's
    if status == reqwest::StatusCode::CONFLICT && msg.contains("another organization") {
        if let Some(o) = app_root.and_then(session::load).and_then(|s| s.organization) { return anyhow!("{status}: {msg}\nthis folder belongs to {o}: run `cori login {server} --org {o}`"); }
    }
    anyhow!("{status}: {msg}")
}

// ---------------------------------------------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------------------------------------------

/// The registry rows the compiler and the scaffold need (`GET /v1/connectors`); an error names the fix.
pub async fn registry_rows(server: &str, token: &str, app_root: Option<&Path>) -> Result<Vec<Value>> {
    let v = Api::new(server, token).for_app(app_root).get("/v1/connectors").await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// Registry rows plus the app's own `cori.toml` connectors (overrides and mocks), marked by `source`. A store connector's
/// tools come from the folder (`stores/<ref>/` migrations and roles), with no server: `getItem`, `listItems`, …
pub fn with_app_connectors(mut rows: Vec<Value>, app_root: Option<&Path>) -> Vec<Value> {
    for r in rows.iter_mut() { r["source"] = json!("registry"); }
    if let Some(app) = app_root { if let Ok(cfg) = cori_engine::config::AppConfig::load(app) { for (name, c) in cfg.connectors {
        let has_mock = c.dev.as_ref().map(|d| d.mock.is_some()).unwrap_or(false);
        let store_tools = if c.kind == "store" { cori_compiler::stores::connector_tools(app, &name) } else { None };
        if let Some(existing) = rows.iter_mut().find(|x| x["name"].as_str() == Some(name.as_str())) { existing["source"] = json!("registry+app"); existing["dev_mock"] = json!(has_mock); if let Some(t) = store_tools { existing["tools"] = json!(t); } continue; }
        rows.push(json!({ "name": name, "kind": c.kind, "source": "app", "transport": { "url": c.url, "command": c.command, "base_url": c.base_url, "task_queue": c.task_queue }, "audience": c.audience, "secret": c.secret, "worker": c.worker, "dev_mock": has_mock, "tools": store_tools.unwrap_or_default() }));
    } } }
    rows
}

// ---------------------------------------------------------------------------------------------------------------
// workflow new · scaffold
// ---------------------------------------------------------------------------------------------------------------

pub struct NewWorkflow { pub dir: PathBuf, pub id: String, pub name: Option<String>, pub description: Option<String>, pub params: Vec<String>, pub connectors: Vec<String>, pub endpoint: Option<String>, pub roles: Vec<String>, pub schedule: Option<String>, pub schedule_tz: String }

pub fn workflow_new(n: &NewWorkflow) -> Result<String> {
    let app = session::app_root_of(&n.dir).ok_or_else(|| anyhow!("{} is not inside a Cori app (no cori.toml found upward)", n.dir.display()))?;
    let wdir = app.join("workflows").join(&n.id);
    if wdir.join("workflow.yaml").exists() { bail!("workflow `{}` already exists in {}", n.id, wdir.display()); }
    let params = n.params.iter().map(|p| authoring::Param::parse(p)).collect::<Result<Vec<_>>>()?;
    let endpoint = match n.endpoint.as_deref() { Some(m @ ("sync" | "async")) => Some(authoring::Endpoint { mode: m.into(), roles: n.roles.clone() }), Some(other) => bail!("--endpoint must be sync or async (got `{other}`)"), None if n.schedule.is_none() => Some(authoring::Endpoint { mode: "sync".into(), roles: n.roles.clone() }), None => None };
    let name = n.name.clone().unwrap_or_else(|| n.id.clone());
    let yaml = authoring::workflow_yaml(&n.id, &name, n.description.as_deref().unwrap_or(""), &params, &n.connectors, endpoint.as_ref(), n.schedule.as_deref().map(|s| (s, n.schedule_tz.as_str())))?;
    std::fs::create_dir_all(wdir.join("steps"))?;
    std::fs::write(wdir.join("workflow.yaml"), &yaml)?;
    std::fs::write(wdir.join("README.md"), format!("# {name}\n\n## Goal\n{}\n\n## Verification\n- \n", n.description.as_deref().unwrap_or("")))?;
    std::fs::write(wdir.join("types.ts"), "import { z } from \"zod\";\n\n// shared zod schemas of this workflow\n")?;
    // `.cori/context.ts` (Params, an empty Ctx) so the first step file's import resolves before the first `cori check`
    if let Ok(m) = cori_compiler::manifest::parse(&yaml) { if let Err(e) = cori_compiler::write_context_stub(&wdir, &m, None) { eprintln!("note: {e}"); } }
    Ok(format!("created workflows/{}/ (workflow.yaml with `steps: []`, README.md, types.ts, steps/, .cori/context.ts)\nnext: `cori scaffold <connector> <tool> --workflow {}` per connector step, or write steps/<name>.ts and list it in `steps:`; then `cori check workflows/{}`", n.id, n.id, n.id))
}

/// `cori store new <name>`: `stores/<name>/` with rules.yaml, a role file per app role, a first migration, the dev
/// fixture and the `[connectors.<name>]` binding in cori.toml. Nothing is overwritten.
pub fn store_new(dir: &Path, name: &str, roles: &[String], tenant: Option<&str>) -> Result<String> {
    if let Some(t) = tenant { if t.is_empty() || !t.chars().next().unwrap().is_ascii_lowercase() || !t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') || matches!(t, "id" | "title" | "status" | "created_at") { bail!("--tenant must be a new snake_case column name, e.g. `owner` (got `{t}`)"); } }
    if name.is_empty() || !name.chars().next().unwrap().is_ascii_lowercase() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') { bail!("store name must be snake_case (got `{name}`)"); }
    let app = session::app_root_of(dir).ok_or_else(|| anyhow!("{} is not inside a Cori app (no cori.toml found upward)", dir.display()))?;
    let sdir = app.join("stores").join(name);
    if sdir.join("rules.yaml").exists() { bail!("stores/{name}/ already exists"); }
    let toml_path = app.join("cori.toml");
    let toml_text = std::fs::read_to_string(&toml_path)?;
    let roles: Vec<String> = if roles.is_empty() {
        // `[auth].roles` (builtin auth), else `[front].allow.roles` (external auth names roles only there), else `app_user`
        // (a neutral name: "member" also means the app's owner/builders in server messages)
        let t = toml::from_str::<toml::Value>(&toml_text).ok();
        t.as_ref().and_then(|t| t.get("auth")?.get("roles")?.as_table().map(|m| m.keys().cloned().collect::<Vec<_>>())).filter(|v| !v.is_empty())
            .or_else(|| t.as_ref().and_then(|t| t.get("front")?.get("allow")?.get("roles")?.as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect::<Vec<_>>())).filter(|v| !v.is_empty()))
            .unwrap_or_else(|| vec!["app_user".into()])
    } else { roles.to_vec() };
    std::fs::create_dir_all(sdir.join("roles"))?; std::fs::create_dir_all(sdir.join("migrations"))?;
    std::fs::write(sdir.join("migrations/0001_init.sql"), format!("-- Tables of the `{name}` store: plain Postgres DDL, applied in file order to a version's sandbox at `cori deploy`, to the live database at `cori promote`, to the local database of `cori run`.\nCREATE TABLE items (\n  id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,   -- createItem returns it; callers never invent ids\n  title TEXT NOT NULL,\n  status TEXT NOT NULL DEFAULT 'open',\n{tenant_col}  owner_email TEXT,\n  created_at TIMESTAMPTZ NOT NULL DEFAULT now()\n);\n", tenant_col = tenant.map(|t| format!("  {t} TEXT NOT NULL,   -- the caller's user id, set by createItem (`tenant: {t}` in rules.yaml)\n")).unwrap_or_default()))?;
    // seeds: sandboxes and local runs start with these rows; the live database never runs them
    std::fs::create_dir_all(sdir.join("seed"))?;
    std::fs::write(sdir.join("seed/0001_demo.sql"), format!("-- Demo rows for the `{name}` store: applied once, after the migrations, when a version's sandbox or the `cori run` database is\n-- created (`cori deploy`, `cori reseed`). The live database (`cori promote`) never runs seed files.\n{}", match tenant {
        // a tenant table only shows a row to the user whose sub is in the column: seed for the identities you test with
        Some(t) => format!("-- `{t}` is the user id (sub) that sees the row: `cori run --user alice:<role>` and the bearer `dev:alice:<roles>` under `cori up` are `alice`.\nINSERT INTO items (id, title, status, {t}) VALUES ('I-1', 'First item', 'open', 'alice'), ('I-2', 'Second item', 'done', 'alice');\n"),
        None => "INSERT INTO items (id, title, status) VALUES ('I-1', 'First item', 'open'), ('I-2', 'Second item', 'done');\n".into(),
    }))?;
    std::fs::write(sdir.join("rules.yaml"), format!("# Column rules shared by every role: allowed values, patterns, tags. `pii` tags are reported by `cori check`\n# when a step exposes the column without a `redact:` entry.\nversion: \"1.0.0\"\ntables:\n  items:\n{}    columns:\n      status: {{ allowed_values: [open, done] }}\n      owner_email: {{ type: email, tags: [pii] }}\n", match tenant {
        Some(t) => format!("    tenant: {t}          # rows belong to the caller: list/get/update/delete only see their own rows\n"),
        None => "    global: true         # shared: every role that can read the table sees every row. Rows per user (a todo list): `tenant: <column>` instead (`cori store new --tenant owner`)\n".into(),
    }))?;
    for r in &roles {
        std::fs::write(sdir.join("roles").join(format!("{r}.yaml")), format!("name: {r}\ndescription: What `{r}` may read and write in `{name}` (columns must exist in the migrations; `cori check .` verifies)\ntables:\n  items:\n    readable: [id, title, status, created_at]      # leave pii columns out unless the workflow redacts them\n    creatable:\n      title: {{ required: true }}\n      status: {{ restrict_to: [open] }}\n    updatable:\n      status: {{}}\n    deletable: true                                # or {{ requires_approval: true }}; remove to forbid deleteItem\n"))?;
    }
    let fixture = app.join("fixtures").join(format!("{name}.json"));
    std::fs::create_dir_all(app.join("fixtures"))?;
    if !fixture.exists() { std::fs::write(&fixture, "{\n  \"tools\": {},\n  \"query\": { \"*\": [] }\n}\n")?; }
    let mut notes = vec![format!("created stores/{name}/ (rules.yaml, roles/{{{}}}.yaml, migrations/0001_init.sql, seed/0001_demo.sql) and fixtures/{name}.json", roles.join(","))];
    if toml_text.contains(&format!("[connectors.{name}]")) { notes.push(format!("cori.toml already declares [connectors.{name}]")); }
    else {
        std::fs::write(&toml_path, format!("{}\n[connectors.{name}]\nkind = \"store\"\nref = \"{name}\"                         # stores/{name}/\n[connectors.{name}.dev]\nmock = \"fixtures/{name}.json\"        # `cori run` without a Postgres; `cori up` uses the real store\n", toml_text.trim_end()))?;
        notes.push(format!("cori.toml: added [connectors.{name}] kind = \"store\", ref = \"{name}\" (steps use `connector: \"{name}\"`, workflow.yaml lists it in `connectors:`)"));
    }
    notes.push(format!("next: edit the migration and the role files, then `cori check .`; `cori store tools {name}` lists the tools per role (getItem, listItems, …), `cori scaffold {name} <tool> --workflow <id>` writes a step"));
    Ok(notes.join("\n"))
}

pub struct ScaffoldArgs { pub dir: PathBuf, pub connector: String, pub tool: String, pub workflow: String, pub name: Option<String>, pub after: Option<String>, pub force: bool, pub print: bool }

/// Writes `workflows/<wf>/steps/<name>.ts` from the registry and puts the name into `steps:`; the generated content
/// never has to pass through the agent.
pub async fn scaffold(a: &ScaffoldArgs, server: &str, token: &str) -> Result<String> {
    let app = session::app_root_of(&a.dir).ok_or_else(|| anyhow!("{} is not inside a Cori app (no cori.toml found upward)", a.dir.display()))?;
    // an app store connector needs no registry (its tools come from the folder): scaffold it with the server down
    let app_store = cori_compiler::stores::bindings(&app).contains_key(&a.connector);
    let registry = match registry_rows(server, token, Some(&app)).await { Ok(r) => r, Err(_) if app_store => Vec::new(), Err(e) => return Err(e) };
    let rows = with_app_connectors(registry, Some(&app));
    let s = authoring::scaffold(&rows, &a.connector, &a.tool, a.name.as_deref())?;
    if a.print { return Ok(s.file); }
    let wf = a.workflow.trim_start_matches("workflows/").trim_end_matches('/');
    let wdir = app.join("workflows").join(wf);
    let yaml_path = wdir.join("workflow.yaml");
    if !yaml_path.exists() { bail!("{} has no workflow.yaml (`cori workflow new {wf}` first)", wdir.display()); }
    let file = wdir.join("steps").join(format!("{}.ts", s.step));
    if file.exists() && !a.force { bail!("{} exists; pass --force to overwrite, or --name <other>", file.display()); }
    let (yaml, steps) = authoring::insert_step(&std::fs::read_to_string(&yaml_path)?, &s.step, a.after.as_deref())?;
    std::fs::create_dir_all(wdir.join("steps"))?;
    std::fs::write(&file, &s.file)?;
    std::fs::write(&yaml_path, &yaml)?;
    if !wdir.join(".cori/context.ts").exists() { if let Ok(m) = cori_compiler::manifest::parse(&yaml) { let _ = cori_compiler::write_context_stub(&wdir, &m, None); } }
    let mut notes = vec![format!("wrote workflows/{wf}/steps/{}.ts ({} step, effect {}) · steps: [{}]", s.step, s.kind, s.effect, steps.join(", "))];
    let yaml_text = std::fs::read_to_string(&yaml_path)?;
    if !yaml_text.lines().any(|l| l.starts_with("connectors:") && l.contains(&a.connector)) { notes.push(format!("add `{}` to `connectors:` in workflows/{wf}/workflow.yaml", a.connector)); }
    notes.push("bind `input` from ctx.params / ctx.steps, then `cori check`".into());
    Ok(notes.join("\n"))
}

// ---------------------------------------------------------------------------------------------------------------
// apps
// ---------------------------------------------------------------------------------------------------------------

/// Whether a Cori server answers `GET /healthz` at `server` within 2 s.
pub async fn server_answers(server: &str) -> bool {
    let Ok(c) = cori_engine::http::builder().timeout(std::time::Duration::from_secs(2)).build() else { return false };
    matches!(c.get(format!("{}/healthz", base(server))).send().await, Ok(r) if r.status().is_success())
}

pub async fn apps_list(server: &str, token: &str) -> Result<Value> { Api::new(server, token).get("/v1/apps").await }
pub async fn apps_get(server: &str, token: &str, app: &str) -> Result<Value> { Api::new(server, token).get(&format!("/v1/apps/{app}")).await }

/// Reserves an app id on the server (the caller becomes its owner). 409 when it exists is reported, not fatal.
pub async fn apps_create(server: &str, token: &str, app: &str, name: Option<&str>, roles: &[String], builders: &[String], template: Option<&str>) -> Result<Value> {
    let api = Api::new(server, token);
    let (status, v) = api.call(api.request(reqwest::Method::POST, "/v1/apps").json(&json!({ "id": app, "name": name.unwrap_or(app), "roles": roles, "builders": builders, "template": template }))).await?;
    if status == reqwest::StatusCode::CONFLICT { return Ok(json!({ "id": app, "existed": true, "detail": v["error"] })); }
    if !status.is_success() { return Err(api.error(status, &v)); }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------------------------
// bundles: deploy · pull
// ---------------------------------------------------------------------------------------------------------------

pub struct Bundle { pub bytes: Vec<u8>, pub hash: String, pub files: usize, pub app: String, pub credentials: Vec<String> }

/// The app folder as a `tar.zst`: the server's own view of a bundle (`cori_server::bundles`: everything but `.cori/`,
/// `.git/`, `node_modules/`, `target/`, `.DS_Store` and credential files), hashed and archived the same way, so the
/// server finds the hash the cli announces.
pub fn bundle(dir: &Path) -> Result<Bundle> {
    if !dir.join("cori.toml").exists() { bail!("{} has no cori.toml: pass the app folder (the repository root holding cori.toml and workflows/)", dir.display()); }
    if !dir.join("workflows").is_dir() { bail!("{} has no workflows/ folder (a workflow is workflows/<id>/workflow.yaml)", dir.display()); }
    let (files, credentials) = cori_server::bundles::collect(dir)?;
    let cori_toml = String::from_utf8_lossy(files.get("cori.toml").unwrap()).to_string();
    let app = toml::from_str::<toml::Value>(&cori_toml).ok().and_then(|t| t.get("app")?.get("id")?.as_str().map(String::from)).ok_or_else(|| anyhow!("cori.toml must declare [app] id"))?;
    let hash = cori_server::bundles::manifest_hash(&files);
    let bytes = cori_server::bundles::archive_from_files(&files)?;
    Ok(Bundle { bytes, hash, files: files.len(), app, credentials })
}

/// Extracts a `tar.zst` bundle into `dir`, refusing paths that escape it.
pub fn extract(bytes: &[u8], dir: &Path) -> Result<usize> { cori_server::bundles::extract(bytes, dir) }

#[derive(Debug, Default, Clone)]
pub struct GitMeta { pub commit: Option<String>, pub branch: Option<String>, pub dirty: Option<bool>, pub subject: Option<String> }

pub fn git_meta(dir: &Path) -> GitMeta {
    let run = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let Some(commit) = run(&["rev-parse", "HEAD"]) else { return GitMeta::default() };
    GitMeta { commit: Some(commit), branch: run(&["rev-parse", "--abbrev-ref", "HEAD"]), dirty: run(&["status", "--porcelain"]).map(|s| !s.is_empty()), subject: run(&["log", "-1", "--format=%s"]).filter(|s| !s.is_empty()) }
}

/// `git init` + a first commit when `dir` is not already inside a repository; silent when git is missing.
pub fn git_init(dir: &Path, message: &str) -> Option<String> {
    let inside = std::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--is-inside-work-tree"]).output().ok().map(|o| o.status.success()).unwrap_or(false);
    if inside { return None; }
    let ok = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok().map(|o| o.status.success()).unwrap_or(false);
    if !ok(&["init", "-q"]) { return None; }
    if !ok(&["add", "-A"]) { return None; }
    let _ = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=cori", "-c", "user.email=cori@local", "commit", "-q", "-m", message]).output();
    Some("git repository initialized".into())
}

/// The app id declared by `<dir>/cori.toml`, when there is one.
pub fn app_id_of(dir: &Path) -> Option<String> { std::fs::read_to_string(dir.join("cori.toml")).ok().and_then(|t| toml::from_str::<toml::Value>(&t).ok()).and_then(|t| t.get("app")?.get("id")?.as_str().map(String::from)) }

/// The path of a store's MCP endpoint: the live database, or a version's sandbox when `version` is given. The app defaults
/// to the `cori.toml` of the current folder.
pub fn store_path(app: Option<String>, version: Option<String>, store: &str) -> Result<String> {
    let app = app.or_else(|| app_id_of(Path::new("."))).ok_or_else(|| anyhow!("--app is required outside an app folder (no ./cori.toml)"))?;
    Ok(match version { Some(v) => format!("/v1/apps/{app}/versions/{v}/stores/{store}/mcp"), None => format!("/v1/apps/{app}/stores/{store}/mcp") })
}

/// A front with a build step (`front/package.json`) whose `dist/` is missing or older than its sources: the version
/// would ship a stale front. `None` when there is nothing to say.
pub fn front_build_warning(dir: &Path) -> Option<String> {
    let front = dir.join("front");
    if !front.join("package.json").is_file() { return None; }
    let newest = |root: &Path| walkdir::WalkDir::new(root).into_iter().filter_entry(|e| !(e.file_type().is_dir() && e.file_name().to_str().map(cori_server::bundles::skip_dir).unwrap_or(false))).filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).filter_map(|e| e.metadata().ok()?.modified().ok()).max();
    let built = std::fs::metadata(dir.join("dist/index.html")).ok().and_then(|m| m.modified().ok());
    let sources = [front.join("src"), front.join("index.html"), front.join("package.json"), front.join("vite.config.ts")].iter().filter(|p| p.exists()).filter_map(|p| newest(p)).max();
    let workflows = newest(&dir.join("workflows"));
    match built {
        None => Some("front/ has no build in dist/: run `npm install && npm run build` in front/ before deploying".into()),
        Some(b) if sources.map(|s| s > b).unwrap_or(false) => Some("front/ changed since dist/ was built: run `npm run build` in front/ before deploying, or this version ships the old front".into()),
        Some(b) if workflows.map(|w| w > b).unwrap_or(false) => Some("workflows/ changed since dist/ was built: `npm run build` in front/ regenerates the typed client and type-checks the front against them".into()),
        _ => None,
    }
}

/// Uploads the app folder as a release with its own sandbox stores (a human or agent session; membership required). The
/// release message is `message`, else the last commit's subject.
pub async fn publish_bundle(dir: &Path, server: &str, token: &str, require_clean: bool, message: Option<&str>) -> Result<Value> {
    let b = bundle(dir)?;
    if !b.credentials.is_empty() { eprintln!("not deployed (credentials stay out of bundles): {}", b.credentials.join(", ")); }
    let g = git_meta(dir);
    if require_clean && g.dirty == Some(true) { bail!("{} has uncommitted changes and --require-clean was given", dir.display()); }
    let msg = message.map(String::from).or_else(|| g.subject.clone());
    let api = Api::new(server, token).for_app(Some(dir));
    let mut req = api.request(reqwest::Method::POST, &format!("/v1/apps/{}/versions", b.app)).query(&[("message", msg.unwrap_or_default())]).header("content-type", "application/zstd").header("x-cori-manifest-hash", &b.hash);
    if let Some(c) = &g.commit { req = req.header("x-cori-git-commit", c); }
    if let Some(br) = &g.branch { req = req.header("x-cori-git-branch", br); }
    if let Some(d) = g.dirty { req = req.header("x-cori-git-dirty", if d { "true" } else { "false" }); }
    let (started, started_at) = (std::time::Instant::now(), chrono::Utc::now());
    let cut = |why: &str| format!("the connection was cut after {:.1} s ({why}): something between you and the server limits how long a request may last. The server finishes the deploy on its own; waiting for the release…", started.elapsed().as_secs_f32());
    let mut body = match req.body(b.bytes).send().await {
        Ok(r) => {
            crate::client::note_cli_min(r.headers());
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            let v = crate::client::parse(&text);
            if status.is_success() { v }
            else if cut_by_proxy(status, &v) { wait_for_release(&api, &b.app, &b.hash, &cut(status.as_str()), started_at).await? }
            else if status == reqwest::StatusCode::CONFLICT && v["error"].as_str().is_some_and(|e| e.contains("is being deployed already")) {
                wait_for_release(&api, &b.app, &b.hash, "this same content is being deployed already, by another call (a deploy cut and started again?): waiting for the release it records…", started_at).await?
            }
            else { return Err(api.error(status, &v)); }
        }
        // the answer never came (a timeout, a connection closed on the way): the upload may have reached the server
        Err(e) if !e.is_connect() && !e.is_builder() => wait_for_release(&api, &b.app, &b.hash, &cut(&e.to_string()), started_at).await?,
        Err(e) => bail!("{server} is not reachable: {e}"),
    };
    body["app_id"] = json!(b.app); body["files"] = json!(b.files); body["git"] = json!({ "commit": g.commit, "branch": g.branch, "dirty": g.dirty });
    Ok(body)
}

/// An answer that is a proxy's, not the server's: a gateway timeout (504, Cloudflare's 524) without the server's JSON. The
/// proxy gave up waiting while the server was still deploying.
fn cut_by_proxy(status: reqwest::StatusCode, body: &Value) -> bool {
    matches!(status.as_u16(), 504 | 524) && body.get("error").is_none()
}

/// When a deploy's own answer does not come (its connection cut, or another call deploying the same content): the server
/// finishes the deploy on its own, and its release appears in the app's listing. Waits for it (10 minutes at most) and
/// answers as the deploy would have, in short. `started_at`: when this deploy began, to tell a release recorded before it.
async fn wait_for_release(api: &Api, app: &str, hash: &str, note: &str, started_at: chrono::DateTime<chrono::Utc>) -> Result<Value> {
    eprintln!("{note}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Ok(list) = api.get(&format!("/v1/apps/{app}/versions")).await
            && let Some(x) = list.as_array().and_then(|a| a.iter().find(|x| x["hash"].as_str() == Some(hash))) {
            let release = x["release"].as_str().unwrap_or("").to_string();
            // recorded before this deploy began (with room for clocks that disagree): the same content, deployed earlier
            let before = x["created_at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t.with_timezone(&chrono::Utc) < started_at - chrono::Duration::seconds(30));
            let (adds_writes, needs_approval) = (x["adds_writes"].as_bool().unwrap_or(false), x["needs_approval"].as_bool().unwrap_or(false));
            let warning = if before { format!("this content was deployed before, as {release}: this summary is that release's record (`cori releases`)") } else { "this summary is the release's record, without the deploy's warnings and sandbox report (`cori releases`)".to_string() };
            return Ok(json!({ "app": app, "hash": hash, "short": x["short"], "seq": x["seq"], "release": release, "status": x["status"], "redeployed": before, "warnings": [warning],
                "preview_url": x["preview_url"], "review": { "delta": x["delta"], "needs_approval": adds_writes }, "needs_approval": needs_approval, "approved_by": x["approved_by"],
                // a version needs an approver when it adds writes or its app is protected
                "protected": needs_approval && !adds_writes, "promote": format!("cori promote {app} {release}"), "workflows": [] }));
        }
        if std::time::Instant::now() > deadline {
            bail!("no release of this folder (version {}) appeared within 10 minutes: the upload may not have reached the server, or its deploy failed (in its checks, most often: `cori check .` here shows them). Deploy again", hash.trim_start_matches("sha256:").get(..12).unwrap_or(hash));
        }
    }
}

/// `cori pull <app> [dir]`: the promoted version (or `--hash`) into a folder, as a git repository, with the session.
pub async fn pull(app: &str, dir: Option<PathBuf>, hash: Option<&str>, force: bool, server: &str, token: &str, source: &session::Source) -> Result<String> {
    let dir = dir.unwrap_or_else(|| PathBuf::from(app));
    if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() && !force { bail!("{} is not empty; pass --force to overwrite its files", dir.display()); }
    let which = hash.map(|h| if h.starts_with("sha256:") { h.to_string() } else { format!("sha256:{h}") }).unwrap_or_else(|| "promoted".into());
    let api = Api::new(server, token);
    let r = api.request(reqwest::Method::GET, &format!("/v1/apps/{app}/versions/{which}/bundle")).send().await.with_context(|| format!("{server} is not reachable"))?;
    let status = r.status();
    if !status.is_success() { let v: Value = r.json().await.unwrap_or(Value::Null); return Err(api.error(status, &v)); }
    let resolved = r.headers().get("x-cori-version").and_then(|h| h.to_str().ok()).map(String::from).unwrap_or(which.clone());
    let bytes = r.bytes().await?;
    std::fs::create_dir_all(&dir)?;
    let n = extract(&bytes, &dir)?;
    if !dir.join("AGENTS.md").exists() { std::fs::write(dir.join("AGENTS.md"), authoring::AGENTS_MD)?; }
    if !dir.join(".gitignore").exists() { std::fs::write(dir.join(".gitignore"), ".cori/\nnode_modules/\n.DS_Store\n")?; }
    let git = git_init(&dir, &format!("cori pull {app}@{}", resolved.trim_start_matches("sha256:").get(..12).unwrap_or("")));
    let sess = persist_session(&dir, Some(app), server, token, source, false, organization_of(server, token).await)?;
    Ok(format!("pulled {app}@{} into {} ({n} files){}{}", resolved.trim_start_matches("sha256:").get(..12).unwrap_or(&resolved), dir.display(), git.map(|g| format!(" · {g}")).unwrap_or_default(), sess.map(|(p, s)| format!(" · session {} ({})", p.display(), session::describe(&s))).unwrap_or_default()))
}

/// `init` and `pull` write `.cori/session` for the server they used, so no later command in the folder needs to name
/// it. A token given as flags or in the environment (an agent's bootstrap) is the folder's own; otherwise the folder
/// holds none and its commands use your login to that server (`cori login`, kept in `~/.cori/config.toml`), so it
/// refreshes in one place. A local `cori up` nothing named is written only with `include_default` (`init` found it
/// answering). Returns the file and what it holds.
pub fn persist_session(app_dir: &Path, app: Option<&str>, server: &str, token: &str, source: &session::Source, include_default: bool, organization: Option<session::Organization>) -> Result<Option<(PathBuf, Session)>> {
    if session::path(app_dir).exists() { return Ok(None); }
    let s = match source {
        session::Source::Env if !token.is_empty() => {
            let kind = if token.starts_with("dev:") { "human".to_string() } else { std::env::var("CORI_SESSION_KIND").unwrap_or_else(|_| "human".into()) };
            Session { server: base(server), app: app.map(String::from), token: token.to_string(), kind, expires_at: std::env::var("CORI_SESSION_EXPIRES_AT").ok(), organization, ..Default::default() }
        }
        session::Source::Default if !include_default && server == session::LOCAL_SERVER => return Ok(None),
        _ => Session { organization, ..Session::binding(&base(server), app) },
    };
    Ok(Some((session::save(app_dir, &s)?, s)))
}

/// The organization `token` acts in on `server` (`/v1/me`), on a server hosting several: what a folder it binds belongs to.
pub async fn organization_of(server: &str, token: &str) -> Option<session::Organization> {
    let me = Api::with_timeout(server, token, std::time::Duration::from_secs(10)).get("/v1/me").await.ok()?;
    session::Organization::of_me(&me)
}

// ---------------------------------------------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------------------------------------------

pub struct LoginArgs { pub url: Option<String>, pub token: Option<String>, pub device: bool, pub global: bool, pub org: Option<String> }

/// The server `cori login` (or `--forget`) means: the one named, else the app folder's, else your default login's.
fn login_server(url: Option<&str>, resolved: &session::Resolved, logins: &session::Logins) -> Option<String> {
    if let Some(u) = url { return Some(base(u)); }
    if let Some(s) = &resolved.session { return Some(base(&s.server)); }
    logins.default_login().map(|l| base(&l.server))
}

/// Where `cori login` signs in: the server it means, else Cori Cloud.
fn login_target(url: Option<&str>, resolved: &session::Resolved, logins: &session::Logins) -> String {
    login_server(url, resolved, logins).unwrap_or_else(|| session::CLOUD_SERVER.to_string())
}

/// `cori login <url> [--org <slug>]`: a token as given, or the org's OIDC issuer through authorization code + PKCE on a
/// loopback redirect (`--device` for the device grant), naming the organization to sign in to (the issuer asks a person in
/// several otherwise). The login is kept in `~/.cori/config.toml`, one per server and organization, and becomes the
/// default outside app folders; inside an app folder (unless `--global`), the folder works with that server and, when it
/// belongs to none yet, that organization. A folder of another organization keeps its own.
pub async fn login(a: &LoginArgs, resolved: &session::Resolved) -> Result<String> {
    let url = login_target(a.url.as_deref(), resolved, &session::Logins::load());
    let http = cori_engine::http::builder().user_agent(crate::client::USER_AGENT).timeout(std::time::Duration::from_secs(15)).build()?;
    let health = http.get(format!("{url}/healthz")).send().await.map_err(|e| anyhow!("{url} is not reachable: {e}"))?.error_for_status()?;
    crate::client::note_cli_min(health.headers());
    let app_root = if a.global { None } else { std::env::current_dir().ok().and_then(|c| session::app_root_of(&c)) };
    let mut sess = Session { server: url.clone(), kind: "human".into(), ..Default::default() };
    if let Some(t) = &a.token { sess.token = t.clone(); }
    else {
        let cfg: Value = http.get(format!("{url}/v1/auth/config")).send().await?.json().await.unwrap_or(Value::Null);
        match cfg["mode"].as_str() {
            Some("builtin") => {
                let issuer = cfg["issuer"].as_str().context("server has no issuer")?.to_string();
                let client_id = cfg["cli_client_id"].as_str().or(cfg["client_id"].as_str()).unwrap_or("cori-cli").to_string();
                // the server's own resource (RFC 8707): an issuer that honours it (cori-auth) signs a token of that audience,
                // without it an opaque one the server cannot verify; one that does not (Keycloak) ignores it
                let resource = cfg["resource"].as_str();
                // the server's own scopes besides ours: an issuer may pick the token's audience by them (Entra ID)
                let extra: Vec<String> = cfg["scopes"].as_array().map(|s| s.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                let scope = cori_server::sign_in_scopes(&LOGIN_SCOPES, &extra).join(" ");
                let oidc: Value = http.get(format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))).send().await?.json().await?;
                let org = a.org.as_deref().map(str::trim).filter(|o| !o.is_empty());
                let tok = if a.device { device_grant(&http, &oidc, &client_id, &scope, resource, org).await? } else { pkce_grant(&http, &oidc, &client_id, &scope, resource, org).await? };
                sess.token = tok["access_token"].as_str().context("no access_token")?.to_string();
                sess.refresh_token = tok["refresh_token"].as_str().map(String::from);
                sess.expires_at = tok["expires_in"].as_i64().map(|e| (chrono::Utc::now() + chrono::Duration::seconds(e)).to_rfc3339());
                sess.issuer = Some(issuer); sess.client_id = Some(client_id);
                // a refresh asks for the same scopes again, when the server adds its own
                sess.sign_in_scope = (!extra.is_empty()).then_some(scope);
            }
            Some("dev") => { sess.token = session::dev_token(); }
            Some("off") => { sess.token = String::new(); }
            other => bail!("server auth mode {other:?}: pass --token"),
        }
    }
    let me: Value = http.get(format!("{url}/v1/me")).bearer_auth(&sess.token).send().await?.json().await.unwrap_or(Value::Null);
    sess.organization = session::Organization::of_me(&me);
    // the organization asked for, and the one the token names: a person not in it was asked for another, or refused
    if let Some(want) = a.org.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        match &sess.organization {
            Some(o) if o.slug.as_deref() == Some(want) || o.id == want => {}
            Some(o) => bail!("you signed in to {o}, not {want}: your account may not belong to {want} (your login was not saved)"),
            None => bail!("{url} hosts a single organization: sign in without --org"),
        }
    }
    let who = me["sub"].as_str().map(|s| format!("you are {} · roles {}", me["email"].as_str().unwrap_or(s), me["roles"])).unwrap_or_else(|| "token not accepted by the server".into());
    let org = sess.organization.clone();
    let at = match &org { Some(o) => format!("{url} in organization {o}"), None => url.clone() };
    // your login to that server and organization, in place of the one before, and now the default; one copy of its refresh
    // token, which the server rotates: the folders that work with them use it from there
    let file = session::global_path().context("no home directory")?;
    let others = session::save_login(&file, sess.clone())?;
    let mut out = match app_root {
        Some(root) => {
            let current = session::load(&root);
            let theirs = current.as_ref().filter(|c| session::origin(&c.server) == session::origin(&url)).and_then(|c| c.organization.clone());
            match (&theirs, &org) {
                // a folder of another organization stays that organization's
                (Some(t), Some(o)) if t.id != o.id => format!("logged in to {at} · {who}\n{} belongs to {t}, and keeps it: `cori login {url} --org {t}` signs you in there (your logins: {})", session::path(&root).display(), file.display()),
                _ => {
                    let p = session::save(&root, &Session { organization: theirs.or(org.clone()), ..Session::binding(&url, app_id_of(&root).as_deref()) })?;
                    format!("logged in to {at} · {who}\n{} now works with {at}, and so do commands outside app folders (your logins: {})", p.display(), file.display())
                }
            }
        }
        None => format!("logged in to {at} (saved in {}: the default outside app folders) · {who}", file.display()),
    };
    if !others.is_empty() { out.push_str(&format!("\nyour other logins, for the app folders that work with them: {}", others.join(", "))); }
    // the person's other organizations here, when the server can tell
    if org.is_some() {
        if let Ok(r) = http.get(format!("{url}/v1/me/organizations")).bearer_auth(&sess.token).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let rest: Vec<String> = v["organizations"].as_array().map(|a| a.iter().filter(|o| o["id"].as_str() != org.as_ref().map(|o| o.id.as_str())).filter_map(|o| o["slug"].as_str().map(String::from)).collect()).unwrap_or_default();
                if !rest.is_empty() { out.push_str(&format!("\nyou also belong to {} here: `cori login {url} --org <slug>` signs you in to one of them", rest.join(", "))); }
            }
        }
    }
    Ok(out)
}

/// `cori login --list`: your logins, the default first, with their organization and when each expires.
pub fn list_logins() -> Result<String> {
    let l = session::Logins::load();
    if l.0.is_empty() { return Ok("no login yet: `cori login <url>`".into()); }
    let rows: Vec<String> = l.0.iter().enumerate().map(|(i, s)| {
        let exp = s.expires_at.as_deref().map(|e| if s.expired() && s.refresh_token.is_none() { format!("expired {e}") } else { format!("until {e}{}", if s.refresh_token.is_some() { ", refreshes itself" } else { "" }) }).unwrap_or_default();
        format!("{}{}{}{}", if i == 0 { "* " } else { "  " }, s.server, s.organization.as_ref().map(|o| format!("  organization {o}")).unwrap_or_default(), if exp.is_empty() { String::new() } else { format!("  ({exp})") })
    }).collect();
    Ok(format!("{}\n(* the default, outside app folders; {})", rows.join("\n"), session::global_path().map(|p| p.display().to_string()).unwrap_or_default()))
}

/// `cori login --forget [url] [--org <slug>]`: forgets your login to the server in that organization (the only one you
/// have there, without --org).
pub fn forget_login(url: Option<&str>, org: Option<&str>, resolved: &session::Resolved) -> Result<String> {
    let url = login_server(url, resolved, &session::Logins::load()).context("name the server whose login to forget: `cori login <url> --forget`")?;
    let file = session::global_path().context("no home directory")?;
    let _lock = session::lock(&file)?;
    let mut l = session::Logins::read(&file);
    let mine: Vec<Session> = l.all_for_server(&url).cloned().collect();
    let target = match org {
        Some(o) => mine.iter().find(|s| s.organization.as_ref().is_some_and(|x| x.slug.as_deref() == Some(o) || x.id == o)).cloned(),
        None if mine.len() == 1 => mine.first().cloned(),
        None if mine.is_empty() => None,
        None => bail!("you have logins to several organizations on {url} ({}): name one with --org", mine.iter().map(session::login_label).collect::<Vec<_>>().join(", ")),
    };
    let Some(t) = target else { bail!("no login to {url}{}", org.map(|o| format!(" in {o}")).unwrap_or_default()) };
    l.forget(&url, t.org_id());
    l.write(&file)?;
    Ok(format!("forgot your login to {} (its refresh token with it)", session::login_label(&t)))
}

fn b64url(b: &[u8]) -> String { use base64::Engine; base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }

/// The scopes `cori login` asks for, before the server's own (`/v1/auth/config`'s `scopes`).
const LOGIN_SCOPES: [&str; 4] = ["openid", "profile", "email", "offline_access"];

async fn pkce_grant(http: &reqwest::Client, oidc: &Value, client_id: &str, scope: &str, resource: Option<&str>, org: Option<&str>) -> Result<Value> {
    let auth_ep = oidc["authorization_endpoint"].as_str().context("issuer has no authorization_endpoint")?;
    let token_ep = oidc["token_endpoint"].as_str().context("issuer has no token_endpoint")?.to_string();
    let verifier = b64url(&rand::random::<[u8; 32]>());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = b64url(&rand::random::<[u8; 16]>());
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
    let mut url = format!("{auth_ep}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}", urlenc(client_id), urlenc(&redirect), urlenc(scope));
    if let Some(r) = resource { url.push_str(&format!("&resource={}", urlenc(r))); }
    // the organization to sign in to: an issuer that knows organizations picks it without asking (cori-auth's `organization`)
    if let Some(o) = org { url.push_str(&format!("&organization={}", urlenc(o))); }
    eprintln!("opening the browser to sign in; if nothing happens, open:\n  {url}");
    open_browser(&url);
    let (code, got_state) = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
        listener.set_nonblocking(false)?;
        let (mut stream, _) = listener.accept()?;
        let mut buf = [0u8; 4096]; let n = std::io::Read::read(&mut stream, &mut buf)?;
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let line = req.lines().next().unwrap_or("").to_string();
        let query = line.split_whitespace().nth(1).and_then(|p| p.split_once('?')).map(|(_, q)| q.to_string()).unwrap_or_default();
        let mut code = String::new(); let mut st = String::new(); let mut err = String::new();
        for kv in query.split('&') { let (k, v) = kv.split_once('=').unwrap_or((kv, "")); match k { "code" => code = urldec(v), "state" => st = urldec(v), "error" | "error_description" => err = format!("{err} {}", urldec(v)), _ => {} } }
        let body = if code.is_empty() { format!("<h2>Sign-in failed</h2><p>{err}</p>") } else { "<h2>Signed in to Cori</h2><p>You can close this tab.</p>".to_string() };
        let _ = std::io::Write::write_all(&mut stream, format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes());
        if code.is_empty() { bail!("sign-in failed:{err}"); }
        Ok((code, st))
    }).await??;
    if got_state != state { bail!("state mismatch in the OIDC callback"); }
    let mut form = vec![("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", redirect.as_str()), ("client_id", client_id), ("code_verifier", verifier.as_str())];
    form.extend(resource.map(|r| ("resource", r)));
    let r = http.post(&token_ep).form(&form).send().await?;
    let v: Value = r.json().await?;
    if v["access_token"].is_null() { bail!("token endpoint refused the code: {}", v["error_description"].as_str().or(v["error"].as_str()).unwrap_or("")); }
    Ok(v)
}

async fn device_grant(http: &reqwest::Client, oidc: &Value, client_id: &str, scope: &str, resource: Option<&str>, org: Option<&str>) -> Result<Value> {
    let dev_ep = oidc["device_authorization_endpoint"].as_str().context("issuer has no device_authorization_endpoint (enable the device grant on the client, or drop --device)")?;
    let token_ep = oidc["token_endpoint"].as_str().context("issuer has no token_endpoint")?.to_string();
    let mut form = vec![("client_id", client_id), ("scope", scope)];
    form.extend(resource.map(|r| ("resource", r)));
    form.extend(org.map(|o| ("organization", o)));
    let d: Value = http.post(dev_ep).form(&form).send().await?.json().await?;
    let code = d["device_code"].as_str().context("no device_code")?.to_string();
    eprintln!("open {} and enter the code {}", d["verification_uri_complete"].as_str().or(d["verification_uri"].as_str()).unwrap_or("?"), d["user_code"].as_str().unwrap_or("?"));
    let interval = d["interval"].as_u64().unwrap_or(5).max(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(d["expires_in"].as_u64().unwrap_or(600));
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline { bail!("device code expired"); }
        let mut form = vec![("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("device_code", code.as_str()), ("client_id", client_id)];
        form.extend(resource.map(|r| ("resource", r)));
        let v: Value = http.post(&token_ep).form(&form).send().await?.json().await?;
        if v["access_token"].is_string() { return Ok(v); }
        match v["error"].as_str() { Some("authorization_pending") | Some("slow_down") => continue, Some(e) => bail!("device grant failed: {e} {}", v["error_description"].as_str().unwrap_or("")), None => bail!("unexpected token response: {v}") }
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")] let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")] let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")] let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
}
fn urlenc(s: &str) -> String { let mut o = String::new(); for b in s.bytes() { if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { o.push(b as char) } else { o.push_str(&format!("%{b:02X}")) } } o }
fn urldec(s: &str) -> String { let b = s.as_bytes(); let mut o = Vec::new(); let mut i = 0; while i < b.len() { if b[i] == b'%' && i + 2 < b.len() { if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { o.push(v); i += 3; continue; } } o.push(if b[i] == b'+' { b' ' } else { b[i] }); i += 1; } String::from_utf8_lossy(&o).to_string() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_means_the_server_named_the_folders_or_your_default_else_cori_cloud() {
        let nothing = session::Resolved { source: session::Source::Default, session: None, app_root: None, login: None, server: session::LOCAL_SERVER.into() };
        let none = session::Logins::default();
        assert_eq!(login_server(None, &nothing, &none), None, "`--forget` refuses to guess");
        assert_eq!(login_target(None, &nothing, &none), "https://app.cori.do", "a first `cori login`: Cori Cloud");
        assert_eq!(login_target(Some("http://127.0.0.1:8080/"), &nothing, &none), "http://127.0.0.1:8080");
        let mine = session::Logins(vec![Session { server: "https://cori.acme.example".into(), ..Default::default() }]);
        assert_eq!(login_target(None, &nothing, &mine), "https://cori.acme.example", "your default login's server");
        let folder = session::Resolved { session: Some(Session { server: "https://other.example/".into(), ..Default::default() }), ..nothing };
        assert_eq!(login_target(None, &folder, &mine), "https://other.example", "the app folder's server comes first");
    }
    #[test]
    fn bundle_round_trip() {
        let d = tempdir("bundle"); std::fs::create_dir_all(d.join("workflows/w/steps")).unwrap(); std::fs::create_dir_all(d.join(".cori")).unwrap(); std::fs::create_dir_all(d.join("dist")).unwrap();
        std::fs::write(d.join("cori.toml"), "[app]\nid = \"t\"\n").unwrap(); std::fs::write(d.join("workflows/w/workflow.yaml"), "id: w\nsteps: []\n").unwrap(); std::fs::write(d.join(".cori/session"), "secret").unwrap(); std::fs::write(d.join("dist/a.png"), [0u8, 1, 2, 255]).unwrap();
        std::fs::create_dir_all(d.join("front")).unwrap(); std::fs::write(d.join("front/.env.local"), "VITE_KEY=x").unwrap(); std::fs::write(d.join(".npmrc"), "//registry/:_authToken=x").unwrap();
        let b = bundle(&d).unwrap(); assert_eq!(b.app, "t"); assert_eq!(b.files, 3);
        assert_eq!(b.credentials, vec![".npmrc".to_string(), "front/.env.local".to_string()]);
        let out = tempdir("out"); assert_eq!(extract(&b.bytes, &out).unwrap(), 3);
        assert!(!out.join(".cori/session").exists()); assert_eq!(std::fs::read(out.join("dist/a.png")).unwrap(), [0u8, 1, 2, 255]);
        assert_eq!(bundle(&out).unwrap().hash, b.hash);
    }
    #[test]
    fn front_build_warning_tracks_dist() {
        let d = tempdir("front-warn");
        assert!(front_build_warning(&d).is_none(), "no front/package.json: nothing to say");
        std::fs::create_dir_all(d.join("front/src")).unwrap(); std::fs::write(d.join("front/package.json"), "{}").unwrap(); std::fs::write(d.join("front/src/App.tsx"), "").unwrap();
        assert!(front_build_warning(&d).unwrap().contains("no build"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(d.join("dist")).unwrap(); std::fs::write(d.join("dist/index.html"), "").unwrap();
        assert!(front_build_warning(&d).is_none());
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(d.join("front/src/App.tsx"), "x").unwrap();
        assert!(front_build_warning(&d).unwrap().contains("changed since"));
    }
    fn tempdir(n: &str) -> PathBuf { let p = std::env::temp_dir().join(format!("cori-cli-test-{n}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&p); std::fs::create_dir_all(&p).unwrap(); p }
}

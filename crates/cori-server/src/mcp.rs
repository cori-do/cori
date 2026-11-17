// SPDX-License-Identifier: Apache-2.0
//! The remote MCP server (`POST /mcp`, streamable HTTP, JSON responses): what an agent host reaches with the OAuth
//! session it holds. Nothing here reads or writes the app's files: apps and sessions (`list_apps`,
//! `get_app`, `create_app`, `login`), discovery (`list_connectors`, `find_tool`, `request_connector`, and `scaffold`,
//! which returns a step file as text for hosts without a shell), and the MCP Apps widgets (`app_settings`, `store_browse`,
//! `store_query`, `preview`) whose HTML lives under `apps/` and calls back through the host (`tools/call`). The widgets
//! read; an app changes only through its folder and `cori deploy`.
use crate::{api::{self, ApiError, InRealm}, auth::Principal, state::AppState};
use axum::{Json, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use cori_compiler::authoring;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

const PROTOCOL: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Cori builds durable, typed, permissioned backends for internal apps. Authoring happens in the app folder with the `cori` cli (read the app's AGENTS.md); this server gives what the folder cannot: `list_apps` / `get_app` / `create_app` (which app to edit or create), `login` (a session for the cli: write its `session_toml` to <app>/.cori/session, or run the bootstrap command), `list_connectors` / `find_tool` (what the org declared; never configure servers yourself; `request_connector` when nothing fits), and `scaffold` when no shell is available (with a shell use `cori scaffold`). A person's first app: `create_app` with `template: team-requests`, `login`, `cori init <id> --app <id> --template team-requests`, then `cori deploy` in the folder: it runs as it is, and its preview URL shows a working front. Every deployed version runs on its own seeded sandbox stores; `cori promote` makes one the default, a human action (console or `cori login`), gated by an approver on protected apps. An agent session (from `login`) reaches its own app only: it deploys and pulls versions and reads their sandboxes; promotion, approvals, live data and live runs stay with a person. `app_settings`, `store_browse` and `preview` show a person the app's settings, store rows and front (MCP Apps).";

const SETTINGS_HTML: &str = include_str!("../apps/settings.html");
const STORE_HTML: &str = include_str!("../apps/store.html");
const PREVIEW_HTML: &str = include_str!("../apps/preview.html");

pub async fn handle_get() -> Response { (StatusCode::METHOD_NOT_ALLOWED, [("allow", "POST, DELETE")], Json(json!({ "error": "this MCP server answers POST requests with JSON (no server-initiated stream)" }))).into_response() }
pub async fn handle_delete() -> Response { StatusCode::OK.into_response() }

pub async fn handle_post(InRealm(st): InRealm, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    let who = match api::mcp_principal(&st, &headers).await {
        Ok(w) => w,
        Err(ApiError(status, msg)) if status == StatusCode::UNAUTHORIZED => {
            let meta = format!("{}/.well-known/oauth-protected-resource/mcp", st.cfg.public_url.trim_end_matches('/'));
            // the deployment's own scopes, which the metadata lists too, for a host that reads the challenge only
            let scope = if st.cfg.auth_scopes.is_empty() { String::new() } else { format!(", scope=\"{}\"", crate::sign_in_scopes(&api::MCP_SCOPES, &st.cfg.auth_scopes).join(" ")) };
            return (StatusCode::UNAUTHORIZED, [("www-authenticate", format!("Bearer resource_metadata=\"{meta}\"{scope}, error=\"invalid_token\", error_description=\"{}\"", msg.replace('"', "'")))], Json(json!({ "error": msg }))).into_response();
        }
        Err(e) => return e.into_response(),
    };
    let session = headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).map(String::from);
    let (msgs, batch) = match req { Value::Array(a) => (a, true), v => (vec![v], false) };
    let mut out = Vec::new(); let mut new_session: Option<String> = None;
    for m in msgs {
        let id = m.get("id").cloned();
        let method = m.get("method").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if id.is_none() || id == Some(Value::Null) { continue; } // notifications and responses: nothing to answer
        let params = m.get("params").cloned().unwrap_or(json!({}));
        let sess = session.clone().or_else(|| new_session.clone());
        let result = match method.as_str() {
            "initialize" => {
                // the host says what it is (`clientInfo`): its organization's first steps see that an agent connected
                let host = params.get("clientInfo").map(|c| format!("{} {}", c["name"].as_str().unwrap_or("an MCP host"), c["version"].as_str().unwrap_or(""))).unwrap_or_else(|| "an MCP host".into());
                api::client_seen(&st, "mcp", host.trim(), &who.sub).await;
                let sid = session.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()); new_session = Some(sid); Ok(json!({ "protocolVersion": params.get("protocolVersion").and_then(|p| p.as_str()).filter(|p| *p == "2025-03-26" || *p == "2025-06-18" || *p == "2024-11-05").unwrap_or(PROTOCOL), "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false } }, "serverInfo": { "name": "cori", "version": env!("CARGO_PKG_VERSION") }, "instructions": INSTRUCTIONS.replacen("Cori", &st.brand.name, 1) })) }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "resources/list" => Ok(json!({ "resources": resource_defs(&st) })),
            "resources/templates/list" | "prompts/list" => Ok(json!({ "resourceTemplates": [], "prompts": [] })),
            "resources/read" => read_resource(&st, params.get("uri").and_then(|u| u.as_str()).unwrap_or("")),
            "tools/call" => Ok(call(&st, &who, sess.as_deref(), &params).await),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        out.push(match result { Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }), Err((code, msg)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } }) });
    }
    let mut resp = if out.is_empty() { StatusCode::ACCEPTED.into_response() } else if batch { Json(Value::Array(out)).into_response() } else { Json(out.remove(0)).into_response() };
    if let Some(sid) = new_session.or(session) { if let Ok(v) = sid.parse() { resp.headers_mut().insert("mcp-session-id", v); } }
    resp
}

fn text_result(v: Value, text: Option<String>) -> Value { json!({ "content": [{ "type": "text", "text": text.unwrap_or_else(|| serde_json::to_string_pretty(&v).unwrap_or_default()) }], "structuredContent": v, "isError": false }) }
fn error_result(msg: String) -> Value { json!({ "content": [{ "type": "text", "text": msg }], "isError": true }) }

async fn call(st: &Arc<AppState>, who: &Principal, session: Option<&str>, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let meta = params.get("_meta").cloned().unwrap_or(json!({}));
    match tool(st, who, session, name, &args, &meta).await { Ok(v) => v, Err(e) => { let (status, msg) = e.public(); error_result(format!("{status}: {msg}")) } }
}

async fn tool(st: &Arc<AppState>, who: &Principal, session: Option<&str>, name: &str, args: &Value, meta: &Value) -> Result<Value, ApiError> {
    let s = |k: &str| args[k].as_str().map(String::from);
    let strings = |k: &str| args[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
    // `app` and `id` name the same thing (list_apps answers `id`); either spelling is accepted everywhere
    let need = |k: &str| args[k].as_str().or_else(|| match k { "app" => args["id"].as_str(), "id" => args["app"].as_str(), _ => None }).filter(|v| !v.is_empty()).map(String::from).ok_or(ApiError(StatusCode::BAD_REQUEST, format!("`{k}` is required")));
    match name {
        "list_apps" => {
            let out = api::built_apps(st, who).await?;
            let text = if out.is_empty() { format!("no app yet for {}: create one with `create_app`", who.sub) } else { out.iter().map(|a| format!("{} · owner {} · {} · {}", a["id"].as_str().unwrap_or(""), a["owner"].as_str().unwrap_or("-"), a["promoted"].as_str().map(|h| format!("promoted {}", h.trim_start_matches("sha256:").get(..12).unwrap_or(""))).unwrap_or_else(|| "no promoted version".into()), a["pull"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n") };
            Ok(text_result(json!({ "apps": out }), Some(text)))
        }
        "get_app" => {
            let app = need("app")?;
            let rec = api::app_record(st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}` (list_apps, or create_app)")))?;
            if !api::is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; ask to be added as a builder", rec.owner.as_deref().unwrap_or("nobody")))); }
            let mut v = api::app_summary(st, &rec).await?;
            v["next"] = json!(if rec.promoted.is_some() || v["versions"].as_i64().unwrap_or(0) > 0 { format!("`login` with app `{app}`, then `{}` (or `cori pull {app} ./{app} --hash <hash>` for a version that is not promoted)", v["pull"].as_str().unwrap_or("")) } else { format!("`login` with app `{app}`, then `cori init {app} --app {app}` (no version was published yet)") });
            Ok(text_result(v, None))
        }
        "create_app" => {
            let id = need("id")?;
            let rec = api::create_app_record(st, who, &id, s("name").as_deref(), &strings("roles"), &strings("builders"), s("template").as_deref()).await?;
            let mut v = api::app_summary(st, &rec).await?;
            v["next"] = json!(format!("`login` with app `{id}`, then `cori init {id} --app {id}{}`", s("template").filter(|t| t != "blank").map(|t| format!(" --template {t}")).unwrap_or_default()));
            Ok(text_result(v, None))
        }
        "login" => {
            let app = need("app")?;
            let rec = api::app_record(st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`: create_app first")))?;
            if !api::is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; ask to be added as a builder", rec.owner.as_deref().unwrap_or("nobody")))); }
            let ttl = args["ttl_secs"].as_i64().unwrap_or(crate::auth::AGENT_TTL_DEFAULT_SECS);
            let (app_roles, _) = api::app_roles_of(st, who, &app, rec.client_id.as_deref()).await;
            let m = st.auth.mint_agent(who, &app, app_roles, ttl, session.map(String::from), st.realm()).map_err(|e| ApiError(StatusCode::FORBIDDEN, e))?;
            sqlx::query("INSERT INTO agent_sessions (jti, sub, app, mcp_session, issued_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6)").bind(&m.jti).bind(&who.sub).bind(&app).bind(session).bind(chrono::Utc::now().to_rfc3339()).bind(&m.expires_at).execute(&st.db).await?;
            crate::db::audit(&st.db, &who.sub, "agent_session.create", &format!("{app}:{}", m.jti), json!({ "mcp_session": session, "expires_at": m.expires_at })).await?;
            let server = st.cfg.public_url.trim_end_matches('/').to_string();
            // on a server of several organizations, the folder belongs to the one the session acts in
            let org = api::organization_of(st);
            let org_toml = org.as_ref().map(|o| format!("organization = {{ id = {:?}{} }}\n", o["id"].as_str().unwrap_or_default(), o["slug"].as_str().map(|s| format!(", slug = {s:?}")).unwrap_or_default())).unwrap_or_default();
            let session_toml = format!("server = {server:?}\napp = {app:?}\nkind = \"agent\"\ntoken = {:?}\nexpires_at = {:?}\nscopes = [{}]\n{org_toml}", m.token, m.expires_at, m.scopes.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", "));
            let has_version = rec.promoted.is_some() || sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM versions WHERE app = $1").bind(&app).fetch_one(&st.db).await? > 0;
            // a new folder from the template the app was created with (`create_app`), else empty
            let init = format!("cori init {app} --app {app}{}", rec.template.as_deref().filter(|t| *t != "blank").map(|t| format!(" --template {t}")).unwrap_or_default());
            let bootstrap = format!("CORI_SERVER={server} CORI_TOKEN={} CORI_SESSION_KIND=agent CORI_SESSION_EXPIRES_AT={} {}", m.token, m.expires_at, if has_version { format!("cori pull {app} ./{app}") } else { init.clone() });
            let mut public = json!({ "server": server, "app": app, "kind": "agent", "expires_at": m.expires_at, "scopes": m.scopes, "session_file": ".cori/session", "not_allowed": ["promote", "approve", "rollback", "live data", "live runs", "other apps", "administration"] });
            if let Some(o) = &org { public["organization"] = o.clone(); }
            let sink = meta.get("cori.dev/credential-sink").and_then(|v| v.as_str());
            if sink.is_some() {
                // the harness installs the credential itself and strips `_meta` before the model sees the result
                let mut v = public.clone(); v["delivery"] = json!("sink"); v["next"] = json!(format!("the session is installed by the host; run `cori` commands in the app folder ({})", if has_version { format!("`cori pull {app} ./{app}` if the folder does not exist yet") } else { format!("`{init}` first") }));
                let mut r = text_result(v, None);
                r["_meta"] = json!({ "cori.dev/credential": { "server": public["server"], "app": app, "kind": "agent", "token": m.token, "expires_at": m.expires_at, "scopes": m.scopes, "session_toml": session_toml, "bootstrap": bootstrap } });
                return Ok(r);
            }
            let mut v = public; v["token"] = json!(m.token); v["session_toml"] = json!(session_toml); v["bootstrap"] = json!(bootstrap);
            v["next"] = json!(format!("write `session_toml` verbatim to <app folder>/.cori/session (mode 0600), or run `bootstrap` when the folder does not exist yet; then every `cori` command in that folder is authenticated until {}. Approve and promote stay with a human.", m.expires_at));
            Ok(text_result(v, None))
        }
        "list_connectors" => {
            api::require_scope(who, "registry:read", None)?;
            let rows = api::visible_connectors(st, who).await?;
            let kind = s("kind");
            let rows: Vec<Value> = rows.into_iter().filter(|c| kind.as_deref().map(|k| c["kind"].as_str() == Some(k)).unwrap_or(true)).collect();
            let summary = authoring::summarize_connectors(&rows);
            let text = if summary.is_empty() { "no connector declared yet: ask an admin (request_connector) or use cori.toml overrides for local mocks".to_string() } else { summary.iter().map(|c| format!("{} ({}): {}", c["name"].as_str().unwrap_or(""), c["kind"].as_str().unwrap_or(""), c["tools"].as_array().map(|t| t.iter().map(|x| format!("{} [{}]", x["name"].as_str().unwrap_or(""), x["effect"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ")).unwrap_or_default())).collect::<Vec<_>>().join("\n") };
            Ok(text_result(json!({ "connectors": summary, "next": "find_tool for schemas; `cori scaffold <connector> <tool> --workflow <id>` writes a typed step" }), Some(text)))
        }
        "find_tool" => {
            api::require_scope(who, "registry:read", None)?;
            let rows = api::visible_connectors(st, who).await?;
            let hits = authoring::find_tools(&rows, &need("query")?);
            Ok(text_result(json!({ "tools": hits, "next": "`cori scaffold <connector> <tool> --workflow <id>`" }), None))
        }
        "request_connector" => {
            let url = need("url")?;
            Ok(text_result(api::queue_connector_request(st, who, &url, &s("reason").unwrap_or_default()).await?, None))
        }
        "scaffold" => {
            api::require_scope(who, "registry:read", None)?;
            let mut rows = api::visible_connectors(st, who).await?;
            // an app's own store connector: its tools come from the version's files (stores/<ref>/ migrations and roles),
            // the same derivation as `cori scaffold` in the folder
            if let Some(app) = s("app") {
                api::require_scope(who, "stores:read", Some(&app))?;
                let rec = api::require_app(st, &app).await?;
                if !api::is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; its stores are for its owner, builders and admins", rec.owner.as_deref().unwrap_or("nobody")))); }
                let hash = match s("version") { Some(v) => api::resolve_version_hash(st, &app, &v).await?, None => sqlx::query("SELECT hash FROM versions WHERE app = $1 ORDER BY created_at DESC LIMIT 1").bind(&app).fetch_optional(&st.db).await?.map(|r| r.get::<String, _>("hash")).ok_or(ApiError(StatusCode::CONFLICT, format!("`{app}` has no version yet: with a shell, `cori scaffold` reads the folder; otherwise deploy one first")))? };
                let dir = st.bundles.dir(st.db.realm(), &hash).await?;
                let connector = need("connector")?;
                if let Some(tools) = cori_compiler::stores::bindings(&dir).contains_key(&connector).then(|| cori_compiler::stores::connector_tools(&dir, &connector)).flatten() {
                    rows.retain(|r| r["name"].as_str() != Some(connector.as_str()));
                    rows.push(json!({ "name": connector, "kind": "store", "source": "app", "tools": tools }));
                }
            }
            let sc = authoring::scaffold(&rows, &need("connector")?, &need("tool")?, s("name").as_deref()).map_err(|e| ApiError(StatusCode::BAD_REQUEST, if s("app").is_none() && e.to_string().contains("is not declared") { format!("{e}; for an app's own store connector pass `app`") } else { e.to_string() }))?;
            Ok(text_result(json!({ "connector": sc.connector, "tool": sc.tool, "step": sc.step, "kind": sc.kind, "effect": sc.effect, "file": sc.file, "path": format!("workflows/<workflow>/steps/{}.ts", sc.step), "next": ["write `file` to that path and add the step name to `steps:` in workflow.yaml (with a shell, `cori scaffold` does both)", "add the connector to `connectors:`", "bind `input` from ctx.params / ctx.steps", "`cori check`"] }), Some(sc.file)))
        }
        "app_settings" => {
            let app = need("app")?;
            let (rec, cori_toml, hash) = settings_source(st, who, &app).await?;
            let settings: Value = toml::from_str::<toml::Value>(&cori_toml).map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).unwrap_or(Value::Null);
            let v = json!({ "app": app, "name": rec.name, "owner": rec.owner, "builders": rec.builders, "version": hash, "promoted": rec.promoted, "settings": { "app": settings["app"], "front": settings["front"], "auth": settings["auth"], "llm": settings["llm"], "webhooks": settings["webhooks"], "connectors": settings["connectors"] }, "cori_toml": cori_toml });
            let text = format!("{app}: {} · roles {} · llm {} · {} webhook(s){}", rec.name, settings["auth"]["roles"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default(), settings["llm"]["provider"].as_str().unwrap_or("-"), settings["webhooks"].as_array().map(|a| a.len()).unwrap_or(0), hash.as_deref().map(|h| format!(" · from version {}", h.trim_start_matches("sha256:").get(..12).unwrap_or(""))).unwrap_or_else(|| " · no version yet".into()));
            Ok(text_result(v, Some(text)))
        }
        "store_browse" => {
            let app = need("app")?;
            api::require_scope(who, "stores:read", Some(&app))?;
            let who = &api::scope_to_app(st, who.clone(), &app).await?;   // store roles are app roles
            let at = store_target(st, who, &app, s("version").as_deref()).await?;
            let inspect = store_reach(st, who, &at).await?;
            // its builders get every store with its schema and role files; one of its users the stores their roles open,
            // and of each the tables their role reads, nothing of the rest
            let mut stores: Vec<Value> = Vec::new();
            for r in sqlx::query("SELECT name, schema_yaml, roles_yaml, updated_at FROM store_instances WHERE app = $1 AND target = $2 ORDER BY name").bind(&at.app).bind(&at.target).fetch_all(&st.db).await? {
                let name: String = r.get("name");
                if inspect {
                    let schema = serde_yaml::from_str::<Value>(&r.get::<String, _>("schema_yaml")).unwrap_or(Value::Null);
                    let tables: Vec<String> = schema["tables"].as_object().map(|t| t.keys().cloned().collect()).or_else(|| schema.as_object().map(|t| t.keys().cloned().collect())).unwrap_or_default();
                    stores.push(json!({ "name": name, "tables": tables, "schema": schema, "roles": serde_yaml::from_str::<Value>(&r.get::<String, _>("roles_yaml")).unwrap_or(Value::Null), "updated_at": r.get::<String, _>("updated_at") }));
                } else if api::store_roles(&r.get::<String, _>("roles_yaml")).iter().any(|x| who.roles.contains(x)) {
                    let d = describe(st, who, &at, &name, false).await?;
                    let tables: Vec<String> = d["tables"].as_array().map(|ts| ts.iter().filter(|t| t["columns"].as_array().is_some_and(|cs| cs.iter().any(|c| c["read"].as_bool() == Some(true)))).filter_map(|t| t["name"].as_str().map(String::from)).collect()).unwrap_or_default();
                    stores.push(json!({ "name": name, "tables": tables, "updated_at": r.get::<String, _>("updated_at") }));
                }
            }
            let store = s("store").or_else(|| stores.first().and_then(|x| x["name"].as_str().map(String::from)));
            let mut v = json!({ "app": at.app, "target": at.target, "stores": stores.iter().map(|x| json!({ "name": x["name"], "tables": x["tables"], "updated_at": x["updated_at"] })).collect::<Vec<_>>() });
            if let Some(name) = &store {
                let Some(listed) = stores.iter().find(|x| x["name"].as_str() == Some(name)) else {
                    return Err(if inspect { ApiError(StatusCode::NOT_FOUND, format!("`{app}` has no store `{name}` here")) } else { ApiError(StatusCode::FORBIDDEN, format!("`{app}` has no store `{name}` your roles ({}) open", who.roles.join(", "))) });
                };
                let tables: Vec<String> = listed["tables"].as_array().map(|ts| ts.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default();
                let table = s("table").or_else(|| tables.first().cloned());
                v["store"] = json!(name); v["table"] = json!(table);
                if inspect { v["schema"] = listed["schema"].clone(); }
                if let Some(t) = &table {
                    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return Err(ApiError(StatusCode::BAD_REQUEST, "bad table name".into())); }
                    let limit = args["limit"].as_u64().unwrap_or(50).min(500); let offset = args["offset"].as_u64().unwrap_or(0);
                    // the guarded query refuses `select *` on a table whose role lists its columns: name the readable ones
                    let d = describe(st, who, &at, name, inspect).await?;
                    let described = d["tables"].as_array().and_then(|ts| ts.iter().find(|x| x["name"].as_str() == Some(t))).cloned().unwrap_or(Value::Null);
                    let cols: Vec<String> = described["columns"].as_array().map(|cs| cs.iter().filter(|c| c["read"].as_bool() == Some(true)).filter_map(|c| c["name"].as_str().map(cori_store::ident)).collect()).unwrap_or_default();
                    if cols.is_empty() { return Err(ApiError(StatusCode::FORBIDDEN, format!("your roles ({}) read no column of `{t}` in store `{name}`", who.roles.join(", ")))); }
                    // pages follow the primary key (always granted to a role that reads the table): without an order,
                    // LIMIT/OFFSET pages may repeat or skip rows
                    let pk: Vec<String> = described["primary_key"].as_array().map(|ks| ks.iter().filter_map(|k| k.as_str().map(cori_store::ident)).collect()).unwrap_or_default();
                    let order_by = if pk.is_empty() { String::new() } else { format!(" ORDER BY {}", pk.join(", ")) };
                    let sql = format!("SELECT {} FROM {}{order_by} LIMIT {limit} OFFSET {offset}", cols.join(", "), cori_store::ident(t));
                    let r = store_query(st, who, &at, name, &sql).await?;
                    v["rows"] = r["rows"].clone(); v["columns"] = r["columns"].clone(); v["limit"] = json!(limit); v["offset"] = json!(offset);
                }
            }
            let which = if at.is_live() { "live".to_string() } else { format!("sandbox {}", at.target.trim_start_matches("sha256:").get(..12).unwrap_or("")) };
            let text = match (&store, v["rows"].as_array()) { (Some(n), Some(rows)) => format!("{app} · store {n} ({which}) · table {} · {} row(s) shown (page of {})", v["table"].as_str().unwrap_or("-"), rows.len(), v["limit"]), (Some(n), None) => format!("{app} · store {n} ({which}): no table"), (None, _) => format!("{app}: no {which} store database (a live one exists once a version with stores/<name>/ is promoted; every deployed version has a sandbox)") };
            Ok(text_result(v, Some(text)))
        }
        "store_query" => {
            let app = need("app")?;
            api::require_scope(who, "stores:read", Some(&app))?;
            let who = &api::scope_to_app(st, who.clone(), &app).await?;   // store roles are app roles
            let at = store_target(st, who, &app, s("version").as_deref()).await?;
            store_reach(st, who, &at).await?;
            let store = need("store")?; let sql = need("sql")?;
            let r = store_query(st, who, &at, &store, &sql).await?;
            let text = format!("{} row(s)", r["rows"].as_array().map(|a| a.len()).unwrap_or(0));
            Ok(text_result(r, Some(text)))
        }
        "preview" => {
            let app = need("app")?;
            let rec = api::app_record(st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
            if !api::is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, "only the app's owner, its builders and admins may do this".into())); }
            let hash = match s("hash") { Some(h) => Some(api::resolve_version_hash(st, &app, &h).await?), None => rec.promoted.clone().or(sqlx::query("SELECT hash FROM versions WHERE app = $1 ORDER BY created_at DESC LIMIT 1").bind(&app).fetch_optional(&st.db).await?.map(|r| r.get::<String, _>("hash"))) };
            let Some(hash) = hash else { return Err(ApiError(StatusCode::NOT_FOUND, format!("`{app}` has no version yet (`cori deploy`)"))); };
            let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or("").to_string();
            let is_promoted = rec.promoted.as_deref() == Some(hash.as_str());
            let origin = st.front_origin(&app);
            let seq: Option<i64> = sqlx::query_scalar("SELECT seq FROM versions WHERE hash = $1 AND app = $2").bind(&hash).bind(&app).fetch_optional(&st.db).await?.flatten();
            let url = match (is_promoted, seq) { (true, _) => st.front_url(&app), (false, Some(n)) => st.release_url(&app, n), (false, None) => format!("{origin}/apps/{app}/@{short}/") };
            // the screenshot rides in the result's `_meta`, for the widget: an `<img>` pointing at the server cannot send the
            // session's bearer, and the model has no use for the bytes
            let screenshot = api::screenshot_available(st, &app, &hash).and_then(|p| std::fs::read(p).ok()).filter(|b| b.len() <= 4 * 1024 * 1024).map(|b| format!("data:image/png;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b)));
            let ver = sqlx::query("SELECT status, created_at, created_by, git_commit, review FROM versions WHERE hash = $1").bind(&hash).fetch_optional(&st.db).await?;
            let review: Value = ver.as_ref().and_then(|r| serde_json::from_str(&r.get::<String, _>("review")).ok()).unwrap_or(Value::Null);
            let v = json!({ "app": app, "version": hash, "short": short, "promoted": is_promoted, "url": url, "console_url": crate::names::console_app_url(&st.cfg.public_url, &app), "screenshot": screenshot.is_some(), "status": ver.as_ref().map(|r| r.get::<String, _>("status")), "created_at": ver.as_ref().map(|r| r.get::<String, _>("created_at")), "created_by": ver.as_ref().map(|r| r.get::<String, _>("created_by")), "git_commit": ver.as_ref().and_then(|r| r.get::<Option<String>, _>("git_commit")), "workflows": review["workflows"].as_array().map(|a| a.iter().map(|w| json!({ "id": w["id"], "endpoint": w["endpoint"], "schedule": w["schedule"] })).collect::<Vec<_>>()).unwrap_or_default(), "needs_approval": review["needs_approval"] });
            let mut r = text_result(v, Some(format!("{app} {} {short}: {url}", if is_promoted { "promoted" } else { "deployed (sandbox)" })));
            if let Some(png) = screenshot { r["_meta"] = json!({ "cori.dev/screenshot": png }); }
            Ok(r)
        }
        other => Err(ApiError(StatusCode::NOT_FOUND, format!("unknown tool `{other}`"))),
    }
}

/// The cori.toml the settings widget shows: the promoted version's, else the latest version's, else none.
async fn settings_source(st: &AppState, who: &Principal, app: &str) -> Result<(api::AppRecord, String, Option<String>), ApiError> {
    let rec = api::app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !api::is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, "only the app's owner, its builders and admins may do this".into())); }
    let row = match &rec.promoted { Some(h) => sqlx::query("SELECT hash, cori_toml FROM versions WHERE hash = $1").bind(h).fetch_optional(&st.db).await?, None => sqlx::query("SELECT hash, cori_toml FROM versions WHERE app = $1 ORDER BY created_at DESC LIMIT 1").bind(app).fetch_optional(&st.db).await? };
    match row { Some(r) => Ok((rec.clone(), r.get::<String, _>("cori_toml"), Some(r.get::<String, _>("hash")))), None => Ok((rec.clone(), format!("[app]\nid = {app:?}\nname = {:?}\n", rec.name), None)) }
}

/// The databases a store tool reads: a version's sandbox when `version` (a hash or prefix) is given; else the live ones
/// once a version is promoted, the latest version's sandbox before that. An agent session, which can never promote, never
/// reads live data: without `version` it reads the latest version's sandbox.
async fn store_target(st: &Arc<AppState>, who: &Principal, app: &str, version: Option<&str>) -> Result<crate::stores::Target, ApiError> {
    let rec = api::app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    match version {
        Some(v) => Ok(crate::stores::Target::sandbox(app, &api::resolve_version_hash(st, app, v).await?)),
        None if rec.promoted.is_some() && !who.is_agent() => Ok(crate::stores::Target::live(app)),
        None => {
            let latest: Option<String> = sqlx::query("SELECT hash FROM versions WHERE app = $1 ORDER BY created_at DESC LIMIT 1").bind(app).fetch_optional(&st.db).await?.map(|r| r.get::<String, _>("hash"));
            match latest { Some(h) => Ok(crate::stores::Target::sandbox(app, &h)), None if !who.is_agent() => Ok(crate::stores::Target::live(app)), None => Err(ApiError(StatusCode::CONFLICT, format!("`{app}` has no version yet: deploy one (`cori deploy`)"))) }
        }
    }
}

/// Whether `who` (scoped to the app) may read the stores of `at` with these tools, as the HTTP routes let them
/// (`app_store_mcp`, `version_store_mcp`), and whether as the inspector: the live stores as the app's builder or one of its
/// users, a version's sandbox as its builder, an approver, anyone under dev auth.
async fn store_reach(st: &Arc<AppState>, who: &Principal, at: &crate::stores::Target) -> Result<bool, ApiError> {
    let rec = api::require_app(st, &at.app).await?;
    let inspect = api::may_inspect(st, who, &rec, at);
    let reaches = if at.is_live() { inspect || !api::member_roles(st, who, &rec).await?.is_empty() } else { api::is_owner_or_builder(who, &rec) || api::sandbox_guest(st, who) };
    if !reaches {
        let whose = if at.is_live() { "its owner, builders and users" } else { "its owner, builders and approvers" };
        return Err(ApiError(StatusCode::FORBIDDEN, format!("the {} data of `{}` is for {whose}", if at.is_live() { "live" } else { "preview" }, at.app)));
    }
    Ok(inspect)
}

/// `cori/describe` on a store of `at` as `who`: what their role reaches only, unless they may inspect it.
async fn describe(st: &Arc<AppState>, who: &Principal, at: &crate::stores::Target, store: &str, inspect: bool) -> Result<Value, ApiError> {
    let d = crate::stores::rpc(st, at.clone(), store.to_string(), who.clone(), inspect, json!({ "jsonrpc": "2.0", "id": 1, "method": "cori/describe" })).await.map_err(|e| ApiError(crate::stores::status_of(&e), e.to_string()))?;
    Ok(d["result"].clone())
}

/// A read-only SQL query through the store's own guarded `query` tool (the principal's role decides what it sees).
async fn store_query(st: &Arc<AppState>, who: &Principal, at: &crate::stores::Target, store: &str, sql: &str) -> Result<Value, ApiError> {
    let head = sql.trim_start().get(..6).unwrap_or("").to_ascii_lowercase();
    if !head.starts_with("select") && !head.starts_with("with") { return Err(ApiError(StatusCode::BAD_REQUEST, "only SELECT queries here (writes go through workflows)".into())); }
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "query", "arguments": { "sql": sql, "max_rows": 500 } } });
    let inspect = api::may_inspect(st, who, &api::require_app(st, &at.app).await?, at);
    let v = crate::stores::rpc(st, at.clone(), store.to_string(), who.clone(), inspect, req).await.map_err(|e| ApiError(crate::stores::status_of(&e), e.to_string()))?;
    let r = &v["result"];
    if r["isError"].as_bool().unwrap_or(false) { return Err(ApiError(StatusCode::BAD_REQUEST, r["content"][0]["text"].as_str().unwrap_or("query failed").to_string())); }
    let rows = r["structuredContent"]["rows"].as_array().cloned().unwrap_or_default();
    let columns: Vec<String> = rows.first().and_then(|x| x.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
    Ok(json!({ "store": store, "sql": sql, "columns": columns, "rows": rows }))
}

fn read_resource(st: &AppState, uri: &str) -> Result<Value, (i64, String)> {
    let html = match uri { "ui://cori/settings" => SETTINGS_HTML, "ui://cori/store" => STORE_HTML, "ui://cori/preview" => PREVIEW_HTML, _ => return Err((-32002, format!("unknown resource {uri}"))) };
    let html = if st.brand.is_default() { html.to_string() } else { html.replacen("<title>Cori ", &format!("<title>{} ", st.brand.name.replace('&', "&amp;").replace('<', "&lt;")), 1) };
    let mut r = json!({ "uri": uri, "mimeType": "text/html;profile=mcp-app", "text": html });
    r["_meta"] = ui_meta(st, uri);
    Ok(json!({ "contents": [r] }))
}

fn ui_meta(st: &AppState, uri: &str) -> Value {
    // every widget is self-contained; only the preview nests the hosted front (honoured where the host allows frameDomains)
    // fronts on hosts of their own (`CORI_FRONTS_URL`) are framed from there
    let origin = match &st.cfg.fronts_url { Some(f) => f.pattern(), None => reqwest::Url::parse(&st.cfg.public_url).ok().map(|u| u.origin().ascii_serialization()).unwrap_or_default() };
    let mut m = json!({ "ui": { "prefersBorder": true, "csp": { "connectDomains": [], "resourceDomains": [] } } });
    if uri == "ui://cori/preview" && !origin.is_empty() { m["ui"]["csp"]["frameDomains"] = json!([origin]); }
    m
}

fn resource_defs(st: &AppState) -> Vec<Value> {
    [("ui://cori/settings", "App settings", "Form over the app's cori.toml settings"), ("ui://cori/store", "Store data", "Paged table over a Cori Store"), ("ui://cori/preview", "App preview", "The hosted front of a version")].iter().map(|(uri, name, desc)| json!({ "uri": uri, "name": name, "description": desc, "mimeType": "text/html;profile=mcp-app", "_meta": ui_meta(st, uri) })).collect()
}

/// Each tool's title and what it does to the server, for hosts and directories (MCP `title` and `ToolAnnotations`). Three
/// tools write, none removes anything: `create_app` reserves an id, `login` mints an agent session, `request_connector`
/// files a request in the admins' inbox. Every tool reaches this server only.
fn annotations(name: &str) -> (&'static str, bool) {
    match name {
        "list_apps" => ("List apps", false),
        "get_app" => ("Get an app", false),
        "create_app" => ("Create an app", true),
        "login" => ("Sign the cli in to an app", true),
        "list_connectors" => ("List connectors", false),
        "find_tool" => ("Find a connector tool", false),
        "request_connector" => ("Request a connector", true),
        "scaffold" => ("Write a step file", false),
        "app_settings" => ("App settings", false),
        "store_browse" => ("Browse a store", false),
        "store_query" => ("Query a store", false),
        "preview" => ("Preview a version", false),
        _ => ("", false),
    }
}

fn tool_defs() -> Vec<Value> {
    tools().into_iter().map(|mut t| {
        let (title, writes) = annotations(t["name"].as_str().unwrap_or(""));
        t["title"] = json!(title);
        t["annotations"] = if writes { json!({ "title": title, "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }) } else { json!({ "title": title, "readOnlyHint": true, "openWorldHint": false }) };
        t
    }).collect()
}

fn tools() -> Vec<Value> {
    let app = json!({ "type": "string", "description": "app id (the `id` of list_apps; `id` is accepted as an alias)" });
    vec![
        json!({ "name": "list_apps", "description": "Apps you own or build on this Cori server (id, owner, promoted version, how to pull). Start here to edit an existing app.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "get_app", "description": "One app: owner, builders, roles, promoted version, last version, urls, and the command to fetch it locally (`cori pull`).", "inputSchema": { "type": "object", "properties": { "app": app }, "required": ["app"] } }),
        json!({ "name": "create_app", "description": "Reserve a new app id (you become its owner). Then `login` and `cori init <id> --app <id>` in a folder.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string", "description": "letters, digits, - and _" }, "name": { "type": "string" }, "roles": { "type": "array", "items": { "type": "string" }, "description": "the app's roles, strongest first (e.g. [\"manager\", \"employee\"]): `cori init` writes them into cori.toml in this order, and a person holding several acts in the app's stores as the first. A template keeps its own" }, "builders": { "type": "array", "items": { "type": "string" }, "description": "other principals allowed to publish versions" }, "template": { "type": "string", "enum": ["blank", "team-requests", "support-desk"], "description": "blank (the default: an empty app), team-requests (a small finished app that runs on this server as it is: a store, a model step, an approval, its React front built; the quickest first app) or support-desk (the demo of every step kind, whose connectors an organization must provide)" } }, "required": ["id"] } }),
        json!({ "name": "login", "description": "A session for the `cori` cli on one app: a short-lived agent token that deploys and pulls versions and reads their sandboxes (never promotes, approves, or reaches live data). Write `session_toml` verbatim to <app folder>/.cori/session, or run `bootstrap` when the folder does not exist yet. Call again when the cli reports an expired session.", "inputSchema": { "type": "object", "properties": { "app": app, "ttl_secs": { "type": "integer", "description": "lifetime in seconds (default 3600, max 14400)" } }, "required": ["app"] } }),
        json!({ "name": "list_connectors", "description": "Connectors the org declared (MCP servers, stores, HTTP APIs, native activities): names, kinds, tools with their effect. Use these names in `connector:` of a step; never configure servers yourself.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["mcp", "store", "http", "activity", "cli"] } } } }),
        json!({ "name": "find_tool", "description": "Search the declared connectors' tools by words (name, description, schema fields): ranked candidates with input schemas.", "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } }),
        json!({ "name": "request_connector", "description": "Ask an org admin to declare a new MCP server or API in Cori (lands in the admin inbox). Use when no declared connector covers the need.", "inputSchema": { "type": "object", "properties": { "url": { "type": "string" }, "reason": { "type": "string" } }, "required": ["url"] } }),
        json!({ "name": "scaffold", "description": "A typed step file for a connector tool, returned as text (for hosts without a shell; with a shell prefer `cori scaffold`, which writes it and updates `steps:`).", "inputSchema": { "type": "object", "properties": { "connector": { "type": "string" }, "tool": { "type": "string" }, "name": { "type": "string", "description": "step name (snake_case); defaults to the tool name" }, "app": { "type": "string", "description": "app id: needed for the app's own store connectors (their tools come from the app's latest version: stores/<ref>/ migrations and roles)" }, "version": { "type": "string", "description": "version hash or prefix to read the store from (default: the latest version)" } }, "required": ["connector", "tool"] } }),
        json!({ "name": "app_settings", "description": "The app's settings (name, roles, front, llm, webhooks, connector overrides) from its promoted (else latest) version's cori.toml, shown as a form. They change in the app folder: edit cori.toml, then `cori deploy`.", "inputSchema": { "type": "object", "properties": { "app": app }, "required": ["app"] }, "_meta": { "ui": { "resourceUri": "ui://cori/settings" } } }),
        json!({ "name": "store_browse", "description": "Browse a store of an app: its tables and a page of rows (what your role may read). Default: the live databases once a version is promoted, else the latest version's sandbox (an agent session always reads a sandbox); `version` names one.", "inputSchema": { "type": "object", "properties": { "app": app, "version": { "type": "string", "description": "version hash or prefix: browse that version's sandbox" }, "store": { "type": "string" }, "table": { "type": "string" }, "limit": { "type": "integer" }, "offset": { "type": "integer" } }, "required": ["app"] }, "_meta": { "ui": { "resourceUri": "ui://cori/store" } } }),
        json!({ "name": "store_query", "description": "A read-only SQL query (SELECT) on a store of an app, limited to what your role may read; `version` targets a sandbox (an agent session always reads one).", "inputSchema": { "type": "object", "properties": { "app": app, "version": { "type": "string" }, "store": { "type": "string" }, "sql": { "type": "string" } }, "required": ["app", "store", "sql"] }, "_meta": { "ui": { "resourceUri": "ui://cori/store", "visibility": ["app"] } } }),
        json!({ "name": "preview", "description": "Preview an app version: the hosted front's url (nested where the host allows it), a screenshot when available, the version's workflows and review state.", "inputSchema": { "type": "object", "properties": { "app": app, "hash": { "type": "string", "description": "version hash or prefix (default: the promoted version, else the latest)" } }, "required": ["app"] }, "_meta": { "ui": { "resourceUri": "ui://cori/preview" } } }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool has a title and says whether it writes, as hosts show them and a directory requires; three write, and
    /// none of them removes anything.
    #[test]
    fn every_tool_has_a_title_and_says_whether_it_writes() {
        let tools = tool_defs();
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            assert!(!t["title"].as_str().unwrap_or("").is_empty() && t["annotations"]["title"] == t["title"], "{name}: no title");
            assert!(t["annotations"]["readOnlyHint"].is_boolean() && t["annotations"]["openWorldHint"] == false, "{name}");
        }
        let writes: Vec<&str> = tools.iter().filter(|t| t["annotations"]["readOnlyHint"] == false).map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(writes, ["create_app", "login", "request_connector"]);
        assert!(tools.iter().filter(|t| t["annotations"]["readOnlyHint"] == false).all(|t| t["annotations"]["destructiveHint"] == false));
    }
}

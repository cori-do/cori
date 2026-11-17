// SPDX-License-Identifier: Apache-2.0
//! Files runs are given (`file` and `files` parameters). A front announces an upload for a workflow's
//! parameter (`POST …/files`), sends its bytes once (`PUT …/files/{id}/content`), then starts the run with the file's
//! id: the run's parameters carry the server's reference of it (`cori_compiler::files::CoriFile`), never the bytes,
//! which stay in the file store (`bundles::FileStore`: a bucket of their own) until a step's worker asks for them
//! (`GET /v1/runs/{run}/files/{id}`).
//!
//! A file belongs to its app, to the stores its runs work on (`target`: live, or a release's hash, as a run's
//! `store_target`), and to who uploaded it. It is read back by who uploaded it, by those who see every run of the app,
//! and by the people of the runs that hold it (their initiator, who their approvals are routed to).
use super::*;
use cori_compiler::files::{self as cf, CoriFile};
use cori_compiler::manifest::Parameter;
use futures::StreamExt;

/// How long an upload no run used is kept.
const UPLOAD_TTL_HOURS: i64 = 24;
/// The most files one sweep removes; the next takes the rest.
const SWEEP_BATCH: i64 = 500;

const SELECT_FILE: &str = "SELECT id, app, target, owner, workflow, parameter, name, content_type, size, sha256, status, accept, max_bytes, created_at, expires_at FROM files WHERE id = $1 AND app = $2";
const SELECT_FILE_FOR_RUN: &str = "SELECT id, app, target, owner, workflow, parameter, name, content_type, size, sha256, status, accept, max_bytes, created_at, expires_at FROM files WHERE id = $1 AND app = $2 AND target = $3 AND status = 'ready'";

/// A file's record (`files`).
#[derive(Debug, Clone)]
struct FileRow {
    id: String, target: String, owner: String, parameter: String, name: String, content_type: String, size: i64,
    sha256: Option<String>, status: String, accept: Option<Vec<String>>, created_at: String, expires_at: Option<String>,
}

impl FileRow {
    fn of(r: &sqlx::postgres::PgRow) -> Self {
        FileRow {
            id: r.get("id"), target: r.get("target"), owner: r.get("owner"), parameter: r.get("parameter"), name: r.get("name"),
            content_type: r.get("content_type"), size: r.get("size"), sha256: r.get("sha256"), status: r.get("status"),
            accept: r.get::<Option<String>, _>("accept").and_then(|a| serde_json::from_str(&a).ok()), created_at: r.get("created_at"), expires_at: r.get("expires_at"),
        }
    }
    /// The reference a run carries.
    fn reference(&self) -> CoriFile { CoriFile { id: self.id.clone(), name: self.name.clone(), size: self.size.max(0) as u64, content_type: self.content_type.clone(), sha256: self.sha256.clone().unwrap_or_default() } }
    /// The record as the API shows it: the reference, its status, when it was announced and, while no run holds it, when
    /// it goes.
    fn json(&self) -> Value {
        let mut v = self.reference().to_json();
        v["status"] = json!(self.status);
        v["created_at"] = json!(self.created_at);
        v["expires_at"] = json!(self.expires_at);
        if self.status != "ready" { v["sha256"] = Value::Null; }
        v
    }
    fn expired(&self) -> bool { self.expires_at.as_deref().is_some_and(|t| t < chrono::Utc::now().to_rfc3339().as_str()) }
}

async fn load(st: &AppState, app: &str, id: &str) -> Result<Option<FileRow>, ApiError> {
    if !cf::valid_id(id) { return Ok(None); }
    Ok(sqlx::query(SELECT_FILE).bind(id).bind(app).fetch_optional(&st.db).await?.map(|r| FileRow::of(&r)))
}

fn unknown_file() -> ApiError { ApiError(StatusCode::NOT_FOUND, "unknown file".into()) }

/// The largest file `p` takes here: its `max_size`, never more than the server's own limit.
fn limit_of(st: &AppState, p: &Parameter) -> u64 { p.max_size_bytes().map(|m| m.min(st.cfg.files_max_bytes)).unwrap_or(st.cfg.files_max_bytes) }

/// The stores a file or a run works on, for people.
fn scope_words(target: &str) -> String { if target == cori_store::LIVE { "the live app".into() } else { format!("release {}", target.trim_start_matches("sha256:").get(..12).unwrap_or(target)) } }

/// `POST /v1/apps/{app}/files`: announces the upload of a file for a parameter of the promoted version's workflow (a
/// release's own host: for that release). Body `{ workflow, parameter, name, size, content_type? }`; answers 201 with the
/// file (`status: pending`) and where its bytes go (`upload: { method, url }`).
pub(crate) async fn create_file_upload(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    // a release's host uploads for its release, as its runs run it
    if let Some(seq) = front.as_ref().and_then(|f| f.0.release) { return release_upload(st, app, format!("v{seq}"), headers, body).await; }
    let who = app_principal(&st, &headers, &app).await?;
    if who.is_agent() { return Err(ApiError(StatusCode::FORBIDDEN, format!("an agent session uploads files for releases only (`POST /v1/apps/{app}/versions/{{hash}}/files`)"))); }
    let (_, wfs, _) = promoted_version(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("app `{app}` has no promoted version (`cori promote {app} <version>`)")))?;
    announce(&st, &app, cori_store::LIVE, &wfs, &who, body).await
}

/// `POST /v1/apps/{app}/versions/{hash}/files`: the same for a deployed release (its preview: its sandbox stores), for
/// those who may try it.
pub(crate) async fn create_release_file_upload(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    match front.as_ref().map(|f| f.0.release) {
        None => {}
        Some(None) => return Err(ApiError(StatusCode::NOT_FOUND, "a release's files are uploaded on its own host".into())),
        Some(Some(seq)) => if resolve_version_hash(&st, &app, &hash).await? != resolve_version_hash(&st, &app, &format!("v{seq}")).await? { return Err(ApiError(StatusCode::NOT_FOUND, format!("this host runs release v{seq} only"))); },
    }
    release_upload(st, app, hash, headers, body).await
}

async fn release_upload(st: Arc<AppState>, app: String, hash: String, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    let who = release_principal(&st, &app, &headers).await?;
    let (h, wfs, _) = resolve_version(&st, &app, &hash).await?;
    announce(&st, &app, &h, &wfs, &who, body).await
}

/// The record of an upload to come: for a file parameter of a workflow `who` may start, within its limits.
async fn announce(st: &AppState, app: &str, target: &str, wfs: &[bundles::WorkflowEntry], who: &Principal, body: Option<Json<Value>>) -> Res {
    if who.sub == "anonymous" { return Err(ApiError(StatusCode::UNAUTHORIZED, "uploading a file needs a signed-in caller".into())); }
    // bundles in S3 and no bucket for files: refused, rather than kept on this replica's disk alone
    if let Some(why) = st.files.unavailable() { return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, why.into())); }
    let b = body.map(|b| b.0).unwrap_or(json!({}));
    let field = |k: &str| b.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let bad = |m: &str| ApiError(StatusCode::BAD_REQUEST, m.to_string());
    let wf = field("workflow").ok_or_else(|| bad("`workflow`: the workflow whose parameter the file is for"))?;
    let param = field("parameter").ok_or_else(|| bad("`parameter`: the file parameter the file is for"))?;
    let raw_name = field("name").ok_or_else(|| bad("`name`: the file's name"))?;
    let size = b.get("size").and_then(Value::as_u64).ok_or_else(|| bad("`size`: the file's size in bytes"))?;
    let entry = wfs.iter().find(|w| w.id == wf).ok_or(ApiError(StatusCode::NOT_FOUND, format!("workflow `{wf}` is not in {}", if target == cori_store::LIVE { "the promoted version" } else { "that version" })))?;
    endpoint_for(entry, who)?;
    let params = &entry.compiled.manifest.parameters;
    let p = params.iter().find(|p| p.name == param && p.is_file()).ok_or_else(|| {
        let names: Vec<&str> = params.iter().filter(|p| p.is_file()).map(|p| p.name.as_str()).collect();
        ApiError(StatusCode::BAD_REQUEST, format!("workflow `{wf}` has no file parameter `{param}`{}", if names.is_empty() { " (it takes no files)".to_string() } else { format!(" (its file parameters: {})", names.join(", ")) }))
    })?;
    let name = cf::clean_name(raw_name);
    let limit = limit_of(st, p);
    if size > limit { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("`{name}` is {}, more than the {} `{param}` takes", cf::human_size(size), cf::human_size(limit)))); }
    // a first look before the bytes say what they are: the type the browser gives, else the one the extension names
    let declared = field("content_type").map(|t| t.to_ascii_lowercase()).filter(|t| !t.ends_with("/*") && !t.starts_with('.') && cf::check_accept_entry(t).is_ok());
    let by_ext = name.rsplit_once('.').and_then(|(_, e)| cf::type_of_extension(e)).map(String::from);
    let seen: Vec<&str> = [declared.as_deref(), by_ext.as_deref()].into_iter().flatten().collect();
    if let Some(a) = p.accept.as_deref() && !seen.is_empty() && !seen.iter().any(|t| cf::accepts(Some(a), &name, t)) {
        return Err(ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("`{name}` is not of a type `{param}` takes ({})", a.join(", "))));
    }
    // the organization's storage and the app's (`CORI_MAX_STORAGE_BYTES`, the app's limit): files count with stores and bundles
    crate::usage::check_upload(st, app, size).await?;
    let id = format!("file_{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now();
    let expires = (now + chrono::Duration::hours(UPLOAD_TTL_HOURS)).to_rfc3339();
    let content_type = declared.or(by_ext).unwrap_or_else(|| "application/octet-stream".into());
    sqlx::query("INSERT INTO files (id, app, target, owner, workflow, parameter, name, content_type, size, sha256, status, accept, max_bytes, created_at, ready_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, 'pending', $10, $11, $12, NULL, $13)")
        .bind(&id).bind(app).bind(target).bind(&who.sub).bind(wf).bind(param).bind(&name).bind(&content_type).bind(size as i64)
        .bind(p.accept.as_ref().map(|a| json!(a).to_string())).bind(limit as i64).bind(now.to_rfc3339()).bind(&expires).execute(&st.db).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name, "size": size, "content_type": content_type, "status": "pending", "expires_at": expires,
        "upload": { "method": "PUT", "url": format!("/v1/apps/{app}/files/{id}/content") } }))).into_response())
}

/// `PUT /v1/apps/{app}/files/{id}/content`: the bytes of an announced upload, once, from who announced it. The body must
/// be the size announced; its type is read from its first bytes and checked against the parameter's `accept`. Answers
/// the file (`status: ready`, its `sha256`).
pub(crate) async fn put_file_content(InRealm(st): InRealm, Path((app, id)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, body: axum::body::Body) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    let f = load(&st, &app, &id).await?.ok_or_else(unknown_file)?;
    let scope = RunScope::of(&st, &app, front.as_ref().map(|f| &f.0)).await?;
    // another's upload, or one of stores this host or session does not reach: unknown here
    if !scope.sees(Some(&f.target)) || f.owner != who.sub || (who.is_agent() && f.target == cori_store::LIVE) { return Err(unknown_file()); }
    if f.status == "ready" { return Err(ApiError(StatusCode::CONFLICT, "this file's bytes were sent already: a file never changes (announce another one)".into())); }
    if f.expired() { return Err(ApiError(StatusCode::GONE, "this upload expired: announce it again".into())); }
    // one upload at a time: the bytes of a file are written once
    let claimed = sqlx::query("UPDATE files SET status = 'receiving' WHERE id = $1 AND status = 'pending'").bind(&id).execute(&st.db).await?.rows_affected();
    if claimed == 0 { return Err(ApiError(StatusCode::CONFLICT, "this file's bytes are being sent already".into())); }
    match receive(&st, &f, body).await {
        Ok((content_type, sha256)) => {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query("UPDATE files SET status = 'ready', content_type = $1, sha256 = $2, ready_at = $3 WHERE id = $4").bind(&content_type).bind(&sha256).bind(&now).bind(&id).execute(&st.db).await?;
            db::audit(&st.db, &who.sub, "file.upload", &id, json!({ "app": app, "name": f.name, "size": f.size, "content_type": content_type, "target": f.target })).await?;
            let mut done = f.clone();
            done.status = "ready".into(); done.content_type = content_type; done.sha256 = Some(sha256);
            Ok(Json(done.json()).into_response())
        }
        Err(e) => {
            // the announcement stays: the bytes may be sent again
            sqlx::query("UPDATE files SET status = 'pending' WHERE id = $1 AND status = 'receiving'").bind(&id).execute(&st.db).await.ok();
            Err(e)
        }
    }
}

/// Reads an upload's body (never more than announced), checks its type and keeps its bytes: their type and SHA-256.
async fn receive(st: &AppState, f: &FileRow, body: axum::body::Body) -> Result<(String, String), ApiError> {
    let want = f.size.max(0) as usize;
    let mut bytes: Vec<u8> = Vec::with_capacity(want);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("reading the body: {e}")))?;
        if bytes.len() + chunk.len() > want { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("the body is longer than the {want} bytes announced"))); }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != want { return Err(ApiError(StatusCode::BAD_REQUEST, format!("the body is {} bytes; the upload announced {want}", bytes.len()))); }
    let content_type = cf::sniff(&bytes[..bytes.len().min(8192)], &f.name);
    if !cf::accepts(f.accept.as_deref(), &f.name, &content_type) {
        return Err(ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("`{}` is {content_type}, which `{}` does not take ({})", f.name, f.parameter, f.accept.as_deref().unwrap_or_default().join(", "))));
    }
    let sha256 = cf::sha256_hex(&bytes);
    st.files.put(st.db.realm(), &f.id, &bytes).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok((content_type, sha256))
}

/// `GET /v1/apps/{app}/files/{id}`: the file's reference and status, for those who may read it.
pub(crate) async fn get_app_file(InRealm(st): InRealm, Path((app, id)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    let f = readable(&st, &app, &id, front.as_ref().map(|f| &f.0), &who).await?;
    Ok(Json(f.json()).into_response())
}

/// `GET /v1/apps/{app}/files/{id}/content`: the bytes, as an attachment that never runs on the host (`sandbox`).
pub(crate) async fn get_app_file_content(InRealm(st): InRealm, Path((app, id)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    let f = readable(&st, &app, &id, front.as_ref().map(|f| &f.0), &who).await?;
    if f.status != "ready" { return Err(ApiError(StatusCode::CONFLICT, "this file's bytes have not been received yet".into())); }
    let bytes = st.files.get(st.db.realm(), &id).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?.ok_or_else(unknown_file)?;
    use axum::http::header;
    Ok(([
        (header::CONTENT_TYPE, f.content_type.clone()),
        (header::CONTENT_DISPOSITION, attachment(&f.name)),
        (header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'".to_string()),
        (header::CACHE_CONTROL, "private, no-store".to_string()),
    ], bytes).into_response())
}

/// `Content-Disposition` for a download named `name`: an ASCII fallback and the UTF-8 name (RFC 6266, RFC 5987).
fn attachment(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", pct(name))
}

/// Every byte but the unreserved ones (`A-Z a-z 0-9 - . _ ~`) as `%XX`.
pub(crate) fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// A file `who` may read on this host, else unknown: who uploaded it, those who see every run of the app, the initiator of
/// a run that holds it, and the people that run's approvals are routed to. An agent session reads releases' files only.
async fn readable(st: &AppState, app: &str, id: &str, front: Option<&FrontHost>, who: &Principal) -> Result<FileRow, ApiError> {
    let f = load(st, app, id).await?.ok_or_else(unknown_file)?;
    if !RunScope::of(st, app, front).await?.sees(Some(&f.target)) || (who.is_agent() && f.target == cori_store::LIVE) { return Err(unknown_file()); }
    if f.owner == who.sub || may_see_all_runs(st, who, app).await? { return Ok(f); }
    let runs: Vec<(String, String)> = sqlx::query("SELECT r.id, r.user_sub FROM runs r WHERE r.app = $1 AND ('run:' || r.id) IN (SELECT holder FROM file_refs WHERE file_id = $2)").bind(app).bind(id).fetch_all(&st.db).await?
        .iter().map(|r| (r.get("id"), r.get("user_sub"))).collect();
    if runs.iter().any(|(_, owner)| *owner == who.sub) { return Ok(f); }
    for (run, owner) in &runs {
        let routed: Vec<String> = sqlx::query_scalar("SELECT to_role FROM approvals WHERE run_id = $1 AND app = $2").bind(run).bind(app).fetch_all(&st.db).await?;
        if routed.iter().any(|to| who.roles.contains(to) || (to == "user:initiator" && *owner == who.sub) || (to == "approver" && is_approver(st, who)) || who.is_admin()) { return Ok(f); }
    }
    Err(unknown_file())
}

/// `GET /v1/runs/{run}/files/{id}` (workers): the bytes of a file of the run's app and stores, for one of its steps.
pub(crate) async fn worker_run_file(InRealm(st): InRealm, Path((run, id)): Path<(String, String)>, via: crate::workers::Via, headers: HeaderMap) -> Res {
    let worker = crate::workers::authenticate(&st, &headers, &via).await?;
    let r = sqlx::query("SELECT app, version_hash, COALESCE(store_target, 'live') AS target FROM runs WHERE id = $1").bind(&run).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown run".into()))?;
    let (app, version, target): (String, String, String) = (r.get("app"), r.get("version_hash"), r.get("target"));
    if !crate::workers::serves_app(&worker, &crate::workers::app_connectors(&st, &app, &version).await?) { return Err(ApiError(StatusCode::FORBIDDEN, format!("{} does not serve app `{app}`", worker.label()))); }
    if !cf::valid_id(&id) { return Err(unknown_file()); }
    let f = sqlx::query(SELECT_FILE_FOR_RUN).bind(&id).bind(&app).bind(&target).fetch_optional(&st.db).await?.map(|r| FileRow::of(&r)).ok_or_else(unknown_file)?;
    let bytes = st.files.get(st.db.realm(), &id).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?.ok_or_else(unknown_file)?;
    Ok(([(axum::http::header::CONTENT_TYPE, f.content_type.clone()), (axum::http::HeaderName::from_static("x-cori-file-name"), pct(&f.name))], bytes).into_response())
}

/// A run's file parameters: each value (an id, or a reference) must name a file `who` uploaded for these stores
/// (`target`), received, unexpired and within the parameter's limits; it is replaced by the server's reference of it.
/// Returns the files' ids, for the run to hold (`hold_files`).
pub(crate) async fn resolve_file_params(st: &AppState, app: &str, target: &str, who: &Principal, params: &mut Value, parameters: &[Parameter]) -> Result<Vec<String>, ApiError> {
    let mut ids = Vec::new();
    for p in parameters.iter().filter(|p| p.is_file()) {
        let Some(v) = params.get(&p.name).filter(|v| !v.is_null()).cloned() else { continue };
        let many = p.ty == "files";
        let values = if many { v.as_array().cloned().unwrap_or_default() } else { vec![v] };
        let mut refs = Vec::with_capacity(values.len());
        for x in &values {
            let id = cf::id_of(x).unwrap_or_default();
            let f = match load(st, app, id).await? {
                Some(f) if f.owner == who.sub => f,
                _ => return Err(ApiError(StatusCode::BAD_REQUEST, format!("parameter `{}`: no file `{id}` of yours in app `{app}` (upload it first: `POST /v1/apps/{app}/files`)", p.name))),
            };
            if f.target != target { return Err(ApiError(StatusCode::BAD_REQUEST, format!("parameter `{}`: `{}` was uploaded for {}, and this run works on {}: upload it there", p.name, f.name, scope_words(&f.target), scope_words(target)))); }
            if f.status != "ready" { return Err(ApiError(StatusCode::CONFLICT, format!("parameter `{}`: the bytes of `{}` have not been received (`PUT /v1/apps/{app}/files/{id}/content`)", p.name, f.name))); }
            if f.expired() { return Err(ApiError(StatusCode::GONE, format!("parameter `{}`: the upload of `{}` expired; upload it again", p.name, f.name))); }
            let limit = limit_of(st, p);
            if f.size.max(0) as u64 > limit { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("parameter `{}`: `{}` is {}, more than the {} it takes", p.name, f.name, cf::human_size(f.size.max(0) as u64), cf::human_size(limit)))); }
            if !cf::accepts(p.accept.as_deref(), &f.name, &f.content_type) { return Err(ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("parameter `{}`: `{}` is {}, which it does not take ({})", p.name, f.name, f.content_type, p.accept.as_deref().unwrap_or_default().join(", ")))); }
            ids.push(f.id.clone());
            refs.push(f.reference().to_json());
        }
        params[&p.name] = if many { Value::Array(refs) } else { refs.into_iter().next().unwrap_or(Value::Null) };
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The run holds its files (`file_refs`): no sweep removes them while it is kept.
pub(crate) async fn hold_files(st: &AppState, run_id: &str, ids: &[String]) -> Result<(), ApiError> { Ok(hold(&st.db, run_id, ids).await?) }

async fn hold(db: &db::Db, run_id: &str, ids: &[String]) -> Result<(), sqlx::Error> {
    let (holder, now) = (format!("run:{run_id}"), chrono::Utc::now().to_rfc3339());
    for id in ids {
        sqlx::query("INSERT INTO file_refs (file_id, holder, at) VALUES ($1, $2, $3) ON CONFLICT ON CONSTRAINT file_refs_pkey DO NOTHING").bind(id).bind(&holder).bind(&now).execute(db).await?;
        sqlx::query("UPDATE files SET expires_at = NULL WHERE id = $1").bind(id).execute(db).await?;
    }
    Ok(())
}

/// A run that never started lets its files go: those nothing else holds expire as an unused upload does.
pub(crate) async fn release_files(st: &AppState, run_id: &str) {
    if let Err(e) = release(&st.db, run_id).await { tracing::warn!(run = %run_id, error = %e, "the files of a run that did not start were not released"); }
}

async fn release(db: &db::Db, run_id: &str) -> Result<(), sqlx::Error> {
    let holder = format!("run:{run_id}");
    let ids: Vec<String> = sqlx::query_scalar("SELECT file_id FROM file_refs WHERE holder = $1").bind(&holder).fetch_all(db).await?;
    sqlx::query("DELETE FROM file_refs WHERE holder = $1").bind(&holder).execute(db).await?;
    let expires = (chrono::Utc::now() + chrono::Duration::hours(UPLOAD_TTL_HOURS)).to_rfc3339();
    for id in ids { sqlx::query("UPDATE files SET expires_at = $1 WHERE id = $2 AND expires_at IS NULL AND NOT EXISTS (SELECT 1 FROM file_refs WHERE file_id = $2)").bind(&expires).bind(&id).execute(db).await?; }
    Ok(())
}

/// Runs gone (retention, an app's deletion) hold nothing any more.
pub(crate) async fn forget_run_files(st: &AppState, run_id: &str) -> Result<(), sqlx::Error> { forget(&st.db, run_id).await }

async fn forget(db: &db::Db, run_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM file_refs WHERE holder = $1").bind(format!("run:{run_id}")).execute(db).await.map(|_| ())
}

/// The background sweep: uploads no run used once they expired, and files nothing holds any more (their runs went with
/// the retention). Returns how many it removed.
pub(crate) async fn sweep_files(st: &AppState) -> Result<usize, ApiError> { sweep(&st.db, &st.files).await }

async fn sweep(db: &db::Db, store: &bundles::FileStore) -> Result<usize, ApiError> {
    let now = chrono::Utc::now().to_rfc3339();
    let ids: Vec<String> = sqlx::query_scalar("SELECT f.id FROM files f WHERE (f.expires_at IS NOT NULL AND f.expires_at < $1) OR (f.expires_at IS NULL AND NOT EXISTS (SELECT 1 FROM file_refs r WHERE r.file_id = f.id)) ORDER BY f.created_at LIMIT $2")
        .bind(&now).bind(SWEEP_BATCH).fetch_all(db).await?;
    let mut n = 0;
    for id in &ids { if remove(db, store, id).await { n += 1; } }
    Ok(n)
}

/// Removes a file: its bytes, then its record (kept when the bytes could not be removed: the next sweep tries again).
async fn remove_file(st: &AppState, id: &str) -> bool { remove(&st.db, &st.files, id).await }

async fn remove(db: &db::Db, store: &bundles::FileStore, id: &str) -> bool {
    if let Err(e) = store.remove(db.realm(), id).await { tracing::warn!(file = %id, error = %e, "file store: delete failed"); return false; }
    let r: Result<(), sqlx::Error> = async {
        sqlx::query("DELETE FROM file_refs WHERE file_id = $1").bind(id).execute(db).await?;
        sqlx::query("DELETE FROM files WHERE id = $1").bind(id).execute(db).await?;
        Ok(())
    }.await;
    r.map_err(|e| tracing::warn!(file = %id, error = %e, "a removed file's record stays")).is_ok()
}

/// The files of a release (its hash) or of a whole app (`None`), removed with it. Returns how many.
pub(crate) async fn remove_files_of(st: &AppState, app: &str, target: Option<&str>) -> usize {
    let ids: Result<Vec<String>, sqlx::Error> = match target {
        Some(t) => sqlx::query_scalar("SELECT id FROM files WHERE app = $1 AND target = $2").bind(app).bind(t).fetch_all(&st.db).await,
        None => sqlx::query_scalar("SELECT id FROM files WHERE app = $1").bind(app).fetch_all(&st.db).await,
    };
    let ids = match ids { Ok(i) => i, Err(e) => { tracing::warn!(app, error = %e, "files not listed for removal"); return 0 } };
    let mut n = 0;
    for id in &ids { if remove_file(st, id).await { n += 1; } }
    n
}

/// The realm's files' bytes, for its purge (its tables' rows go with the purge's own deletes).
pub(crate) async fn remove_all_file_bytes(st: &AppState) -> anyhow::Result<usize> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM files").fetch_all(&st.db).await?;
    for id in &ids { st.files.remove(st.db.realm(), id).await?; }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    /// An upload no run used goes once it expired; a fresh one stays; a file a run holds stays until that run goes (the
    /// retention), then goes with the next sweep; a run that never started lets its files expire again. Needs
    /// CORI_STORE_DATABASE_URL (skipped without).
    #[tokio::test]
    async fn unused_uploads_expire_and_files_go_with_the_runs_that_held_them() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let dir = std::env::temp_dir().join(format!("cori-files-sweep-{}", uuid::Uuid::new_v4().simple()));
        let store = crate::bundles::FileStore::local(dir.clone());
        let now = chrono::Utc::now();
        let add = |id: &'static str, expires: Option<chrono::DateTime<chrono::Utc>>| {
            let db = &db;
            async move {
                sqlx::query("INSERT INTO files (id, app, target, owner, workflow, parameter, name, content_type, size, sha256, status, accept, max_bytes, created_at, ready_at, expires_at) VALUES ($1, 'a', 'live', 'alice', 'w', 'p', 'x.pdf', 'application/pdf', 3, 'h', 'ready', NULL, 10, $2, $2, $3)")
                    .bind(id).bind(now.to_rfc3339()).bind(expires.map(|e| e.to_rfc3339())).execute(db).await.unwrap();
            }
        };
        add("file_old", Some(now - chrono::Duration::minutes(1))).await;
        add("file_fresh", Some(now + chrono::Duration::hours(24))).await;
        add("file_held", Some(now + chrono::Duration::hours(24))).await;
        add("file_back", Some(now + chrono::Duration::hours(24))).await;
        for id in ["file_old", "file_fresh", "file_held", "file_back"] { store.put(None, id, b"abc").await.unwrap(); }
        super::hold(&db, "r1", &["file_held".to_string()]).await.unwrap();
        super::hold(&db, "r2", &["file_back".to_string()]).await.unwrap();
        // a run that never started: its file is an unused upload again, not one held forever
        super::release(&db, "r2").await.unwrap();
        assert_eq!(super::sweep(&db, &store).await.map_err(|e| e.1).unwrap(), 1);
        let left = |db: &crate::db::Db| { let db = db.clone(); async move { let mut v: Vec<String> = sqlx::query_scalar("SELECT id FROM files").fetch_all(&db).await.unwrap(); v.sort(); v } };
        assert_eq!(left(&db).await, vec!["file_back", "file_fresh", "file_held"]);
        assert!(store.get(None, "file_old").await.unwrap().is_none() && store.get(None, "file_held").await.unwrap().is_some());
        let back: Option<String> = sqlx::query_scalar("SELECT expires_at FROM files WHERE id = 'file_back'").fetch_one(&db).await.unwrap();
        assert!(back.is_some(), "a released file expires again");
        // its run is gone (retention): nothing holds the file, the next sweep takes it
        super::forget(&db, "r1").await.unwrap();
        assert_eq!(super::sweep(&db, &store).await.map_err(|e| e.1).unwrap(), 1);
        assert_eq!(left(&db).await, vec!["file_back", "file_fresh"]);
        assert!(store.get(None, "file_held").await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_are_named_safely() {
        assert_eq!(super::attachment("facture été.pdf"), "attachment; filename=\"facture _t_.pdf\"; filename*=UTF-8''facture%20%C3%A9t%C3%A9.pdf");
        assert_eq!(super::attachment("a\"b\\c.txt"), "attachment; filename=\"a_b_c.txt\"; filename*=UTF-8''a%22b%5Cc.txt");
        assert_eq!(super::pct("A-z_0.~ /"), "A-z_0.~%20%2F");
    }
}

// SPDX-License-Identifier: Apache-2.0
//! Versions of an app: deploy (bundle, review, sandbox), promote, rollback, reseed, delete, and the release list.
use super::*;
use tracing::Instrument;

/// `GET /v1/apps/{app}/versions/{hash|promoted}/bundle`: the tar.zst `cori pull` extracts (owner, builders and admins only).
pub(crate) async fn get_bundle(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, Caller(who): Caller) -> Res {
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !is_owner_or_builder(&who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; ask to be added as a builder", rec.owner.as_deref().unwrap_or("nobody")))); }
    let h = if hash == "promoted" { rec.promoted.clone().ok_or(ApiError(StatusCode::NOT_FOUND, format!("app `{app}` has no promoted version; pass a version hash (`cori releases {app}`)")))? } else { resolve_version(&st, &app, &hash).await?.0 };
    let bytes = st.bundles.archive(st.db.realm(), &h).await?;
    Ok(([("content-type", "application/zstd"), ("x-cori-version", h.as_str())], bytes).into_response())
}

/// `POST /v1/apps/{app}/versions`: a `tar.zst` bundle (`cori deploy`), hashed by its manifest, compiled, reviewed, and given
/// a seeded sandbox per store. Headers: `X-Cori-Manifest-Hash` (checked), `X-Cori-Git-Commit/-Branch/-Dirty` (recorded).
/// Nothing here changes what users see: that is `POST …/versions/{hash}/promote`.
pub(crate) async fn deploy(InRealm(st): InRealm, Path(app): Path<String>, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>, Caller(who): Caller, headers: HeaderMap, body: Bytes) -> Res {
    let ct = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !ct.starts_with("application/zstd") && !ct.starts_with("application/x-tar") { return Err(ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE, "send the app folder as a tar.zst body (`cori deploy`), content-type application/zstd".into())); }
    let hdr = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(String::from);
    let git = GitMeta { commit: hdr("x-cori-git-commit"), branch: hdr("x-cori-git-branch"), dirty: hdr("x-cori-git-dirty").map(|d| if d == "true" { 1 } else { 0 }), message: q.get("message").map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) };
    let expect_hash = hdr("x-cori-manifest-hash");
    // the deploy runs to its end even when its caller is gone (a proxy's time limit, a closed laptop): its release is then
    // there for `cori releases`, and nothing it started is left half done
    let task = tokio::spawn(async move {
        let mut v = deploy_archive(&st, &who, &app, &body, expect_hash.as_deref(), git).await?;
        // the version may declare roles the identity provider does not know yet: one by one when the hook hears of roles
        // (now, or when the release goes live with CORI_ROLES_SYNC_AT=promote), else `app.created` again with them
        // (idempotent; the hook merges roles)
        if let Some(h) = v["hash"].as_str().map(String::from) {
            let roles = crate::client_hook::recorded_roles(&st, &h).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if crate::client_hook::role_events(&st) {
                if !st.cfg.identity.roles_at_promote {
                    match crate::client_hook::sync_roles(&st, &app, &roles).await { Ok(created) => v["roles_created"] = json!(created), Err(e) => v["identity_warning"] = json!(format!("{e:#}")) }
                }
            } else if let Err(e) = sync_client_with(&st, &app, &roles.into_keys().collect::<Vec<_>>()).await { v["client_warning"] = json!(e); }
        }
        Ok::<Value, ApiError>(v)
    }.instrument(tracing::Span::current()));
    let v = task.await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("the deploy stopped: {e}")))??;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

#[derive(Debug, Default, Clone)]
pub(crate) struct GitMeta { pub commit: Option<String>, pub branch: Option<String>, pub dirty: Option<i64>, /// the release message (`cori deploy -m`, else the commit subject)
    pub message: Option<String> }

/// Stores, compiles and reviews an uploaded archive as a new deployed version with its own sandbox stores.
/// What hosted runs ignore or refuse of a version's own connector declarations (`merge_registry_hosted`), so the deploy
/// says it: credentials of connectors the org registry does not declare, worker pins, and (on a strict server) the
/// programs and native queues an app declares itself.
pub(crate) fn credential_warnings(server: &crate::ServerConfig, cori_toml: &str, dir: &std::path::Path, rows: &[Value]) -> Vec<String> {
    let Ok(mut cfg) = super::runs::hosted_app_config(server, cori_toml, dir) else { return vec![] };
    let reg: BTreeMap<String, cori_engine::config::ConnectorConfig> = rows.iter().filter_map(|r| Some((r["name"].as_str()?.to_string(), serde_json::from_value(r["config"].clone()).ok()?))).collect();
    let o = cfg.merge_registry_hosted(&reg, server.strict_connectors());
    let mut out: Vec<String> = o.credentials.into_iter().map(|c| format!("connector `{c}`: its secret, audience and auth_env are ignored when hosted; credentials come from the org registry (`cori connectors add`)")).collect();
    out.extend(o.pins.into_iter().map(|c| format!("connector `{c}`: its `worker` pin is ignored when hosted; the org registry decides where a connector runs")));
    out.extend(o.refused.into_iter().map(|(_, why)| format!("{why}; runs that use it will not start")));
    out
}

pub(crate) async fn deploy_archive(st: &Arc<AppState>, who: &Principal, app: &str, body: &[u8], expect_hash: Option<&str>, git: GitMeta) -> Result<Value, ApiError> {
    require_scope(who, "versions:publish", Some(app))?;
    let rec = app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`: create it first (`cori apps create {app}`, `cori init … --app {app}`, or the create_app tool)")))?;
    if !is_owner_or_builder(who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; only its owner, its builders and admins may deploy versions", rec.owner.as_deref().unwrap_or("nobody")))); }
    crate::usage::check_bundle(st, body.len())?;
    let (dir, hash) = bundles::store_archive(st.bundles.data_dir(), st.db.realm(), body).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("bad bundle: {e}")))?;
    // one deploy of this content at a time, on every replica, until this one ends (its cleanup included)
    let _deploying = deploy_lock(st, app, &hash).await?;
    // the versions an app keeps and the realm's storage (`realm::Limits`): before the archive reaches the bundle store
    if let Err(e) = crate::usage::check_deploy(st, app, &hash, body.len()).await {
        // what this upload extracted goes, unless a version of this realm already has it (the folder is the realm's)
        if sqlx::query("SELECT 1 FROM versions WHERE hash = $1").bind(&hash).fetch_optional(&st.db).await?.is_none() {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_file(bundles::archive_path(st.bundles.data_dir(), st.db.realm(), &hash));
        }
        return Err(e);
    }
    // the archive is in the bundle store before the version exists, so every replica can serve it
    st.bundles.upload(st.db.realm(), &hash, body).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let recorded = record_version(st, who, app, &rec, body, &dir, &hash, expect_hash, git).await;
    // a version not recorded leaves nothing behind, in the bundle store either
    if recorded.is_err() && sqlx::query("SELECT 1 FROM versions WHERE hash = $1").bind(&hash).fetch_optional(&st.db).await?.is_none() { st.bundles.remove(st.db.realm(), &hash).await; }
    recorded
}

/// The lock of a deploy of `hash` (a session advisory lock, on a connection of its own that closing releases, however the
/// deploy ends). Two deploys of the same content at once (a deploy cut and started again while the first one still
/// runs) would number it twice, and one failing would remove what the other records: the second is refused, and its
/// caller waits for the release the first one records.
async fn deploy_lock(st: &AppState, app: &str, hash: &str) -> Result<sqlx::PgConnection, ApiError> {
    let mut conn = st.db.acquire().await?.detach();
    let key = format!("cori_deploy:{}:{app}:{hash}", st.db.realm().map(|r| r.as_str()).unwrap_or(""));
    let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))").bind(&key).fetch_one(&mut conn).await?;
    if !taken {
        let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or(hash);
        return Err(ApiError(StatusCode::CONFLICT, format!("version {short} of `{app}` is being deployed already, by another call: its release shows in `cori releases` when that deploy ends")));
    }
    Ok(conn)
}

/// The rest of a deploy, once the archive is in the bundle store: its checks, its sandbox, its record.
#[allow(clippy::too_many_arguments)]
async fn record_version(st: &Arc<AppState>, who: &Principal, app: &str, rec: &AppRecord, body: &[u8], dir: &std::path::Path, hash: &str, expect_hash: Option<&str>, git: GitMeta) -> Result<Value, ApiError> {
    let (dir, hash) = (dir.to_path_buf(), hash.to_string());
    if let Some(h) = expect_hash { if h != hash { return Err(ApiError(StatusCode::BAD_REQUEST, format!("manifest hash mismatch: client {h}, server {hash} (an old cli?)"))); } }
    let cori_toml = std::fs::read_to_string(dir.join("cori.toml")).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("cori.toml: {e}")))?;
    let declared = toml::from_str::<toml::Value>(&cori_toml).ok().and_then(|t| t.get("app")?.get("id")?.as_str().map(String::from));
    if declared.as_deref() != Some(app) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("cori.toml declares app `{}` but the upload targets `{app}`", declared.unwrap_or_default()))); }
    let dir_c = dir.clone();
    let registry_rows = registry_rows(&st).await?;
    let mut warnings = credential_warnings(&st.cfg, &cori_toml, &dir, &registry_rows);
    warnings.extend(cori_compiler::app_warnings(&dir));
    // the type check and the schema export load the steps' modules: in a sandbox under the app's lease, when there are some
    let deno = cori_engine::sandbox::Pool::global().map(|pool| std::sync::Arc::new(cori_engine::sandbox::SandboxChecks {
        pool, lease: cori_engine::sandbox::lease_of(st.realm().map(|r| r.as_str()), app), hash: hash.clone(), bundle: dir.clone(), connectors: registry_rows.clone(), handle: tokio::runtime::Handle::current(),
    }) as std::sync::Arc<dyn cori_compiler::DenoRunner>);
    let workflows = tokio::task::spawn_blocking(move || bundles::compile_all_with(&dir_c, registry_rows, deno)).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|errs| ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("check failed:\n{}", errs.join("\n"))))?;
    // the roles the version declares and uses: the identity provider's catalog of the app (`client_hook`)
    let ignore = crate::client_hook::deployment_roles(st);
    let roles = crate::client_hook::version_roles(&cori_toml, &workflows, &dir);
    let undeclared = roles.undeclared(&ignore);
    if st.cfg.identity.strict_roles && !undeclared.is_empty() {
        let list = undeclared.iter().map(|(r, at)| format!("`{r}` ({})", at.join(", "))).collect::<Vec<_>>().join("; ");
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("roles used but not declared in cori.toml's [auth] roles: {list}. This server requires every role declared (CORI_ROLES_STRICT), so that the organization's identity provider knows them all")));
    }
    warnings.extend(roles.warnings(&ignore));
    let version_roles = roles.all(&ignore);
    // the version's own sandbox: every store migrated from scratch and seeded; the live databases are untouched until promote
    let sandbox = crate::stores::provision_bundle(st, crate::stores::Target::sandbox(app, &hash), dir.clone()).await.map_err(|e| crate::stores::provisioning_error(e, "sandbox provisioning failed"))?;
    let promoted = promoted_version(st, app).await?;
    let review = bundles::review(&workflows, promoted.as_ref().map(|d| d.1.as_slice()));
    let status = if promoted.as_ref().map(|p| p.0 == hash).unwrap_or(false) { "promoted" } else { "deployed" };
    // a release is its content: deploying the same tree again is the same release (its number, message and first commit stay),
    // recorded in its history
    let existing: Option<i64> = sqlx::query("SELECT seq FROM versions WHERE hash = $1").bind(&hash).fetch_optional(&st.db).await?.and_then(|r| r.get::<Option<i64>, _>("seq"));
    let redeployed = existing.is_some();
    let seq = match existing { Some(n) => n, None => db::next_release_seq(&st.db, app).await? };
    sqlx::query("INSERT INTO versions (hash, app, status, created_at, created_by, workflows, review, cori_toml, git_commit, git_branch, git_dirty, created_kind, seq, message, bytes, roles) VALUES ($1, $2, 'deployed', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) ON CONFLICT ON CONSTRAINT versions_pkey DO UPDATE SET review = excluded.review, bytes = excluded.bytes, roles = excluded.roles, status = CASE WHEN versions.status = 'skipped' THEN 'deployed' ELSE versions.status END")
        .bind(&hash).bind(app).bind(chrono::Utc::now().to_rfc3339()).bind(&who.sub).bind(serde_json::to_string(&workflows).unwrap()).bind(review.to_string()).bind(&cori_toml).bind(&git.commit).bind(&git.branch).bind(git.dirty).bind(if who.is_agent() { "agent" } else { "human" }).bind(seq).bind(&git.message).bind(body.len() as i64).bind(serde_json::to_string(&version_roles).unwrap()).execute(&st.db).await?;
    crate::usage::add(&st.db, "deploys", app, 1.0).await;
    db::release_event(&st.db, app, &hash, if redeployed { "redeployed" } else { "deployed" }, &who.sub, json!({ "git_commit": git.commit, "git_branch": git.branch, "git_dirty": git.dirty.map(|d| d != 0), "message": git.message })).await?;
    db::audit(&st.db, &who.sub, "version.deploy", &format!("{app}@{hash}"), json!({ "workflows": workflows.iter().map(|w| &w.id).collect::<Vec<_>>(), "git_commit": git.commit, "kind": if who.is_agent() { "agent" } else { "human" }, "sandbox": sandbox.iter().filter_map(|s| s["store"].as_str()).collect::<Vec<_>>() })).await?;
    trim_sandboxes(st, app).await;
    let approved_by = current_approval(st, &hash, promoted.as_ref().map(|p| p.0.as_str()).unwrap_or("")).await?;
    let needs_approval = rec.protected || review.get("needs_approval").and_then(|b| b.as_bool()).unwrap_or(false);
    let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or("").to_string();
    Ok(json!({ "app": app, "hash": hash, "short": short, "seq": seq, "release": format!("v{seq}"), "warnings": warnings, "roles": version_roles, "redeployed": redeployed, "status": status, "review": review, "needs_approval": needs_approval, "protected": rec.protected, "approved_by": approved_by, "approver_roles": st.settings.approver_roles, "sandbox": sandbox, "preview_url": st.release_url(&app, seq), "promote": format!("cori promote {app} v{seq}"), "workflows": workflows.iter().map(|w| json!({ "id": w.id, "endpoint": w.compiled.manifest.endpoint, "schedule": w.compiled.manifest.schedule })).collect::<Vec<_>>() }))
}

/// Keeps at most `CORI_SANDBOXES_PER_APP` (default 5) sandboxes per app, newest first, never the promoted version's. A version
/// whose sandbox is gone gets it back, seeded again, on its next store call.
pub(crate) async fn trim_sandboxes(st: &Arc<AppState>, app: &str) {
    let keep = st.cfg.sandboxes_per_app;
    let promoted: Option<String> = sqlx::query("SELECT promoted_hash FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await.ok().flatten().and_then(|r| r.get::<Option<String>, _>("promoted_hash"));
    let Ok(rows) = sqlx::query("SELECT v.hash FROM versions v WHERE v.app = $1 AND EXISTS (SELECT 1 FROM store_instances s WHERE s.app = v.app AND s.target = v.hash) ORDER BY v.created_at DESC").bind(app).fetch_all(&st.db).await else { return };
    let mut kept = 0usize;
    for r in rows {
        let hash: String = r.get("hash");
        if promoted.as_deref() == Some(hash.as_str()) { continue; }
        if kept < keep { kept += 1; continue; }
        match crate::stores::drop_sandbox(st, crate::stores::Target::sandbox(app, &hash)).await {
            Ok(names) => tracing::info!(app, version = %hash, stores = ?names, "sandbox retired (kept the {keep} newest)"),
            Err(e) => tracing::warn!(app, version = %hash, error = %e, "sandbox retirement failed"),
        }
    }
}

pub(crate) async fn resolve_version_hash(st: &AppState, app: &str, hash: &str) -> Result<String, ApiError> { Ok(resolve_version(st, app, hash).await?.0) }

/// A screenshot of a version's front, rendered once by a headless browser when `CORI_CHROMIUM` names one (the preview
/// widget shows it where the host refuses to nest the front); cached under `<data_dir>/screenshots/` (the realm's).
pub(crate) fn screenshot_available(st: &AppState, app: &str, hash: &str) -> Option<std::path::PathBuf> {
    let p = screenshot_path(st, hash);
    if p.exists() { return Some(p); }
    let chromium = st.cfg.chromium.clone()?;
    std::fs::create_dir_all(p.parent().unwrap()).ok()?;
    // the release's preview link: a front's host sends it on to the release's own
    let url = format!("{}/apps/{app}/@{}/", st.front_origin(app), hash.trim_start_matches("sha256:").get(..12).unwrap_or(""));
    let ok = std::process::Command::new(chromium).args(["--headless=new", "--disable-gpu", "--no-sandbox", "--hide-scrollbars", "--window-size=1280,800", &format!("--screenshot={}", p.display()), &url]).output().ok().map(|o| o.status.success()).unwrap_or(false);
    if ok && p.exists() { Some(p) } else { None }
}

/// The promotion approval of a version, parked in the same inbox as workflow gates and store changes.
pub(crate) fn promote_approval_id(app: &str, hash: &str) -> String { format!("promote:{app}:{hash}") }

/// The hash of the version an app serves now ('' before its first promotion): what a change review and an approval hold against.
pub(crate) async fn promoted_hash(st: &AppState, app: &str) -> Result<String, ApiError> {
    Ok(sqlx::query("SELECT promoted_hash FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await?.and_then(|r| r.get::<Option<String>, _>("promoted_hash")).unwrap_or_default())
}

/// A version's approver, if the approval was given against the version promoted now: promoting another one moves the change
/// review, and the approval lapses with it.
pub(crate) async fn current_approval(st: &AppState, hash: &str, against: &str) -> Result<Option<String>, ApiError> {
    let row = sqlx::query("SELECT approved_by, approved_against FROM versions WHERE hash = $1").bind(hash).fetch_optional(&st.db).await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("approved_by").filter(|_| r.get::<Option<String>, _>("approved_against").as_deref() == Some(against))))
}

/// Recomputes a version's change review against `promoted` (the workflows live now) and stores it.
pub(crate) async fn refresh_review(st: &AppState, hash: &str, workflows: &[bundles::WorkflowEntry], promoted: Option<&[bundles::WorkflowEntry]>) -> Result<Value, ApiError> {
    let review = bundles::review(workflows, promoted);
    sqlx::query("UPDATE versions SET review = $1 WHERE hash = $2").bind(review.to_string()).bind(hash).execute(&st.db).await?;
    Ok(review)
}

/// After the promoted version changes: the versions still waiting get their review against it, and a promotion request parked
/// for one of them shows the new delta.
pub(crate) async fn refresh_reviews(st: &AppState, app: &str) -> Result<(), ApiError> {
    let promoted = promoted_version(st, app).await?;
    let rows = sqlx::query("SELECT hash, workflows FROM versions WHERE app = $1 AND status IN ('deployed', 'skipped')").bind(app).fetch_all(&st.db).await?;
    for r in rows {
        let hash: String = r.get("hash");
        let Ok(workflows) = serde_json::from_str::<Vec<bundles::WorkflowEntry>>(&r.get::<String, _>("workflows")) else { continue };
        let review = refresh_review(st, &hash, &workflows, promoted.as_ref().map(|p| p.1.as_slice())).await?;
        let aid = promote_approval_id(app, &hash);
        if let Some(p) = sqlx::query("SELECT payload FROM approvals WHERE id = $1 AND status = 'pending'").bind(&aid).fetch_optional(&st.db).await? {
            let mut payload: Value = serde_json::from_str(&p.get::<String, _>("payload")).unwrap_or(json!({}));
            payload["delta"] = review.get("delta").cloned().unwrap_or(Value::Null);
            sqlx::query("UPDATE approvals SET payload = $1 WHERE id = $2").bind(payload.to_string()).bind(&aid).execute(&st.db).await?;
        }
    }
    Ok(())
}

/// `v9`, or the short hash of a version deployed before releases were numbered.
pub(crate) fn release_label(seq: Option<i64>, hash: &str) -> String { seq.map(|n| format!("v{n}")).unwrap_or_else(|| hash.trim_start_matches("sha256:").get(..12).unwrap_or(hash).to_string()) }

pub(crate) async fn release_label_of(st: &AppState, hash: &str) -> String {
    let seq = sqlx::query("SELECT seq FROM versions WHERE hash = $1").bind(hash).fetch_optional(&st.db).await.ok().flatten().and_then(|r| r.get::<Option<i64>, _>("seq"));
    release_label(seq, hash)
}

/// A bundle's migrations as `store/file`.
pub(crate) async fn bundle_migrations(st: &AppState, hash: &str) -> Result<Vec<String>, ApiError> {
    Ok(crate::stores::read_store_dirs(&st.bundles.dir(st.db.realm(), hash).await?).into_iter().flat_map(|d| d.migrations.into_iter().map(move |(f, _)| format!("{}/{f}", d.name))).collect())
}

/// The migrations the live stores have run (`store/file`): every one a promotion applied, from the promotion history.
pub(crate) async fn live_migrations(st: &AppState, app: &str) -> Result<std::collections::BTreeSet<String>, ApiError> {
    let mut out = std::collections::BTreeSet::new();
    for r in sqlx::query("SELECT migrations FROM deployments WHERE app = $1").bind(app).fetch_all(&st.db).await? {
        if let Some(m) = r.get::<Option<String>, _>("migrations").and_then(|m| serde_json::from_str::<Vec<String>>(&m).ok()) { out.extend(m); }
    }
    Ok(out)
}

/// Whether a release was live before: going back to it needs no approval and runs no migration.
pub(crate) async fn was_live(st: &AppState, app: &str, hash: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query("SELECT 1 FROM deployments WHERE app = $1 AND hash = $2").bind(app).bind(hash).fetch_optional(&st.db).await?.is_some())
}

/// Makes a release live ("go live"; `cori promote`): it becomes what `/apps/{app}/` serves and the live stores
/// follow its migrations. Going back to a release that was live before is the same call: no approval (its
/// review passed when it went live) and no migration (the live stores already ran its own); the answer warns when the live
/// stores ran migrations that release does not have. Going forward is gated: a protected app always needs an approver,
/// otherwise a review that adds writes does. The review is recomputed against the release live now, and an approval counts
/// only if it was given against that same release. An approver approves in the same step; anyone else's request is parked
/// as `promote:<app>:<hash>` for an approver, and the answer says so. Older releases still waiting are passed over
/// (`skipped`), their parked requests closed.
pub(crate) async fn do_promote(st: &Arc<AppState>, app: &str, hash: &str, who: &Principal) -> Result<Value, ApiError> {
    require_human(who, "going live")?;
    let rec = app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !is_owner_or_builder(who, &rec) && !is_approver(st, who) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; its owner, builders, approvers and admins make releases live", rec.owner.as_deref().unwrap_or("nobody")))); }
    let (hash, workflows, _) = resolve_version(st, app, hash).await?;
    let row = sqlx::query("SELECT created_at, seq FROM versions WHERE hash = $1 AND app = $2").bind(&hash).bind(app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown version".into()))?;
    let (created_at, seq): (String, Option<i64>) = (row.get("created_at"), row.get("seq"));
    let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or("").to_string();
    let label = release_label(seq, &hash);
    let current = promoted_version(st, app).await?;
    let current_hash = current.as_ref().map(|c| c.0.clone()).unwrap_or_default();
    let going_back = current_hash != hash && was_live(st, app, &hash).await?;
    let review = refresh_review(st, &hash, &workflows, current.as_ref().map(|c| c.1.as_slice())).await?;
    let adds_writes = review.get("needs_approval").and_then(|b| b.as_bool()).unwrap_or(false);
    // the roles the release declares, for the identity provider: sent now when they follow promotions, and, when the server
    // says so, confirmed by it before the release goes live
    let identity_roles = if crate::client_hook::role_events(st) { crate::client_hook::recorded_roles(st, &hash).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? } else { BTreeMap::new() };
    if crate::client_hook::role_events(st) && current_hash != hash {
        if st.cfg.identity.roles_at_promote { crate::client_hook::sync_roles(st, app, &identity_roles).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?; }
        if st.cfg.identity.promote_needs_confirmed_roles {
            let missing = crate::client_hook::unconfirmed(st, app, &identity_roles.keys().cloned().collect::<Vec<_>>()).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if !missing.is_empty() { return Err(ApiError(StatusCode::CONFLICT, format!("{label} declares roles the identity provider has not confirmed yet ({}): it goes live once they are (CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES; `cori identity catalog`)", missing.join(", ")))); }
        }
    }
    // going live with the release already live changes nothing users see (it only retries pending migrations): no gate
    if current_hash != hash && !going_back && (rec.protected || adds_writes) && current_approval(st, &hash, &current_hash).await?.is_none() {
        if is_approver(st, who) {
            sqlx::query("UPDATE versions SET approved_by = $1, approved_against = $2 WHERE hash = $3").bind(&who.sub).bind(&current_hash).bind(&hash).execute(&st.db).await?;
            db::audit(&st.db, &who.sub, "version.approve", &format!("{app}@{hash}"), review.get("delta").cloned().unwrap_or(Value::Null)).await?;
            db::release_event(&st.db, app, &hash, "approved", &who.sub, json!({ "against": if current_hash.is_empty() { None } else { Some(release_label_of(st, &current_hash).await) } })).await?;
        } else {
            let aid = promote_approval_id(app, &hash);
            let reason = if rec.protected { "protected app" } else { "new writes" };
            let payload = json!({ "kind": "promotion", "app": app, "version": hash, "short": short, "release": label, "requested_by": who.sub, "reason": reason, "delta": review.get("delta").cloned().unwrap_or(Value::Null) });
            // a request declined or passed over earlier opens again
            sqlx::query("INSERT INTO approvals (id, run_id, app, step, to_role, payload, status, created_at) VALUES ($1, $2, $3, 'promote', 'approver', $4, 'pending', $5) ON CONFLICT ON CONSTRAINT approvals_pkey DO UPDATE SET payload = excluded.payload, status = 'pending', decided_by = NULL, decided_at = NULL, created_at = excluded.created_at")
                .bind(&aid).bind(format!("promote:{app}")).bind(app).bind(payload.to_string()).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
            db::audit(&st.db, &who.sub, "version.promote.requested", &format!("{app}@{hash}"), json!({ "reason": reason })).await?;
            db::release_event(&st.db, app, &hash, "go_live_requested", &who.sub, json!({ "reason": reason })).await?;
            return Ok(json!({ "app": app, "version": hash, "short": short, "release": label, "status": "pending_approval", "approval_id": aid, "reason": reason, "approver_roles": st.settings.approver_roles, "next": format!("an approver accepts it in the console inbox, or runs `cori promote {app} {label}`; {label} goes live then") }));
        }
    }
    // what the live stores ran that this release does not know (going back after a migration): its code must cope with it
    let own: std::collections::BTreeSet<String> = bundle_migrations(st, &hash).await?.into_iter().collect();
    let schema_ahead: Vec<String> = live_migrations(st, app).await?.into_iter().filter(|m| !own.contains(m)).collect();
    // the irreversible step going forward: the release's pending migrations on the live databases, before the pointer moves
    let migrations: Vec<String> = if going_back { vec![] } else {
        let report = crate::stores::provision_bundle(st, crate::stores::Target::live(app), st.bundles.dir(st.db.realm(), &hash).await?).await.map_err(|e| crate::stores::provisioning_error(e, &format!("live store migration failed, {label} did not go live")))?;
        report.iter().flat_map(|r| { let s = r["store"].as_str().unwrap_or("").to_string(); r["migrations_applied"].as_array().cloned().unwrap_or_default().into_iter().filter_map(move |m| m.as_str().map(|m| format!("{s}/{m}"))) }).collect()
    };
    let previous: Option<String> = rec.promoted.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let skipped: Vec<String> = if going_back { vec![] } else { sqlx::query("SELECT hash FROM versions WHERE app = $1 AND status = 'deployed' AND created_at < $2 AND hash <> $3").bind(app).bind(&created_at).bind(&hash).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("hash")).collect() };
    for h in &skipped {
        sqlx::query("UPDATE versions SET status = 'skipped' WHERE hash = $1").bind(h).execute(&st.db).await?;
        sqlx::query("UPDATE approvals SET status = 'superseded', decided_by = $1, decided_at = $2 WHERE id = $3 AND status = 'pending'").bind(&who.sub).bind(&now).bind(promote_approval_id(app, h)).execute(&st.db).await?;
    }
    sqlx::query("UPDATE versions SET status = 'superseded' WHERE app = $1 AND status = 'promoted'").bind(app).execute(&st.db).await?;
    sqlx::query("UPDATE versions SET status = 'promoted' WHERE hash = $1").bind(&hash).execute(&st.db).await?;
    sqlx::query("UPDATE apps SET promoted_hash = $1 WHERE id = $2").bind(&hash).bind(app).execute(&st.db).await?;
    sqlx::query("INSERT INTO deployments (id, app, hash, previous_hash, by_sub, kind, at, migrations) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)").bind(uuid::Uuid::new_v4().to_string()).bind(app).bind(&hash).bind(&previous).bind(&who.sub).bind(if going_back { "rollback" } else { "promote" }).bind(&now).bind(serde_json::to_string(&migrations).unwrap()).execute(&st.db).await?;
    sqlx::query("UPDATE approvals SET status = 'decided', decided_by = $1, decided_at = $2 WHERE id = $3 AND status = 'pending'").bind(&who.sub).bind(&now).bind(promote_approval_id(app, &hash)).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, if going_back { "version.rollback" } else { "version.promote" }, &format!("{app}@{hash}"), json!({ "previous": previous, "migrations": migrations, "skipped": skipped, "schema_ahead": schema_ahead })).await?;
    let previous_label = match &previous { Some(p) if p != &hash => Some(release_label_of(st, p).await), _ => None };
    db::release_event(&st.db, app, &hash, "went_live", &who.sub, json!({ "from": previous_label, "back": going_back, "migrations": migrations })).await?;
    if let (Some(p), Some(_)) = (&previous, &previous_label) { db::release_event(&st.db, app, p, "replaced", &who.sub, json!({ "by": label })).await?; }
    refresh_reviews(st, app).await?;
    crate::schedules::sync_app(st, app).await;
    Ok(json!({ "app": app, "version": hash, "short": short, "release": label, "status": "promoted", "went_back": going_back, "previous": previous, "previous_release": previous_label, "migrations": migrations, "schema_ahead": schema_ahead, "skipped": skipped, "url": st.front_url(&app) }))
}

/// `POST /v1/apps/{app}/versions/{hash}/promote` (`cori promote`).
pub(crate) async fn promote(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, Caller(who): Caller) -> Res {
    let v = do_promote(&st, &app, &hash, &who).await?;
    let code = if v["status"] == "pending_approval" { StatusCode::ACCEPTED } else { StatusCode::OK };
    Ok((code, Json(v)).into_response())
}

/// `POST /v1/apps/{app}/versions/{hash}/reseed`: drops and recreates the version's sandbox databases (owner, builders, approvers; agents too).
pub(crate) async fn reseed(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, Caller(who): Caller) -> Res {
    require_scope(&who, "runs:preview", Some(&app))?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !(is_owner_or_builder(&who, &rec) || is_approver(&st, &who)) { return Err(ApiError(StatusCode::FORBIDDEN, format!("reseeding a sandbox needs the app's owner, a builder, an approver or an admin (you are `{}` with roles {:?})", who.sub, who.roles))); }
    let (hash, _, _) = resolve_version(&st, &app, &hash).await?;
    let at = crate::stores::Target::sandbox(&app, &hash);
    let dropped = crate::stores::drop_sandbox(&st, at.clone()).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let report = crate::stores::provision_bundle(&st, at, st.bundles.dir(st.db.realm(), &hash).await?).await.map_err(|e| crate::stores::provisioning_error(e, "sandbox provisioning failed"))?;
    db::audit(&st.db, &who.sub, "sandbox.reseed", &format!("{app}@{hash}"), json!({ "dropped": dropped })).await?;
    db::release_event(&st.db, &app, &hash, "preview_reset", &who.sub, json!({ "stores": dropped })).await?;
    Ok(Json(json!({ "app": app, "version": hash, "dropped": dropped, "sandbox": report })).into_response())
}

/// `POST /v1/apps/{app}/stores/reset`, dev auth only (`cori up`): drops the live store databases of the app and provisions
/// them again from the promoted version's migrations and seed files, the way a sandbox is built (the one place seeds reach
/// a live database: nothing else seeds one). Owner, builders and admins; audited. Nothing of the sort
/// exists outside dev mode.
pub(crate) async fn reset_live_stores(InRealm(st): InRealm, Path(app): Path<String>, headers: HeaderMap) -> Res {
    if !matches!(st.cfg.auth, crate::auth::AuthMode::Dev) { return Err(ApiError(StatusCode::NOT_FOUND, "resetting the live stores only exists under dev auth (`cori up`)".into())); }
    let who = principal(&st, &headers).await?;
    require_human(&who, "resetting the live stores")?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !is_owner_or_builder(&who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, "resetting the live stores needs the app's owner, a builder or an admin".into())); }
    let hash = rec.promoted.clone().ok_or(ApiError(StatusCode::CONFLICT, format!("`{app}` has no promoted version, so no live store")))?;
    let dropped = crate::stores::drop_live(&st, &app).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let report = crate::stores::provision_live_seeded(&st, &app, st.bundles.dir(st.db.realm(), &hash).await?).await.map_err(|e| crate::stores::provisioning_error(e, "live store provisioning failed"))?;
    db::audit(&st.db, &who.sub, "stores.reset_live", &app, json!({ "version": hash, "dropped": dropped })).await?;
    Ok(Json(json!({ "app": app, "version": hash, "dropped": dropped, "live": report })).into_response())
}

/// Stops runs that are still going (Temporal terminate, best effort: a run Temporal no longer knows is only marked) and marks
/// them failed. Returns the ids terminated.
pub(crate) async fn terminate_runs(st: &AppState, ids: &[String], reason: &str) -> Vec<String> {
    for id in ids {
        let handle = st.temporal.get_workflow_handle::<CoriWorkflow>(id.clone());
        if let Err(e) = handle.terminate(WorkflowTerminateOptions::builder().reason(reason.to_string()).build()).await { tracing::warn!(run = %id, error = %e, "terminate failed; the run is marked failed anyway"); }
        sqlx::query("UPDATE runs SET status = 'failed', ended_at = $1, error = $2 WHERE id = $3 AND status = 'running'").bind(chrono::Utc::now().to_rfc3339()).bind(format!("terminated: {reason}")).bind(id).execute(&st.db).await.ok();
    }
    ids.to_vec()
}

/// The files a version leaves on the server: the extracted bundle, its archive, its screenshot.
pub(crate) async fn remove_version_files(st: &AppState, hash: &str) {
    st.bundles.remove(st.db.realm(), hash).await;
    let _ = std::fs::remove_file(screenshot_path(st, hash));
}

fn screenshot_path(st: &AppState, hash: &str) -> std::path::PathBuf {
    st.cfg.data_dir.join(crate::names::realm_folder(st.db.realm(), "screenshots")).join(format!("{}.png", hash.replace(':', "_")))
}

/// `DELETE /v1/apps/{app}/versions/{hash}[?force=true]` (`cori undeploy`): a deployed version goes away with its sandbox
/// databases, its bundle and its preview url. Never the promoted version (promote or roll back to another one first). Runs
/// still going on it refuse the deletion unless `force` terminates them. Pending promotion requests and store changes parked
/// on its sandbox are withdrawn. The audit, the run history and the promotion history keep its hash; a rollback skips it.
/// Owner, builders and admins; a human action.
pub(crate) async fn delete_version(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>, Caller(who): Caller) -> Res {
    require_human(&who, "deleting a version")?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if !is_owner_or_builder(&who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is owned by {}; only its owner, its builders and admins may delete versions", rec.owner.as_deref().unwrap_or("nobody")))); }
    let (hash, _, _) = resolve_version(&st, &app, &hash).await?;
    let short = release_label_of(&st, &hash).await;
    if rec.promoted.as_deref() == Some(hash.as_str()) { return Err(ApiError(StatusCode::CONFLICT, format!("{short} is live: make another release live first (deleting the app is `cori apps delete {app}`)"))); }
    let running: Vec<String> = sqlx::query("SELECT id FROM runs WHERE app = $1 AND version_hash = $2 AND status = 'running'").bind(&app).bind(&hash).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("id")).collect();
    let force = q.get("force").map(|v| v == "true" || v == "1").unwrap_or(false);
    if !running.is_empty() && !force { return Err(ApiError(StatusCode::CONFLICT, format!("{} run(s) of {short} still running ({}): wait for them, or delete with --force to terminate them", running.len(), running.join(", ")))); }
    let terminated = terminate_runs(&st, &running, &format!("release {short} deleted")).await;
    // the databases first: when dropping fails nothing else is touched and the deletion can be retried
    let has_sandbox = sqlx::query("SELECT 1 FROM store_instances WHERE app = $1 AND target = $2").bind(&app).bind(&hash).fetch_optional(&st.db).await?.is_some();
    let dropped = if has_sandbox { crate::stores::drop_sandbox(&st, crate::stores::Target::sandbox(&app, &hash)).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("sandbox not dropped, version kept: {e}")))? } else { vec![] };
    let now = chrono::Utc::now().to_rfc3339();
    let store_prefix = format!("store:{app}:{}:", hash.trim_start_matches("sha256:"));
    sqlx::query("UPDATE approvals SET status = 'withdrawn', decided_by = $1, decided_at = $2 WHERE app = $3 AND status = 'pending' AND (id = $4 OR substr(id, 1, $5) = $6)").bind(&who.sub).bind(&now).bind(&app).bind(promote_approval_id(&app, &hash)).bind(store_prefix.len() as i32).bind(&store_prefix).execute(&st.db).await?;
    sqlx::query("DELETE FROM versions WHERE hash = $1 AND app = $2").bind(&hash).bind(&app).execute(&st.db).await?;
    sqlx::query("DELETE FROM release_events WHERE hash = $1 AND app = $2").bind(&hash).bind(&app).execute(&st.db).await?;
    remove_version_files(&st, &hash).await;
    // the files its preview was given go with it, as its sandbox did
    remove_files_of(&st, &app, Some(&hash)).await;
    // a role only this version declared starts its way out of the identity provider's catalog
    if let Err(e) = crate::client_hook::sweep_app(&st, &app).await { tracing::warn!(app = %app, error = %format!("{e:#}"), "roles not swept"); }
    db::audit(&st.db, &who.sub, "version.delete", &format!("{app}@{hash}"), json!({ "sandbox_dropped": dropped, "terminated_runs": terminated })).await?;
    Ok(Json(json!({ "app": app, "version": hash, "short": short, "deleted": true, "sandbox_dropped": dropped, "terminated_runs": terminated })).into_response())
}

/// `POST /v1/apps/{app}/rollback` (`cori rollback`): going live with the release that was live before this one (the last one
/// of the history still deployed). The same path as any go-live (`do_promote`): a release that was live before goes back
/// without approval and without migration. Going to any other release is `promote`.
pub(crate) async fn rollback(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    let current: Option<String> = sqlx::query("SELECT promoted_hash FROM apps WHERE id = $1").bind(&app).fetch_optional(&st.db).await?.and_then(|r| r.get::<Option<String>, _>("promoted_hash"));
    let target: String = sqlx::query("SELECT d.hash FROM deployments d JOIN versions v ON v.hash = d.hash WHERE d.app = $1 AND d.hash <> $2 ORDER BY d.at DESC LIMIT 1").bind(&app).bind(current.clone().unwrap_or_default()).fetch_optional(&st.db).await?.map(|r| r.get::<String, _>("hash")).ok_or(ApiError(StatusCode::CONFLICT, "no earlier live release to go back to".into()))?;
    let mut v = do_promote(&st, &app, &target, &who).await?;
    let code = if v["status"] == "pending_approval" { StatusCode::ACCEPTED } else { StatusCode::OK };
    v["promoted"] = json!(target);
    v["rolled_back_from"] = json!(current);
    Ok((code, Json(v)).into_response())
}

pub(crate) async fn list_promotions(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, is_approver(&st, &who) || who.is_operator(), "the deployments").await?;
    let rows = sqlx::query("SELECT hash, previous_hash, by_sub, kind, at, migrations FROM deployments WHERE app = $1 ORDER BY at DESC LIMIT 50").bind(&app).fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "hash": r.get::<String, _>("hash"), "previous": r.get::<Option<String>, _>("previous_hash"), "by": r.get::<String, _>("by_sub"), "kind": r.get::<String, _>("kind"), "at": r.get::<String, _>("at"), "migrations": r.get::<Option<String>, _>("migrations").and_then(|m| serde_json::from_str::<Value>(&m).ok()).unwrap_or(json!([])) })).collect::<Vec<_>>()).into_response())
}

/// A version's front and workflows before it is promoted: `/apps/{app}/@{hash}/` serves its `dist/`, and
/// `POST /v1/apps/{app}/versions/{hash}/wf/{wf}/runs` runs its workflows (builders and approvers only, anyone under dev
/// auth; the endpoint's own `allow` still applies). Hash prefixes (8+ chars) are accepted.
pub(crate) async fn resolve_version(st: &AppState, app: &str, hash: &str) -> Result<(String, Vec<bundles::WorkflowEntry>, String), ApiError> {
    // `v9` names a release by its number
    if let Some(n) = hash.strip_prefix('v').and_then(|n| n.parse::<i64>().ok()) {
        let row = sqlx::query("SELECT hash, workflows, cori_toml FROM versions WHERE app = $1 AND seq = $2").bind(app).bind(n).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("`{app}` has no release v{n} (`cori releases {app}`)")))?;
        let h: String = row.get("hash"); let wf: String = row.get("workflows"); let toml: String = row.get("cori_toml");
        return Ok((h, serde_json::from_str(&wf).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?, toml));
    }
    let like = format!("{}%", hash.trim_start_matches("sha256:"));
    let row = sqlx::query("SELECT hash, workflows, cori_toml FROM versions WHERE app = $1 AND (hash = $2 OR hash LIKE $3 OR hash LIKE $4) ORDER BY created_at DESC LIMIT 1").bind(app).bind(hash).bind(&like).bind(format!("sha256:{like}")).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown version {hash}")))?;
    let h: String = row.get("hash"); let wf: String = row.get("workflows"); let toml: String = row.get("cori_toml");
    Ok((h, serde_json::from_str(&wf).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?, toml))
}

pub(crate) async fn promoted_version(st: &AppState, app: &str) -> Result<Option<(String, Vec<bundles::WorkflowEntry>, String)>, ApiError> {
    let row = sqlx::query("SELECT v.hash, v.workflows, v.cori_toml FROM apps a JOIN versions v ON v.hash = a.promoted_hash WHERE a.id = $1").bind(app).fetch_optional(&st.db).await?;
    Ok(match row { Some(r) => { let hash: String = r.get("hash"); let wf: String = r.get("workflows"); let toml: String = r.get("cori_toml"); Some((hash, serde_json::from_str(&wf).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?, toml)) } None => None })
}

/// Releases newest first (`cori releases`, the console's Releases tab). Each carries its number and message, its `section`
/// (`live`, `newer` than live, `older`), when it was live, one `gate` (`live`, `ready`, `needs_approval` with the reason,
/// `requested` by whom, `approved` by whom and against which release), what going live does for the caller (`go_live`:
/// `now`, `approve` = approve and go live now, `request`, `pending`, or null for the live one), the migrations it would run
/// on the live stores and those the live stores ran that it lacks (`schema_ahead`), its preview and its review delta.
/// The fields of the older listing (`status`, `action`, `approved_by`, `needs_approval`, `sandbox`…) stay for scripts.
pub(crate) async fn list_versions(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    let approver = is_approver(&st, &who) && !who.is_agent();
    let protected = require_builder(&st, &who, &app, approver, "the releases").await?.protected;
    let current = promoted_hash(&st, &app).await?;
    let rows = sqlx::query("SELECT hash, status, created_at, created_by, approved_by, review, git_commit, git_dirty, approved_against, seq, message, git_branch FROM versions WHERE app = $1 ORDER BY COALESCE(seq, 0) DESC, created_at DESC").bind(&app).fetch_all(&st.db).await?;
    let sandboxes = sqlx::query("SELECT target, name FROM store_instances WHERE app = $1 AND target NOT IN ('live', 'local') ORDER BY name").bind(&app).fetch_all(&st.db).await?;
    let pending: BTreeMap<String, String> = sqlx::query("SELECT id, payload FROM approvals WHERE app = $1 AND step = 'promote' AND status = 'pending'").bind(&app).fetch_all(&st.db).await?.iter()
        .map(|r| (r.get::<String, _>("id"), serde_json::from_str::<Value>(&r.get::<String, _>("payload")).ok().and_then(|p| p["requested_by"].as_str().map(String::from)).unwrap_or_default())).collect();
    // live periods from the promotion history (oldest first): each go-live ends the previous one
    let history = sqlx::query("SELECT hash, at FROM deployments WHERE app = $1 ORDER BY at").bind(&app).fetch_all(&st.db).await?;
    let mut spans: Vec<(String, String)> = Vec::new();   // (hash, from), a go-live of the release already live folded in
    for h in &history { let hash: String = h.get("hash"); if spans.last().map(|l| l.0 == hash) != Some(true) { spans.push((hash, h.get("at"))); } }
    let mut periods: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (i, (hash, from)) in spans.iter().enumerate() { periods.entry(hash.clone()).or_default().push(json!({ "from": from, "until": spans.get(i + 1).map(|n| n.1.clone()) })); }
    let applied = live_migrations(&st, &app).await?;
    let live_seq: Option<i64> = rows.iter().find(|r| r.get::<String, _>("hash") == current).and_then(|r| r.get::<Option<i64>, _>("seq"));
    let labels: BTreeMap<String, String> = rows.iter().map(|r| (r.get::<String, _>("hash"), release_label(r.get("seq"), &r.get::<String, _>("hash")))).collect();
    let mut out = Vec::new();
    for r in &rows {
        let hash: String = r.get("hash");
        let status: String = r.get("status");
        let seq: Option<i64> = r.get("seq");
        let label = release_label(seq, &hash);
        let review: Value = serde_json::from_str(&r.get::<String, _>("review")).unwrap_or(Value::Null);
        let adds_writes = review.get("needs_approval").and_then(|b| b.as_bool()).unwrap_or(false);
        // an approval given against another live release has lapsed (reviews are recomputed at each go-live)
        let approved_by = r.get::<Option<String>, _>("approved_by").filter(|_| r.get::<Option<String>, _>("approved_against").as_deref() == Some(current.as_str()));
        let requested_by = pending.get(&promote_approval_id(&app, &hash)).cloned();
        let is_live = hash == current;
        let live_before = periods.contains_key(&hash);
        let section = if is_live { "live" } else if current.is_empty() || seq.unwrap_or(0) > live_seq.unwrap_or(0) { "newer" } else { "older" };
        let writes: Vec<Value> = review["delta"]["new_or_changed_writes"].as_array().cloned().unwrap_or_default();
        let gate = if is_live { json!({ "state": "live" }) }
            else if live_before { json!({ "state": "ready", "reason": "was live before: goes back without approval" }) }
            else if let Some(by) = &approved_by { json!({ "state": "approved", "by": by, "against": labels.get(&current).cloned() }) }
            else if !(protected || adds_writes) { json!({ "state": "ready", "reason": "no new write" }) }
            else if let Some(by) = &requested_by { json!({ "state": "requested", "by": by, "reason": if protected { "protected app" } else { "new writes" } }) }
            else { json!({ "state": "needs_approval", "reason": if protected { "protected app".to_string() } else { format!("new writes: {}", writes.iter().filter_map(|w| w.as_str()).map(|w| w.split(':').next().unwrap_or(w)).collect::<Vec<_>>().join(", ")) } }) };
        let go_live = match gate["state"].as_str().unwrap_or("") { "live" => Value::Null, "ready" | "approved" => json!("now"), _ if approver => json!("approve"), "requested" => json!("pending"), _ => json!("request") };
        let own = bundle_migrations(&st, &hash).await?;
        let pending_migrations: Vec<&String> = if live_before || is_live { vec![] } else { own.iter().filter(|m| !applied.contains(*m)).collect() };
        let schema_ahead: Vec<&String> = applied.iter().filter(|m| !own.contains(*m)).collect();
        let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or("").to_string();
        // the release at the root of its own host (on its sandbox), whether or not it is live
        let preview = match seq { Some(n) => st.release_url(&app, n), None => format!("{}/apps/{app}/@{short}/", st.front_origin(&app)) };
        let stores: Vec<String> = sandboxes.iter().filter(|s| s.get::<String, _>("target") == hash).map(|s| s.get::<String, _>("name")).collect();
        let action = if status == "promoted" { Value::Null } else if gate["state"] == "ready" || gate["state"] == "approved" { json!("promote") } else if approver { json!("approve_and_promote") } else if requested_by.is_some() { json!("pending") } else { json!("request") };
        out.push(json!({ "hash": hash, "short": short, "seq": seq, "release": label, "message": r.get::<Option<String>, _>("message"), "section": section, "live": is_live, "was_live": live_before, "live_periods": periods.get(&hash).cloned().unwrap_or_default(),
            "gate": gate, "go_live": go_live, "pending_migrations": pending_migrations, "schema_ahead": schema_ahead, "delta": review.get("delta").cloned().unwrap_or(Value::Null),
            "preview": { "url": preview, "stores": stores },
            "created_at": r.get::<String, _>("created_at"), "created_by": r.get::<String, _>("created_by"), "git_commit": r.get::<Option<String>, _>("git_commit"), "git_branch": r.get::<Option<String>, _>("git_branch"), "git_dirty": r.get::<Option<i64>, _>("git_dirty").map(|d| d != 0),
            "status": status, "approved_by": approved_by, "approval_lapsed": !is_live && approved_by.is_none() && r.get::<Option<String>, _>("approved_by").is_some() && !live_before,
            "needs_approval": protected || adds_writes, "adds_writes": adds_writes, "sandbox": stores, "promotion_pending": requested_by.is_some(), "action": action, "preview_url": preview }));
    }
    Ok(Json(out).into_response())
}

/// One release: the listing's facts plus its full change review and its history (deployed, redeployed, approved, requested,
/// went live, replaced, preview reset), oldest first.
pub(crate) async fn get_version(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, is_approver(&st, &who) && !who.is_agent(), "the releases").await?;
    let (hash, _, _) = resolve_version(&st, &app, &hash).await?;
    let approved_by = current_approval(&st, &hash, &promoted_hash(&st, &app).await?).await?;
    let r = sqlx::query("SELECT status, review, created_at, approved_by, git_commit, seq, message, git_branch, created_by FROM versions WHERE app = $1 AND hash = $2").bind(&app).bind(&hash).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown version".into()))?;
    let review: Value = serde_json::from_str(&r.get::<String, _>("review")).unwrap_or(Value::Null);
    let sandbox: Vec<Value> = sqlx::query("SELECT name, db_name, seeded, updated_at FROM store_instances WHERE app = $1 AND target = $2 ORDER BY name").bind(&app).bind(&hash).fetch_all(&st.db).await?.iter().map(|s| json!({ "store": s.get::<String, _>("name"), "database": s.get::<String, _>("db_name"), "seeded": s.get::<i64, _>("seeded") != 0, "updated_at": s.get::<String, _>("updated_at") })).collect();
    let mut events: Vec<Value> = sqlx::query("SELECT kind, by_sub, at, details FROM release_events WHERE app = $1 AND hash = $2 ORDER BY at").bind(&app).bind(&hash).fetch_all(&st.db).await?.iter().map(|e| json!({ "kind": e.get::<String, _>("kind"), "by": e.get::<String, _>("by_sub"), "at": e.get::<String, _>("at"), "details": serde_json::from_str::<Value>(&e.get::<String, _>("details")).unwrap_or(Value::Null) })).collect();
    // releases deployed before the history existed: their deploy and go-lives from the versions and promotions tables
    if events.is_empty() {
        events.push(json!({ "kind": "deployed", "by": r.get::<String, _>("created_by"), "at": r.get::<String, _>("created_at"), "details": { "git_commit": r.get::<Option<String>, _>("git_commit") } }));
        for d in sqlx::query("SELECT by_sub, at, kind, migrations FROM deployments WHERE app = $1 AND hash = $2 ORDER BY at").bind(&app).bind(&hash).fetch_all(&st.db).await? { events.push(json!({ "kind": "went_live", "by": d.get::<String, _>("by_sub"), "at": d.get::<String, _>("at"), "details": { "back": d.get::<String, _>("kind") == "rollback", "migrations": d.get::<Option<String>, _>("migrations").and_then(|m| serde_json::from_str::<Value>(&m).ok()) } })); }
    }
    let seq: Option<i64> = r.get("seq");
    Ok(Json(json!({ "hash": hash, "seq": seq, "release": release_label(seq, &hash), "message": r.get::<Option<String>, _>("message"), "status": r.get::<String, _>("status"), "created_at": r.get::<String, _>("created_at"), "created_by": r.get::<String, _>("created_by"), "approved_by": approved_by, "git_commit": r.get::<Option<String>, _>("git_commit"), "git_branch": r.get::<Option<String>, _>("git_branch"), "review": review, "sandbox": sandbox, "events": events })).into_response())
}

pub(crate) async fn bundle_files(InRealm(st): InRealm, Path(hash): Path<String>, via: crate::workers::Via, headers: HeaderMap) -> Res {
    // the workers' path: a worker fetches what it runs (a pool only for the apps it serves; without a worker token, under
    // `cori up`, the in-process worker sends nothing). People fetch a version with `cori pull` (`…/versions/{hash}/bundle`)
    let worker = crate::workers::authenticate(&st, &headers, &via).await?;
    let app: Option<String> = sqlx::query_scalar("SELECT app FROM versions WHERE hash = $1").bind(&hash).fetch_optional(&st.db).await?;
    let Some(app) = app else { return Err(ApiError(StatusCode::NOT_FOUND, "unknown bundle".into())) };
    let dir = st.bundles.dir(st.db.realm(), &hash).await?;
    if !dir.exists() { return Err(ApiError(StatusCode::NOT_FOUND, "unknown bundle".into())); }
    if !crate::workers::serves_app(&worker, &crate::workers::app_connectors(&st, &app, &hash).await?) { return Err(ApiError(StatusCode::FORBIDDEN, format!("{} does not serve app `{app}`", worker.label()))); }
    Ok(Json(bundles::read_files(&dir)?).into_response())
}

pub async fn promoted(st: &AppState, app: &str) -> Result<Option<(String, Vec<bundles::WorkflowEntry>, String)>, ApiError> { promoted_version(st, app).await }

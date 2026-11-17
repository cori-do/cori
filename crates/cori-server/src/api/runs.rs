// SPDX-License-Identifier: Apache-2.0
//! Runs: starting one (sync, async, preview; idempotency and rate limits), its input, its outcome and events.
use super::*;

pub(crate) async fn start_preview_run(InRealm(st): InRealm, Path((app, hash, wf)): Path<(String, String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    // on a front's host: a release's own runs, on its own host only
    match front.as_ref().map(|f| f.0.release) {
        None => {}
        Some(None) => return Err(ApiError(StatusCode::NOT_FOUND, "a release's runs start on its own host".into())),
        Some(Some(seq)) => if resolve_version_hash(&st, &app, &hash).await? != resolve_version_hash(&st, &app, &format!("v{seq}")).await? { return Err(ApiError(StatusCode::NOT_FOUND, format!("this host runs release v{seq} only"))); },
    }
    preview_run(st, app, hash, wf, headers, body).await
}

/// Who may try a release (its runs on its sandbox stores, its files): its builders and approvers, anyone under dev auth,
/// and the holder of a release's own session.
pub(crate) async fn release_principal(st: &AppState, app: &str, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let who = app_principal(st, headers, app).await?;
    let rec = app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    // dev auth trusts any `dev:<sub>:<roles>` token anyway: let a preview be tried as the app's own personas
    let dev = matches!(st.cfg.auth, crate::auth::AuthMode::Dev);
    // a release's session was given to one who may (`api::sessions`), on that release's host only
    let session = who.claims.get("kind").and_then(|k| k.as_str()) == Some("preview");
    if !dev && !session && !(is_owner_or_builder(&who, &rec) || is_approver(st, &who) || who.is_builder()) { return Err(ApiError(StatusCode::FORBIDDEN, format!("previewing a version that is not promoted needs the app's owner, a builder, an approver or an admin (you are `{}` with roles {:?})", who.sub, who.roles))); }
    Ok(who)
}

/// The endpoint of workflow `wf` and whether `who` may start it: exposed, its sign-in and its roles.
pub(crate) fn endpoint_for(entry: &bundles::WorkflowEntry, who: &Principal) -> Result<cori_compiler::manifest::Endpoint, ApiError> {
    let ep = entry.compiled.manifest.endpoint.clone().ok_or(ApiError(StatusCode::FORBIDDEN, format!("workflow `{}` is not exposed as an endpoint", entry.id)))?;
    if ep.auth == "sso" && who.sub == "anonymous" { return Err(ApiError(StatusCode::UNAUTHORIZED, "this endpoint requires a signed-in user".into())); }
    let roles: Vec<String> = ep.allow.as_ref().and_then(|a| a.get("roles")).and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    if !who.has_any_role(&roles) { return Err(ApiError(StatusCode::FORBIDDEN, format!("requires one of roles {roles:?}"))); }
    Ok(ep)
}

/// A run of version `hash` on its sandbox stores, for its builders and approvers (anyone under dev auth).
async fn preview_run(st: Arc<AppState>, app: String, hash: String, wf: String, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    let who = release_principal(&st, &app, &headers).await?;
    let (h, wfs, cori_toml) = resolve_version(&st, &app, &hash).await?;
    // a run of the promoted version through its preview url is still a preview: it works on the version's sandbox
    start_run_on(st, app, wf, who, body.map(|b| b.0).unwrap_or(json!({})), headers, (h, wfs, cori_toml), true).await
}

pub(crate) async fn start_run(InRealm(st): InRealm, Path((app, wf)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, body: Option<Json<Value>>) -> Res {
    // a release's host runs its release, on its sandbox
    if let Some(seq) = front.as_ref().and_then(|f| f.0.release) { return preview_run(st, app, format!("v{seq}"), wf, headers, body).await; }
    let who = app_principal(&st, &headers, &app).await?;
    let v = promoted_version(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("app `{app}` has no promoted version (`cori promote {app} <version>`)")))?;
    start_run_on(st, app, wf, who, body.map(|b| b.0).unwrap_or(json!({})), headers, v, false).await
}

pub(crate) async fn start_run_on(st: Arc<AppState>, app: String, wf: String, who: Principal, params: Value, headers: HeaderMap, version: (String, Vec<bundles::WorkflowEntry>, String), preview: bool) -> Res {
    let (hash, wfs, cori_toml) = version;
    let entry = wfs.iter().find(|w| w.id == wf).ok_or(ApiError(StatusCode::NOT_FOUND, format!("workflow `{wf}` is not in {}", if preview { "that version" } else { "the promoted version" })))?;
    // endpoint policy
    let ep = endpoint_for(entry, &who)?;
    let mut params = params;
    cori_compiler::manifest::validate_params(&mut params, &entry.compiled.manifest.parameters).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    // file parameters: each the caller's own upload for these stores, replaced by the server's record of it
    let store_target = if preview { hash.clone() } else { cori_store::LIVE.to_string() };
    let file_ids = resolve_file_params(&st, &app, &store_target, &who, &mut params, &entry.compiled.manifest.parameters).await?;
    // the accounts its connectors call with, connected before it starts rather than missed halfway
    if let Some(r) = crate::connections::before_run(&st, &who, &app, &cori_toml, entry).await? { return Ok(r); }
    // rate limit: "30/min/principal" or "100/h" (per endpoint). Sliding window in the platform database, so every
    // server replica counts the same requests; an in-memory window is
    // the fallback when the database is unreachable.
    if let Some(rl) = &ep.rate_limit {
        if let Some((n, window, per_principal)) = cori_compiler::timing::rate_limit(rl).ok().or_else(|| legacy_rate_limit(rl)) {
            let key = if per_principal { format!("{app}:{wf}:{}", who.sub) } else { format!("{app}:{wf}") };
            let now_ms = chrono::Utc::now().timestamp_millis();
            let since = now_ms - window.as_millis() as i64;
            let db_count: Result<(i64, Option<i64>), sqlx::Error> = async {
                sqlx::query("DELETE FROM rate_events WHERE key = $1 AND at < $2").bind(&key).bind(since).execute(&st.db).await?;
                let r = sqlx::query("SELECT COUNT(*) AS n, MIN(at) AS oldest FROM rate_events WHERE key = $1 AND at >= $2").bind(&key).bind(since).fetch_one(&st.db).await?;
                Ok((r.get::<i64, _>("n"), r.get::<Option<i64>, _>("oldest")))
            }.await;
            match db_count {
                Ok((count, oldest)) => {
                    if count as usize >= n { let retry = ((oldest.unwrap_or(now_ms) + window.as_millis() as i64 - now_ms) / 1000).max(1); return Ok((StatusCode::TOO_MANY_REQUESTS, [("retry-after", retry.to_string())], Json(json!({ "error": format!("rate limit {rl} exceeded"), "retry_after_secs": retry }))).into_response()); }
                    sqlx::query("INSERT INTO rate_events (id, key, at) VALUES ($1, $2, $3)").bind(uuid::Uuid::new_v4().to_string()).bind(&key).bind(now_ms).execute(&st.db).await.ok();
                }
                Err(_) => {
                    // the process's own window, shared by every realm it serves: the realm is part of the key
                    let mut m = st.rate.lock().unwrap();
                    let q = m.entry(st.realm().map(|r| format!("{}/{key}", r.as_str())).unwrap_or(key)).or_default();
                    let now = std::time::Instant::now();
                    while q.front().map(|t| now.duration_since(*t) > window).unwrap_or(false) { q.pop_front(); }
                    if q.len() >= n { let retry = window.saturating_sub(now.duration_since(*q.front().unwrap())).as_secs().max(1); return Ok((StatusCode::TOO_MANY_REQUESTS, [("retry-after", retry.to_string())], Json(json!({ "error": format!("rate limit {rl} exceeded"), "retry_after_secs": retry }))).into_response()); }
                    q.push_back(now);
                }
            }
        }
    }
    // idempotency: `parameters` dedups on (workflow, principal, params) for 24 h; `header` on Idempotency-Key when present.
    // A preview keys on its version (its runs work on that version's sandbox), so it never collides with the promoted one.
    // The key is claimed atomically in `idempotency_keys` before the run starts: of two concurrent requests one launches,
    // the other replays. Only a running or succeeded run is replayed; a failed one (or one older than 24 h) hands the key over.
    let params_hash = { use sha2::Digest; hex::encode(sha2::Sha256::digest(canonical(&params).as_bytes())) };
    let scope = if preview { format!("{app}@{hash}") } else { app.clone() };
    let idem_key = match ep.idempotency.as_str() {
        "parameters" => Some(format!("{scope}:{wf}:{}:{}", who.sub, &params_hash[..24])),
        "header" => headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(|k| format!("{scope}:{wf}:{}:{k}", who.sub)),
        _ => None,
    };
    let run_id = crate::names::run_id(st.db.realm(), &wf);
    if let Some(k) = &idem_key {
        if let Some(replay) = claim_idem(&st.db, &app, k, &params_hash, &run_id).await? { return Ok(replay); }
        st.pending_idem(&run_id, k);
    }
    // the run's user: of the token's claims, only those the version lists (`[auth] principal_claims`)
    let claims = AppConfig::from_toml(&cori_toml, std::path::Path::new(".")).map(|c| c.principal_claims()).unwrap_or_default();
    let mut user = who.to_user_json(&claims);
    // a run an agent session started never acts with its person's connections (`connections::lease`)
    if who.is_agent() { user["via"] = json!("agent"); }
    // the run holds its files before it starts: no sweep takes one from under a step
    hold_files(&st, &run_id, &file_ids).await?;
    let watcher = match launch_run_with_token(&st, &app, &hash, &cori_toml, entry, &run_id, params.clone(), user, &who.sub, if preview { "preview" } else { &ep.mode }, who.token.clone()).await {
        Ok(w) => w,
        // the run never started: release the key so a retry is not answered with a run that does not exist, and its files
        Err(e) => { st.take_idem(&run_id); if let Some(k) = &idem_key { sqlx::query("DELETE FROM idempotency_keys WHERE idem_key = $1 AND run_id = $2").bind(k).bind(&run_id).execute(&st.db).await.ok(); } release_files(&st, &run_id).await; return Err(e); }
    };
    if ep.mode == "sync" {
        let secs = ep.timeout.as_deref().map(|t| cori_compiler::timing::duration(t).map(|d| d.as_secs()).unwrap_or_else(|_| legacy_secs(t))).unwrap_or(st.cfg.sync_timeout_secs).min(st.cfg.sync_timeout_secs);
        match tokio::time::timeout(std::time::Duration::from_secs(secs), watcher).await {
            Ok(Ok(Some(out))) => return Ok(Json(run_json(&run_id, &out)).into_response()),
            Ok(_) => return Err(ApiError(StatusCode::BAD_GATEWAY, "run failed".into())),
            Err(_) => {}
        }
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "run_id": run_id, "status": "running", "poll": format!("/v1/apps/{app}/runs/{run_id}") }))).into_response())
}

/// Claims an idempotency key for `run_id`. `Ok(None)`: this request owns the key and launches. `Ok(Some(response))`: the replay
/// of the run that owns it (202 running, 200 succeeded), 422 when the same key came with other parameters, 409 while the owner
/// is still starting. A failed or expired owner is taken over with a compare-and-swap, so concurrent retries still launch once.
pub(crate) async fn claim_idem(db: &crate::db::Db, app: &str, key: &str, params_hash: &str, run_id: &str) -> Result<Option<axum::response::Response>, ApiError> {
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::hours(24)).to_rfc3339();
    for _ in 0..3 {
        let claimed = sqlx::query("INSERT INTO idempotency_keys (idem_key, run_id, params_hash, created_at) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT idempotency_keys_pkey DO NOTHING").bind(key).bind(run_id).bind(params_hash).bind(now.to_rfc3339()).execute(db).await?.rows_affected();
        if claimed == 1 { return Ok(None); }
        let Some(row) = sqlx::query("SELECT run_id, params_hash, created_at FROM idempotency_keys WHERE idem_key = $1").bind(key).fetch_optional(db).await? else { continue };
        let (owner, owner_hash, at): (String, String, String) = (row.get("run_id"), row.get("params_hash"), row.get("created_at"));
        let run = sqlx::query("SELECT status, output FROM runs WHERE id = $1").bind(&owner).fetch_optional(db).await?;
        let live = at > since;
        if live && owner_hash != params_hash { return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Idempotency-Key reused with different parameters".into())); }
        match run.as_ref().map(|r| r.get::<String, _>("status")).as_deref() {
            Some("running") if live => return Ok(Some((StatusCode::ACCEPTED, [("x-cori-idempotent-replay", "true")], Json(json!({ "run_id": owner, "status": "running", "poll": format!("/v1/apps/{app}/runs/{owner}"), "idempotent_replay": true }))).into_response())),
            Some("succeeded") if live => {
                let mut j = run.and_then(|r| r.get::<Option<String>, _>("output")).and_then(|o| serde_json::from_str::<WorkflowOutput>(&o).ok()).map(|o| run_json(&owner, &o)).unwrap_or_else(|| json!({ "run_id": owner, "status": "succeeded" }));
                j["idempotent_replay"] = json!(true);
                return Ok(Some(([("x-cori-idempotent-replay", "true")], Json(j)).into_response()));
            }
            None if live && at > (now - chrono::Duration::minutes(2)).to_rfc3339() => return Err(ApiError(StatusCode::CONFLICT, "a request with this idempotency key is still starting; retry shortly".into())),
            _ => {
                // failed, expired, or an owner that never recorded a run: take the key over unless another request just did
                let took = sqlx::query("UPDATE idempotency_keys SET run_id = $1, params_hash = $2, created_at = $3 WHERE idem_key = $4 AND run_id = $5").bind(run_id).bind(params_hash).bind(now.to_rfc3339()).bind(key).bind(&owner).execute(db).await?.rows_affected();
                if took == 1 { return Ok(None); }
            }
        }
    }
    Err(ApiError(StatusCode::CONFLICT, "idempotency key is contended; retry shortly".into()))
}

/// Builds the workflow input for a deployed workflow, starts it, records the run, and returns a watcher that records the outcome.
pub async fn launch_run(st: &Arc<AppState>, app: &str, hash: &str, cori_toml: &str, entry: &bundles::WorkflowEntry, run_id: &str, params: Value, user: Value, actor: &str, mode: &str) -> Result<tokio::task::JoinHandle<Option<WorkflowOutput>>, ApiError> {
    launch_run_with_token(st, app, hash, cori_toml, entry, run_id, params, user, actor, mode, None).await
}

/// A version's cori.toml as this server runs it: without the dev mocks it does not answer from (all of them in production,
/// those of `--no-mock` connectors), before the registry merge decides what may run without one.
pub(crate) fn hosted_app_config(cfg: &crate::ServerConfig, cori_toml: &str, dir: &std::path::Path) -> anyhow::Result<AppConfig> {
    let mut appcfg = AppConfig::from_toml(cori_toml, dir)?;
    if !cfg.dev_mocks { appcfg = appcfg.without_mocks(); }
    for c in &cfg.no_mocks { if let Some(x) = appcfg.connectors.get_mut(c) { x.dev = None; } }
    Ok(appcfg)
}

/// The workflow input for one deployed workflow: app config with registry merge, every connector resolved, LLM bindings.
pub async fn build_input(st: &Arc<AppState>, app: &str, hash: &str, cori_toml: &str, entry: &bundles::WorkflowEntry, params: Value, user: Value) -> Result<(WorkflowInput, AppConfig), ApiError> {
    let bdir = st.bundles.dir(st.db.realm(), hash).await?;
    let mut appcfg = hosted_app_config(&st.cfg, cori_toml, &bdir)?;
    let mut reg = BTreeMap::new();
    for row in registry_rows(st).await? { if let (Some(n), Ok(cfg)) = (row["name"].as_str(), serde_json::from_value::<cori_engine::config::ConnectorConfig>(row["config"].clone())) { reg.insert(n.to_string(), cfg); } }
    let o = appcfg.merge_registry_hosted(&reg, st.cfg.strict_connectors());
    if !o.credentials.is_empty() { tracing::warn!(app, connectors = ?o.credentials, "credentials the app declares for connectors the org registry does not are ignored"); }
    if !o.pins.is_empty() { tracing::warn!(app, connectors = ?o.pins, "worker pools the app pins its connectors to are ignored: the org registry decides where a connector runs"); }
    for c in &entry.compiled.manifest.connectors { appcfg.resolve_connector(c)?; }
    let mut connectors = BTreeMap::new();
    for c in appcfg.connectors.keys().cloned().collect::<Vec<_>>().iter() {
        let Ok(mut rc) = appcfg.resolve_connector(c) else { continue };
        if st.cfg.no_mocks.contains(c) || (rc.kind == "store" && st.cfg.store_database_url.is_some()) { rc.mock = None; }
        // what the worker's requests to it may reach: the org registry's connectors reach further than an app's own, and the
        // deployment's the address its operators wrote
        rc.egress = if st.deployment.reserves(c) { cori_engine::egress::Egress::Any } else if appcfg.connectors.get(c).map(|x| x.registry).unwrap_or(false) { st.cfg.registry_egress() } else { st.cfg.app_egress() };
        connectors.insert(c.clone(), rc);
    }
    let mut llm = appcfg.resolve_llm()?;
    crate::llm::merge_into(&st.db, st.cfg.llm_starter.as_ref(), &mut llm).await?;
    llm.egress = cori_engine::config::LlmEgress { org: st.cfg.registry_egress(), app: st.cfg.app_egress() };
    for r in sqlx::query("SELECT workflow, step, provider, model, mode FROM bindings WHERE app = $1").bind(app).fetch_all(&st.db).await? { llm.bindings.insert(format!("{}:{}:{}", r.get::<String, _>("workflow"), r.get::<String, _>("step"), r.get::<String, _>("mode")), cori_engine::config::Binding { provider: r.get("provider"), model: r.get("model"), mode: r.get("mode") }); }
    let input = WorkflowInput { compiled: entry.compiled.clone(), params, user, root: entry.dir.clone(), bundle: Some(BundleRef { hash: hash.to_string(), server: st.cfg.worker_url(), realm: st.realm().map(|r| r.as_str().to_string()), app: Some(app.to_string()), sandboxed: cori_engine::sandbox::Pool::global().is_some() }), store_server: None, record_dir: None, schedule: None, connectors, llm, started_at: chrono::Utc::now().to_rfc3339(), pool_queue_prefix: Some(crate::names::pool_queue_prefix(&st.cfg, st.db.realm())), sandbox_queue: Some(crate::names::sandbox_queue(&st.cfg)), decisions: BTreeMap::new() };
    Ok((input, appcfg))
}

pub async fn launch_run_with_token(st: &Arc<AppState>, app: &str, hash: &str, cori_toml: &str, entry: &bundles::WorkflowEntry, run_id: &str, params: Value, user: Value, actor: &str, mode: &str, principal_token: Option<String>) -> Result<tokio::task::JoinHandle<Option<WorkflowOutput>>, ApiError> {
    let (input, _) = build_input(st, app, hash, cori_toml, entry, params.clone(), user.clone()).await?;
    let wf = entry.id.clone();
    // the runs of the month (`Limits::runs_per_month`)
    crate::usage::check_run(st).await?;
    // the runs under way at once (`RealmSettings::max_active_runs`): one waiting for a person holds no worker or sandbox
    let cap = st.settings.max_active_runs;
    if cap > 0 {
        let n: i64 = sqlx::query("SELECT COUNT(*) AS n FROM runs r WHERE r.status = 'running' AND NOT EXISTS (SELECT 1 FROM approvals a WHERE a.run_id = r.id AND a.status = 'pending')").fetch_one(&st.db).await?.get("n");
        if n as usize >= cap { return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, format!("{n} runs are under way, the most allowed at once here (CORI_MAX_ACTIVE_RUNS): retry when one ends"))); }
    }
    let mut options = WorkflowStartOptions::new(crate::names::run_queue(&st.cfg, st.db.realm(), &app), run_id.to_string()).build();
    if let Some(key) = crate::names::fairness_key(st.realm()) { options.priority = temporalio_client::Priority::builder().fairness_key(key).build(); }
    let sealed = match &principal_token { Some(t) => Some(crate::identity::seal_for_run(st, run_id, t).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?), None => None };
    let idem = st.take_idem(run_id);
    // a preview run works on its version's sandbox stores; every other run (endpoint, schedule) on the live ones
    let preview = mode == "preview";
    let store_target = if preview { hash.to_string() } else { cori_store::LIVE.to_string() };
    // the run's row before its workflow: a step's first call back (a store call as the run's user, a lease) may come
    // within milliseconds, and the row is what answers it
    sqlx::query("INSERT INTO runs (id, app, workflow, version_hash, status, started_at, user_sub, params, user_json, principal_token, idem_key, store_target) VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8, $9, $10, $11)")
        .bind(run_id).bind(app).bind(&wf).bind(hash).bind(chrono::Utc::now().to_rfc3339()).bind(actor).bind(params.to_string()).bind(user.to_string()).bind(sealed).bind(idem).bind(&store_target).execute(&st.db).await?;
    let handle = match st.temporal.start_workflow(CoriWorkflow::run, input, options).await {
        Ok(h) => h,
        Err(e) => {
            // it never started: no row says it runs (a server stopped in between leaves one to `reconcile_runs`)
            sqlx::query("DELETE FROM runs WHERE id = $1").bind(run_id).execute(&st.db).await.ok();
            return Err(ApiError(StatusCode::BAD_GATEWAY, format!("temporal start: {e}")));
        }
    };
    db::audit(&st.db, actor, "run.start", run_id, json!({ "app": app, "workflow": wf, "mode": mode })).await?;
    crate::usage::add(&st.db, "runs", app, 1.0).await;
    if let Some(t) = principal_token { let (st2, rid) = (st.clone(), run_id.to_string()); tokio::spawn(async move { crate::identity::refresh_for_run(&st2, &rid, &t).await }); }
    let st2 = st.clone(); let rid = run_id.to_string();
    Ok(tokio::spawn(async move {
        let r = handle.get_result(WorkflowGetResultOptions::default()).await;
        finish_run(&st2, &rid, r.as_ref().map_err(|e| e.to_string())).await;
        r.ok()
    }))
}

/// Records a run's outcome, once: the row of a run still `running` gets its status, output and error, and the app's
/// webhooks hear of it (not for a preview, whose version is not live, nor for a child run). The watcher of a run
/// started here calls it, and so does `reconcile_runs` for runs whose watcher went away with a restart. Returns whether
/// this call recorded it.
pub(crate) async fn finish_run(st: &Arc<AppState>, run_id: &str, r: Result<&WorkflowOutput, String>) -> bool {
    let (status, output, error) = match &r { Ok(o) => (o.status.clone(), serde_json::to_string(o).ok(), o.error.clone()), Err(e) => ("failed".to_string(), None, Some(e.clone())) };
    let recorded = sqlx::query("UPDATE runs SET status = $1, ended_at = $2, output = $3, error = $4 WHERE id = $5 AND status = 'running'").bind(&status).bind(chrono::Utc::now().to_rfc3339()).bind(output).bind(&error).bind(run_id).execute(&st.db).await.map(|x| x.rows_affected() == 1);
    match recorded { Ok(true) => {}, Ok(false) => return false, Err(e) => { tracing::warn!(run = %run_id, error = %e, "run outcome not recorded"); return false; } }
    count_run_time(st, run_id).await;
    let row = sqlx::query("SELECT r.app, r.workflow, r.user_sub, r.store_target, r.parent_run, v.cori_toml FROM runs r JOIN versions v ON v.hash = r.version_hash AND v.app = r.app WHERE r.id = $1").bind(run_id).fetch_optional(&st.db).await;
    let Ok(Some(row)) = row else { return true };
    let (app, wf, actor, target, parent, cori_toml): (String, String, Option<String>, Option<String>, Option<String>, Option<String>) = (row.get("app"), row.get("workflow"), row.get("user_sub"), row.get("store_target"), row.get("parent_run"), row.get("cori_toml"));
    // previews stay quiet: no webhook reaches the app's backend or Slack for a run of a version that is not promoted
    if parent.is_some() || target.as_deref().is_some_and(|t| t != cori_store::LIVE) { return true; }
    let hooks = cori_toml.and_then(|t| AppConfig::from_toml(&t, std::path::Path::new(".")).ok()).map(|c| c.webhooks).unwrap_or_default();
    let event = if status == "succeeded" { "run.completed" } else { "run.failed" };
    let payload = json!({ "run_id": run_id, "app": app, "workflow": wf, "status": status, "user": actor, "error": error, "result": r.ok().and_then(|o| o.result.clone()) });
    crate::webhooks::deliver(st, &app, &hooks, event, payload).await;
    true
}

/// Adds the time of a run that just ended to its app's month (`run_ms`); a child run's time is its parent's.
async fn count_run_time(st: &AppState, run_id: &str) {
    let Ok(Some(r)) = sqlx::query("SELECT app, started_at, ended_at, parent_run FROM runs WHERE id = $1").bind(run_id).fetch_optional(&st.db).await else { return };
    if r.get::<Option<String>, _>("parent_run").is_some() { return; }
    let at = |c: &str| r.get::<Option<String>, _>(c).and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
    if let (Some(a), Some(b)) = (at("started_at"), at("ended_at")) { crate::usage::add(&st.db, "run_ms", &r.get::<String, _>("app"), (b - a).num_milliseconds().max(0) as f64).await; }
}

/// Runs still `running` in the index whose workflow has closed in Temporal: their watcher went away with a server restart
/// or a rollout (or the replica that started them), so their outcome was never recorded. And runs whose workflow Temporal
/// does not know a while after they were written: a server stopped between writing the row and starting the workflow
/// (`launch_run_with_token`), or the history expired. Returns how many it recorded.
pub(crate) async fn reconcile_runs(st: &Arc<AppState>) -> anyhow::Result<usize> {
    use temporalio_client::errors::WorkflowInteractionError;
    use temporalio_client::{WorkflowDescribeOptions, WorkflowExecutionStatus};
    // a run whose watcher is still there may be recorded by either: `finish_run` records it once
    let rows: Vec<(String, String)> = sqlx::query("SELECT id, started_at FROM runs WHERE status = 'running'").fetch_all(&st.db).await?.iter().map(|r| (r.get("id"), r.get("started_at"))).collect();
    let mut recorded = 0;
    for (id, started_at) in rows {
        let handle = st.temporal.get_workflow_handle::<CoriWorkflow>(id.clone());
        let d = match handle.describe(WorkflowDescribeOptions::default()).await {
            Ok(d) => d,
            Err(WorkflowInteractionError::NotFound(_)) if written_before(&started_at, chrono::Duration::minutes(5)) => {
                if finish_run(st, &id, Err("Temporal has no workflow for this run: it never started (its server stopped while starting it), or its history expired".into())).await { recorded += 1; }
                continue;
            }
            Err(_) => continue,
        };
        if matches!(d.status(), WorkflowExecutionStatus::Running | WorkflowExecutionStatus::Unspecified) { continue; }
        let r = handle.get_result(WorkflowGetResultOptions::default()).await;
        if finish_run(st, &id, r.as_ref().map_err(|e| e.to_string())).await { recorded += 1; }
    }
    Ok(recorded)
}

/// Whether an RFC 3339 time (a run's `started_at`) is more than `ago` in the past; a time that does not read is not.
fn written_before(at: &str, ago: chrono::Duration) -> bool {
    chrono::DateTime::parse_from_rfc3339(at).map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) > ago).unwrap_or(false)
}

/// An `endpoint.timeout` as versions compiled before `cori check` validated durations read it (`h` and `d` meant seconds).
pub(crate) fn legacy_secs(s: &str) -> u64 { let n: u64 = s.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(30); if s.ends_with('m') { n * 60 } else { n } }

pub(crate) fn run_json(run_id: &str, out: &WorkflowOutput) -> Value { json!({ "run_id": run_id, "status": out.status, "result": out.result, "outputs": out.steps, "error": out.error, "error_detail": out.error_detail, "steps": out.trace.iter().map(|t| { let mut s = json!({ "name": t.name, "kind": t.kind, "status": t.status, "attempts": t.attempts, "ms": t.ms, "cost_eur": t.cost_eur, "notes": t.notes, "error": t.error }); if let Some(p) = &t.path { s["path"] = json!(p); } s }).collect::<Vec<_>>() }) }

/// A run of `app` within what the request's host reaches (`RunScope`): else unknown there.
pub(crate) async fn run_in_scope(st: &AppState, app: &str, run: &str, front: Option<&FrontHost>) -> Result<(), ApiError> {
    let target: Option<Option<String>> = sqlx::query_scalar("SELECT store_target FROM runs WHERE id = $1 AND app = $2").bind(run).bind(app).fetch_optional(&st.db).await?;
    match target { Some(t) if RunScope::of(st, app, front).await?.sees(t.as_deref()) => Ok(()), _ => Err(ApiError(StatusCode::NOT_FOUND, "unknown run".into())) }
}

pub(crate) async fn get_run(InRealm(st): InRealm, Path((app, run)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    run_in_scope(&st, &app, &run, front.as_ref().map(|f| &f.0)).await?;
    let r = sqlx::query("SELECT status, started_at, ended_at, user_sub, params, output, error, workflow FROM runs WHERE id = $1 AND app = $2").bind(&run).bind(&app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown run".into()))?;
    let owner: String = r.get("user_sub");
    if owner != who.sub && !may_see_all_runs(&st, &who, &app).await? { return Err(ApiError(StatusCode::FORBIDDEN, "not your run".into())); }
    let output: Option<WorkflowOutput> = r.get::<Option<String>, _>("output").and_then(|s| serde_json::from_str(&s).ok());
    let mut v = json!({ "run_id": run, "workflow": r.get::<String, _>("workflow"), "status": r.get::<String, _>("status"), "started_at": r.get::<String, _>("started_at"), "ended_at": r.get::<Option<String>, _>("ended_at"), "params": serde_json::from_str::<Value>(&r.get::<String, _>("params")).unwrap_or(Value::Null), "error": r.get::<Option<String>, _>("error") });
    if let Some(o) = output { let rj = run_json(&run, &o); v["result"] = rj["result"].clone(); v["steps"] = rj["steps"].clone(); v["outputs"] = rj["outputs"].clone(); v["error_detail"] = rj["error_detail"].clone(); }
    else if v["status"] == "running" {
        // under way: the steps done so far (their events) and, when it waits for a person, which step and for whom
        // (their data, which the events carry for the run's inspection, is `steps`' own: `GET …/runs/{run}/steps`)
        let steps: Vec<Value> = sqlx::query("SELECT payload FROM run_events WHERE run_id = $1 AND kind = 'step' ORDER BY at").bind(&run).fetch_all(&st.db).await?
            .iter().filter_map(|e| serde_json::from_str::<Value>(&e.get::<String, _>("payload")).ok())
            .map(|mut p| { if let Some(m) = p.as_object_mut() { for k in ["input", "output", "too_large"] { m.remove(k); } } p }).collect();
        v["steps"] = json!(steps);
        let pending = sqlx::query("SELECT id, step, to_role, created_at FROM approvals WHERE run_id = $1 AND app = $2 AND status = 'pending' ORDER BY created_at LIMIT 1").bind(&run).bind(&app).fetch_optional(&st.db).await?;
        v["pending_approval"] = pending.map(|p| json!({ "id": p.get::<String, _>("id"), "step": p.get::<String, _>("step"), "to": p.get::<String, _>("to_role"), "since": p.get::<String, _>("created_at") })).unwrap_or(Value::Null);
    }
    Ok(Json(v).into_response())
}

/// The app's builders and operators see every run of it; anyone else their own (doc 04, run visibility).
pub(crate) async fn may_see_all_runs(st: &AppState, who: &Principal, app: &str) -> Result<bool, ApiError> {
    Ok(who.is_operator() || app_record(st, app).await?.map(|rec| is_owner_or_builder(who, &rec)).unwrap_or(false))
}

pub(crate) async fn list_runs(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    // a front's host lists the runs of its own stores: the live ones, or its release's
    let scope = RunScope::of(&st, &app, front.as_ref().map(|f| &f.0)).await?;
    let rows = if may_see_all_runs(&st, &who, &app).await? {
        sqlx::query("SELECT id, workflow, status, started_at, ended_at, user_sub, parent_run FROM runs WHERE app = $1 AND ($2 = '*' OR COALESCE(store_target, 'live') = $2) ORDER BY started_at DESC LIMIT 100").bind(&app).bind(scope.target()).fetch_all(&st.db).await?
    } else {
        sqlx::query("SELECT id, workflow, status, started_at, ended_at, user_sub, parent_run FROM runs WHERE app = $1 AND user_sub = $2 AND ($3 = '*' OR COALESCE(store_target, 'live') = $3) ORDER BY started_at DESC LIMIT 100").bind(&app).bind(&who.sub).bind(scope.target()).fetch_all(&st.db).await?
    };
    Ok(Json(rows.iter().map(|r| json!({ "run_id": r.get::<String, _>("id"), "workflow": r.get::<String, _>("workflow"), "status": r.get::<String, _>("status"), "started_at": r.get::<String, _>("started_at"), "ended_at": r.get::<Option<String>, _>("ended_at"), "user": r.get::<String, _>("user_sub"), "parent_run": r.get::<Option<String>, _>("parent_run") })).collect::<Vec<_>>()).into_response())
}

pub(crate) async fn decide(InRealm(st): InRealm, Path((app, run, step)): Path<(String, String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, Json(value): Json<Value>) -> Res {
    // the same rules as the approvals inbox: the step's pending approval, its routing, four eyes
    let who = app_principal(&st, &headers, &app).await?;
    run_in_scope(&st, &app, &run, front.as_ref().map(|f| &f.0)).await?;
    let pending = sqlx::query("SELECT id FROM approvals WHERE run_id = $1 AND step = $2 AND app = $3 AND status = 'pending'").bind(&run).bind(&step).bind(&app).fetch_optional(&st.db).await?;
    let Some(p) = pending else { return Err(ApiError(StatusCode::CONFLICT, format!("step `{step}` of this run is not waiting for a decision"))) };
    decide_run_approval(&st, &app, &p.get::<String, _>("id"), &who, value).await
}

#[derive(serde::Deserialize)]
pub(crate) struct Event { run_id: String, kind: String, at: String, payload: Value }

/// Server-sent events: replays recorded events then polls for new ones until the run ends.
pub(crate) async fn run_stream(InRealm(st): InRealm, Path((app, run)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    run_in_scope(&st, &app, &run, front.as_ref().map(|f| &f.0)).await?;
    let ok = sqlx::query("SELECT user_sub FROM runs WHERE id = $1 AND app = $2").bind(&run).bind(&app).fetch_optional(&st.db).await?;
    let Some(r) = ok else { return Err(ApiError(StatusCode::NOT_FOUND, "unknown run".into())) };
    if r.get::<String, _>("user_sub") != who.sub && !may_see_all_runs(&st, &who, &app).await? { return Err(ApiError(StatusCode::FORBIDDEN, "not your run".into())); }
    let stream = async_stream::stream! {
        let mut seen: usize = 0;
        for _ in 0..7200 {
            let rows = sqlx::query("SELECT kind, at, payload FROM run_events WHERE run_id = $1 ORDER BY at").bind(&run).fetch_all(&st.db).await.unwrap_or_default();
            for r in rows.iter().skip(seen) { let data = json!({ "kind": r.get::<String, _>("kind"), "at": r.get::<String, _>("at"), "payload": serde_json::from_str::<Value>(&r.get::<String, _>("payload")).unwrap_or(Value::Null) }); yield Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().event("event").data(data.to_string())); }
            seen = rows.len();
            let status: Option<String> = sqlx::query("SELECT status FROM runs WHERE id = $1").bind(&run).fetch_optional(&st.db).await.ok().flatten().map(|r| r.get("status"));
            if let Some(s) = &status { if s != "running" { yield Ok(axum::response::sse::Event::default().event("done").data(json!({ "status": s }).to_string())); break; } }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    };
    Ok(axum::response::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default()).into_response())
}

pub(crate) async fn ingest(InRealm(st): InRealm, via: crate::workers::Via, headers: HeaderMap, Json(ev): Json<Event>) -> Res {
    let worker = crate::workers::authenticate(&st, &headers, &via).await?;
    if let crate::workers::Worker::Pool(_) = &worker {
        // a pool reports only on runs of apps it serves, and never starts a scheduled run
        let ok = match crate::workers::run_ref(&st, &ev.run_id).await? { Some(r) => crate::workers::serves_app(&worker, &crate::workers::app_connectors(&st, &r.app, &r.version).await?), None => false };
        if !ok || ev.kind == "run_started" { return Err(ApiError(StatusCode::FORBIDDEN, format!("{} does not serve run `{}`", worker.label(), ev.run_id))); }
    }
    sqlx::query("INSERT INTO run_events (id, run_id, kind, at, payload) VALUES ($1, $2, $3, $4, $5)").bind(uuid::Uuid::new_v4().to_string()).bind(&ev.run_id).bind(&ev.kind).bind(&ev.at).bind(ev.payload.to_string()).execute(&st.db).await?;
    if ev.kind == "llm_call" {
        let p = &ev.payload;
        let run = sqlx::query("SELECT app, version_hash FROM runs WHERE id = $1").bind(&ev.run_id).fetch_optional(&st.db).await?.map(|r| (r.get::<String, _>("app"), r.get::<String, _>("version_hash")));
        let (provider, model) = (p["provider"].as_str().unwrap_or(""), p["model"].as_str().unwrap_or(""));
        let (tin, tout) = (p["tokens_in"].as_u64().unwrap_or(0), p["tokens_out"].as_u64().unwrap_or(0));
        // the server prices the call (the worker's figure is not trusted) and counts it in the run's app's month
        let cost = match &run { Some((app, version)) => crate::llm::settle(&st.db, &ev.run_id, app, version, provider, model, tin, tout).await?, None => 0.0 };
        sqlx::query("INSERT INTO llm_calls (id, run_id, app, workflow, step, tier, model, mode, input, output, schema_valid, latency_ms, tokens_in, tokens_out, cost_eur, at, provider) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&ev.run_id).bind(run.as_ref().map(|r| r.0.clone())).bind(p["workflow"].as_str().unwrap_or("")).bind(p["step"].as_str().unwrap_or("")).bind(p["tier"].as_str().unwrap_or("")).bind(model).bind(p["mode"].as_str().unwrap_or("primary"))
            .bind(p["input"].to_string()).bind(p.get("output").map(|o| o.to_string())).bind(p["schema_valid"].as_bool().unwrap_or(false) as i32).bind(p["latency_ms"].as_i64().unwrap_or(0)).bind(tin as i64).bind(tout as i64).bind(cost).bind(&ev.at).bind((!provider.is_empty()).then_some(provider)).execute(&st.db).await.ok();
    }
    if ev.kind == "run_started" {
        // a scheduled run indexes itself; who it runs as, its app, workflow and version come from the server's own schedule
        // record (written at sync), never from the event, and its id must be one the schedule's action produces
        let p = &ev.payload;
        let sched = sqlx::query("SELECT app, workflow, user_json, version_hash FROM schedules WHERE id = $1").bind(p["schedule"].as_str().unwrap_or("")).fetch_optional(&st.db).await?;
        if let Some(r) = sched {
            let (app, wf): (String, String) = (r.get("app"), r.get("workflow"));
            let (user_json, version): (Option<String>, Option<String>) = (r.get("user_json"), r.get("version_hash"));
            if !ev.run_id.starts_with(&crate::names::scheduled_run_prefix(st.db.realm(), &app, &wf)) { return Err(ApiError(StatusCode::FORBIDDEN, "run id does not belong to the schedule".into())); }
            let (Some(user_json), Some(version)) = (user_json, version) else { return Err(ApiError(StatusCode::CONFLICT, "schedule not synced yet".into())) };
            let user: Value = serde_json::from_str(&user_json).unwrap_or(Value::Null);
            let sub = user["sub"].as_str().unwrap_or("schedule").to_string();
            let indexed = sqlx::query("INSERT INTO runs (id, app, workflow, version_hash, status, started_at, user_sub, params, user_json) VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8) ON CONFLICT ON CONSTRAINT runs_pkey DO NOTHING")
                .bind(&ev.run_id).bind(&app).bind(&wf).bind(&version).bind(&ev.at).bind(&sub).bind(p["params"].to_string()).bind(&user_json).execute(&st.db).await.map(|r| r.rows_affected()).unwrap_or(0);
            // a scheduled run counts in the month's runs (its schedule, not the limit, bounds it)
            if indexed > 0 { crate::usage::add(&st.db, "runs", &app, 1.0).await; }
            db::audit(&st.db, &sub, "run.start", &ev.run_id, json!({ "app": app, "workflow": wf, "mode": "schedule" })).await.ok();
        }
    }
    if ev.kind == "run_finished" {
        let p = &ev.payload;
        let ended = sqlx::query("UPDATE runs SET status = $1, ended_at = $2, output = $3, error = $4 WHERE id = $5 AND status = 'running'").bind(p["status"].as_str().unwrap_or("failed")).bind(&ev.at).bind(p.get("output").map(|o| o.to_string())).bind(p["error"].as_str()).bind(&ev.run_id).execute(&st.db).await.map(|r| r.rows_affected()).unwrap_or(0);
        if ended > 0 { count_run_time(&st, &ev.run_id).await; }
        if let Some(r) = sqlx::query("SELECT app, workflow, user_sub FROM runs WHERE id = $1").bind(&ev.run_id).fetch_optional(&st.db).await? {
            let (app, wf, sub): (String, String, String) = (r.get("app"), r.get("workflow"), r.get("user_sub"));
            if let Ok(Some((_, _, cori_toml))) = promoted_version(&st, &app).await { if let Ok(cfg) = AppConfig::from_toml(&cori_toml, std::path::Path::new(".")) { let st2 = st.clone(); let hooks = cfg.webhooks.clone(); let status = p["status"].as_str().unwrap_or("failed").to_string(); let payload = json!({ "run_id": ev.run_id, "app": app, "workflow": wf, "status": status, "user": sub, "error": p["error"], "result": p["result"] }); tokio::spawn(async move { let ev = if status == "succeeded" { "run.completed" } else { "run.failed" }; crate::webhooks::deliver(&st2, payload["app"].as_str().unwrap_or(""), &hooks, ev, payload.clone()).await; }); } }
        }
    }
    if ev.kind == "child_started" {
        let p = &ev.payload;
        if let Some(r) = sqlx::query("SELECT app, version_hash, user_sub, user_json, store_target FROM runs WHERE id = $1").bind(&ev.run_id).fetch_optional(&st.db).await? {
            let (app, hash, sub, uj): (String, String, String, Option<String>) = (r.get("app"), r.get("version_hash"), r.get("user_sub"), r.get("user_json"));
            // a child works on its parent's stores: a preview's children are previews too
            let target: Option<String> = r.get("store_target");
            sqlx::query("INSERT INTO runs (id, app, workflow, version_hash, status, started_at, user_sub, params, user_json, parent_run, store_target) VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8, $9, $10) ON CONFLICT ON CONSTRAINT runs_pkey DO NOTHING")
                .bind(p["child_run_id"].as_str().unwrap_or("")).bind(&app).bind(p["workflow"].as_str().unwrap_or("")).bind(&hash).bind(&ev.at).bind(&sub).bind(p["params"].to_string()).bind(uj).bind(&ev.run_id).bind(target).execute(&st.db).await.ok();
        }
    }
    if ev.kind == "child_finished" {
        let p = &ev.payload;
        sqlx::query("UPDATE runs SET status = $1, ended_at = $2, output = $3, error = $4 WHERE id = $5").bind(p["status"].as_str().unwrap_or("failed")).bind(&ev.at).bind(p.get("output").map(|o| o.to_string())).bind(p["error"].as_str()).bind(p["child_run_id"].as_str().unwrap_or("")).execute(&st.db).await.ok();
    }
    if ev.kind == "approval_pending" {
        let app: Option<String> = sqlx::query("SELECT app FROM runs WHERE id = $1").bind(&ev.run_id).fetch_optional(&st.db).await?.map(|r| r.get("app"));
        if let Some(app) = app {
            let step = ev.payload.get("step").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let to = ev.payload.get("to").cloned().unwrap_or(Value::Null);
            let to_role = to.get("role").and_then(|r| r.as_str()).map(String::from).or_else(|| to.get("user").and_then(|u| u.as_str()).map(|u| format!("user:{u}"))).unwrap_or_default();
            sqlx::query("INSERT INTO approvals (id, run_id, app, step, to_role, payload, status, created_at) VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)")
                .bind(format!("{}:{}", ev.run_id, step)).bind(&ev.run_id).bind(&app).bind(&step).bind(&to_role).bind(ev.payload.to_string()).bind(&ev.at).execute(&st.db).await.ok();
            if let Ok(Some((_, _, cori_toml))) = promoted_version(&st, &app).await { if let Ok(cfg) = AppConfig::from_toml(&cori_toml, std::path::Path::new(".")) { let st2 = st.clone(); let hooks = cfg.webhooks.clone(); let payload = json!({ "approval_id": format!("{}:{}", ev.run_id, step), "run_id": ev.run_id, "app": app, "step": step, "to": to_role, "payload": ev.payload }); tokio::spawn(async move { crate::webhooks::deliver(&st2, &payload["app"].as_str().unwrap_or("").to_string(), &hooks, "approval.pending", payload).await; }); } }
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn run_events(InRealm(st): InRealm, Path((app, run)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    run_in_scope(&st, &app, &run, front.as_ref().map(|f| &f.0)).await?;
    let ok = sqlx::query("SELECT user_sub FROM runs WHERE id = $1 AND app = $2").bind(&run).bind(&app).fetch_optional(&st.db).await?;
    let Some(r) = ok else { return Err(ApiError(StatusCode::NOT_FOUND, "unknown run".into())) };
    if r.get::<String, _>("user_sub") != who.sub && !may_see_all_runs(&st, &who, &app).await? { return Err(ApiError(StatusCode::FORBIDDEN, "not your run".into())); }
    let rows = sqlx::query("SELECT kind, at, payload FROM run_events WHERE run_id = $1 ORDER BY at").bind(&run).fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "kind": r.get::<String, _>("kind"), "at": r.get::<String, _>("at"), "payload": serde_json::from_str::<Value>(&r.get::<String, _>("payload")).unwrap_or(Value::Null) })).collect::<Vec<_>>()).into_response())
}

/// An `endpoint.rate_limit` as versions compiled before `cori check` validated rates read it (`0/min` refused every call,
/// an unknown scope limited all callers together).
pub(crate) fn legacy_rate_limit(s: &str) -> Option<(usize, std::time::Duration, bool)> {
    let mut parts = s.split('/');
    let n: usize = parts.next()?.trim().parse().ok()?;
    let unit = parts.next()?.trim().to_lowercase();
    let window = match unit.as_str() { "s" | "sec" | "second" => 1, "min" | "m" | "minute" => 60, "h" | "hour" => 3600, "d" | "day" => 86400, _ => return None };
    let per_principal = parts.next().map(|p| p.trim() == "principal" || p.trim() == "user").unwrap_or(false);
    Some((n, std::time::Duration::from_secs(window), per_principal))
}

pub(crate) async fn list_schedules(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, who.is_operator(), "the schedules").await?;
    let rows = sqlx::query("SELECT id, workflow, cron, tz, mode, updated_at, user_json, problem FROM schedules WHERE app = $1 ORDER BY workflow").bind(&app).fetch_all(&st.db).await?;
    // `runs_as`: the service account the runs act as, with the roles its own token carries (never the workflow's YAML)
    Ok(Json(rows.iter().map(|r| {
        let user: Value = r.get::<Option<String>, _>("user_json").and_then(|u| serde_json::from_str(&u).ok()).unwrap_or(Value::Null);
        json!({ "id": r.get::<String, _>("id"), "workflow": r.get::<String, _>("workflow"), "cron": r.get::<String, _>("cron"), "tz": r.get::<String, _>("tz"), "mode": r.get::<String, _>("mode"), "updated_at": r.get::<String, _>("updated_at"),
                "runs_as": if user.is_null() { Value::Null } else { json!({ "sub": user["sub"], "roles": user["roles"], "client_id": user["client_id"] }) }, "problem": r.get::<Option<String>, _>("problem") })
    }).collect::<Vec<_>>()).into_response())
}

#[cfg(test)]
mod idem_tests {
    use super::*;
    async fn status(r: Result<Option<axum::response::Response>, ApiError>) -> u16 { match r { Ok(None) => 0, Ok(Some(resp)) => resp.status().as_u16(), Err(e) => e.0.as_u16() } }

    /// The claim is atomic, the same key with other parameters is a 422, only a failed run hands the key over.
    #[tokio::test]
    async fn claim_idem_dedups_and_hands_over_after_failure() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let run = |id: &str, st: &str| sqlx::query("INSERT INTO runs (id, app, workflow, version_hash, status, started_at, user_sub, params) VALUES ($1, 'a', 'w', 'h', $2, $3, 'u', '{}')").bind(id.to_string()).bind(st.to_string()).bind(chrono::Utc::now().to_rfc3339());
        // concurrent first requests: exactly one claims
        let (a, b) = tokio::join!(claim_idem(&db, "a", "k", "p1", "r1"), claim_idem(&db, "a", "k", "p1", "r2"));
        let (a, b) = (status(a).await, status(b).await);
        assert!((a == 0) ^ (b == 0), "{a} {b}");
        let owner = if a == 0 { "r1" } else { "r2" };
        assert_eq!(status(claim_idem(&db, "a", "k", "p1", "r3").await).await, 409);   // owner still starting
        run(owner, "running").execute(&db).await.unwrap();
        assert_eq!(status(claim_idem(&db, "a", "k", "p1", "r3").await).await, 202);
        assert_eq!(status(claim_idem(&db, "a", "k", "p2", "r3").await).await, 422);
        sqlx::query("UPDATE runs SET status = 'failed' WHERE id = $1").bind(owner).execute(&db).await.unwrap();
        assert_eq!(status(claim_idem(&db, "a", "k", "p1", "r4").await).await, 0);     // failed: runs again
        run("r4", "succeeded").execute(&db).await.unwrap();
        assert_eq!(status(claim_idem(&db, "a", "k", "p1", "r5").await).await, 200);
    }
}

// SPDX-License-Identifier: Apache-2.0
//! The approvals inbox: workflow gates, parked store changes, promotions and app deletions, decided by a person.
use super::*;

/// Pending approvals routed to one of the caller's roles, or to the caller as the run initiator. A front's gets no go-live
/// or app deletion (`platform_decision`).
pub(crate) async fn list_approvals(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    let platform = platform_decision(&st.cfg, front.is_some(), &headers);
    // a front's host: the approvals of its own stores' runs and changes (the live ones, or its release's)
    let scope = RunScope::of(&st, &app, front.as_ref().map(|f| &f.0)).await?;
    // workflow gates (routed to a role or the initiator), parked store changes (`store:` ids) and promotion requests
    // (`promote:` ids, routed to the server's approver roles) share one inbox
    let rows = sqlx::query("SELECT a.id, a.run_id, a.step, a.to_role, a.payload, a.created_at, COALESCE(r.user_sub, '') AS user_sub, COALESCE(r.workflow, a.step) AS workflow, r.store_target FROM approvals a LEFT JOIN runs r ON r.id = a.run_id WHERE a.app = $1 AND a.status = 'pending' ORDER BY a.created_at").bind(&app).fetch_all(&st.db).await?;
    let mut mine: Vec<Value> = rows.iter().filter(|r| platform || !is_platform_approval(&r.get::<String, _>("id")))
        .filter(|r| scope.sees(approval_target(&r.get::<String, _>("id"), r.get("store_target")).as_deref()))
        .filter(|r| { let to: String = r.get("to_role"); let owner: String = r.get("user_sub"); who.roles.contains(&to) || (to == "user:initiator" && owner == who.sub) || (to == "approver" && is_approver(&st, &who)) || who.is_admin() })
        .map(|r| { let payload = serde_json::from_str::<Value>(&r.get::<String, _>("payload")).unwrap_or(Value::Null); let requested_by = payload["requested_by"].as_str().map(String::from).unwrap_or_else(|| r.get::<String, _>("user_sub")); json!({ "id": r.get::<String, _>("id"), "kind": payload["kind"].as_str().unwrap_or("decision"), "run_id": r.get::<String, _>("run_id"), "step": r.get::<String, _>("step"), "to": r.get::<String, _>("to_role"), "payload": without_change(payload), "created_at": r.get::<String, _>("created_at"), "requested_by": requested_by, "workflow": r.get::<String, _>("workflow") }) }).collect();
    // who asked, named as the server saw them (`requester`): who decides may not read the organization's people
    let askers: Vec<String> = mine.iter().filter_map(|a| a["requested_by"].as_str().map(String::from)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let names = names_seen(&st, &askers).await?;
    for a in &mut mine { if let Some(n) = a["requested_by"].as_str().and_then(|s| names.get(s)) { a["requester"] = n.clone(); } }
    Ok(Json(mine).into_response())
}

/// The stores an approval's decision acts on: a parked store change's own (its id names them), else its run's (none
/// recorded: live).
pub(crate) fn approval_target(id: &str, run_target: Option<String>) -> Option<String> {
    if id.starts_with("store:") { return crate::stores::parse_approval_id(id).map(|(at, _, _)| at.target); }
    run_target
}

/// A store approval's payload without the parked change it keeps (the requester's arguments and the row as the requester's
/// role read it, which the approver's roles may not read): the inbox shows the summary fields (`fields`, `payload`, `message`).
pub(crate) fn without_change(mut payload: Value) -> Value {
    if let Some(o) = payload.as_object_mut() { o.remove("change"); }
    payload
}

pub(crate) async fn decide_approval(InRealm(st): InRealm, Path((app, id)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, Json(value): Json<Value>) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    if is_platform_approval(&id) && !platform_decision(&st.cfg, front.is_some(), &headers) {
        let what = if id.starts_with("delete:") { "an app's deletion" } else { "a go-live" };
        return Err(ApiError(StatusCode::FORBIDDEN, format!("{what} is decided in the console, in Slack or with `cori promote`, never through an app's front")));
    }
    // a front's host decides what its own stores' runs and changes wait on
    let scope = RunScope::of(&st, &app, front.as_ref().map(|f| &f.0)).await?;
    if scope != RunScope::All {
        let run_target: Option<Option<String>> = sqlx::query_scalar("SELECT r.store_target FROM approvals a LEFT JOIN runs r ON r.id = a.run_id WHERE a.id = $1 AND a.app = $2").bind(&id).bind(&app).fetch_optional(&st.db).await?;
        if !run_target.is_some_and(|t| scope.sees(approval_target(&id, t).as_deref())) { return Err(ApiError(StatusCode::NOT_FOUND, "no pending approval with that id".into())); }
    }
    decide_approval_as(&st, &app, &id, &who, value).await
}

/// Decides an approval as `who` (API, console, or a verified Slack action): store changes and workflow gates alike.
pub(crate) async fn decide_approval_as(st: &Arc<AppState>, app: &str, id: &str, who: &Principal, value: Value) -> Res {
    require_human(who, "deciding an approval")?;
    let (app, id) = (app.to_string(), id.to_string());
    if let Some(rest) = id.strip_prefix("promote:") {
        // a go-live request on a gated release (the one approval a release needs: the console inbox, Slack; an approver's own
        // `cori promote` also closes it): approve makes it live, decline parks it as rejected
        let (_, hash) = rest.split_once(':').ok_or(ApiError(StatusCode::BAD_REQUEST, "bad promotion id".into()))?;
        require_human(who, "deciding a promotion")?;
        require_power(st, who, &[Power::Approver], "deciding a go-live")?;
        sqlx::query("SELECT id FROM approvals WHERE id = $1 AND app = $2 AND status = 'pending'").bind(&id).bind(&app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "no pending promotion with that id".into()))?;
        let approve = value.get("approved").and_then(|b| b.as_bool()).unwrap_or(false);
        if !approve {
            sqlx::query("UPDATE approvals SET status = 'rejected', decided_by = $1, decided_at = $2 WHERE id = $3").bind(&who.sub).bind(chrono::Utc::now().to_rfc3339()).bind(&id).execute(&st.db).await?;
            db::audit(&st.db, &who.sub, "version.promote.declined", &format!("{app}@{hash}"), json!({ "note": value.get("note") })).await?;
            return Ok(Json(json!({ "ok": true, "approved": false, "version": hash })).into_response());
        }
        let out = do_promote(st, &app, hash, who).await?;
        return Ok(Json(json!({ "ok": true, "approved": true, "by": who.sub, "version": hash, "result": out })).into_response());
    }
    if id.starts_with("delete:") {
        // a parked app deletion: approve deletes the app, decline keeps it
        require_human(who, "deciding an app deletion")?;
        require_power(st, who, &[Power::Approver], "deciding an app deletion")?;
        let r = sqlx::query("SELECT payload FROM approvals WHERE id = $1 AND app = $2 AND status = 'pending'").bind(&id).bind(&app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "no pending app deletion with that id".into()))?;
        let requested_by = serde_json::from_str::<Value>(&r.get::<String, _>("payload")).ok().and_then(|p| p["requested_by"].as_str().map(String::from)).unwrap_or_default();
        if !value.get("approved").and_then(|b| b.as_bool()).unwrap_or(false) {
            sqlx::query("UPDATE approvals SET status = 'rejected', decided_by = $1, decided_at = $2 WHERE id = $3").bind(&who.sub).bind(chrono::Utc::now().to_rfc3339()).bind(&id).execute(&st.db).await?;
            db::audit(&st.db, &who.sub, "app.delete.declined", &app, json!({ "requested_by": requested_by, "note": value.get("note") })).await?;
            return Ok(Json(json!({ "ok": true, "approved": false, "app": app })).into_response());
        }
        let out = do_delete_app(st, &app, who, &requested_by).await?;
        return Ok(Json(json!({ "ok": true, "approved": true, "app": app, "result": out })).into_response());
    }
    if id.starts_with("store:") {
        // a parked store change, kept in its approvals row: approve applies it with the requesting role, reject drops it
        let (at, store, _) = crate::stores::parse_approval_id(&id).ok_or(ApiError(StatusCode::BAD_REQUEST, "bad store approval id".into()))?;
        // the caller's roles are theirs in this app: a change parked on another app's store is not decided from here
        if at.app != app { return Err(ApiError(StatusCode::NOT_FOUND, "no pending approval with that id".into())); }
        let r = sqlx::query("SELECT to_role, payload FROM approvals WHERE id = $1 AND app = $2 AND status = 'pending'").bind(&id).bind(app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "no pending approval with that id".into()))?;
        let to: String = r.get("to_role"); let payload: Value = serde_json::from_str(&r.get::<String, _>("payload")).unwrap_or(Value::Null);
        if !(who.roles.contains(&to) || who.is_store_admin()) { return Err(ApiError(StatusCode::FORBIDDEN, format!("approval is routed to `{to}`"))); }
        if payload["requested_by"].as_str() == Some(who.sub.as_str()) && !st.settings.allow_self_approval { return Err(ApiError(StatusCode::FORBIDDEN, "four eyes: the change was requested by you; another member of the role decides it".into())); }
        let approve = value.get("approved").and_then(|b| b.as_bool()).unwrap_or(false);
        let now = chrono::Utc::now();
        let expire = |why: &str| {
            let why = why.to_string();
            let (pool, id) = (st.db.clone(), id.clone());
            async move {
                sqlx::query("UPDATE approvals SET status = 'expired' WHERE id = $1 AND status = 'pending'").bind(&id).execute(&pool).await?;
                Err::<Response, ApiError>(ApiError(StatusCode::GONE, cori_store::DecideError::Gone(id, why).to_string()))
            }
        };
        let out = if !approve {
            let n = sqlx::query("UPDATE approvals SET status = 'decided', decided_by = $1, decided_at = $2 WHERE id = $3 AND status = 'pending'").bind(&who.sub).bind(now.to_rfc3339()).bind(&id).execute(&st.db).await?.rows_affected();
            if n != 1 { return Err(ApiError(StatusCode::CONFLICT, cori_store::DecideError::Decided(id.clone()).to_string())); }
            json!({ "rejected": payload["approval_id"] })
        } else {
            if payload["expires_at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t < now) { return expire("it expired").await; }
            // a change parked by an earlier server kept its data on that server's disk
            let Some(change) = payload.get("change").and_then(|c| serde_json::from_value::<cori_store::ParkedChange>(c.clone()).ok()) else { return expire("it was parked by an earlier version of Cori; ask for it again").await };
            match crate::stores::apply(st, at.clone(), store.clone(), change, id.clone(), who.sub.clone()).await {
                Ok(v) => v,
                Err(e) => return Err(match e.downcast_ref::<cori_store::DecideError>() {
                    Some(cori_store::DecideError::Decided(_) | cori_store::DecideError::Conflict(_)) => ApiError(StatusCode::CONFLICT, e.to_string()),
                    Some(cori_store::DecideError::Gone(..)) => ApiError(StatusCode::GONE, e.to_string()),
                    None => ApiError(crate::stores::status_of(&e), e.to_string()),
                }),
            }
        };
        db::audit(&st.db, &who.sub, "store.approval.decide", &id, json!({ "approved": approve, "note": value.get("note"), "result": out })).await?;
        return Ok(Json(json!({ "ok": true, "store": store, "target": at.target, "approved": approve, "result": out })).into_response());
    }
    decide_run_approval(&st, &app, &id, &who, value).await
}

/// Decides a pending `human` step: the decider holds the role it is routed to (an administrator may too), or is the
/// run's initiator when it is routed to them; the initiator never decides an approval routed to a role (four eyes).
pub(crate) async fn decide_run_approval(st: &Arc<AppState>, app: &str, id: &str, who: &Principal, value: Value) -> Res {
    require_human(who, "deciding an approval")?;
    let r = sqlx::query("SELECT a.run_id, a.step, a.to_role, r.user_sub FROM approvals a JOIN runs r ON r.id = a.run_id WHERE a.id = $1 AND a.app = $2 AND a.status = 'pending'").bind(id).bind(app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "no pending approval with that id".into()))?;
    let (run, step, to, owner): (String, String, String, String) = (r.get("run_id"), r.get("step"), r.get("to_role"), r.get("user_sub"));
    let to_initiator = to == "user:initiator";
    let allowed = if to_initiator { owner == who.sub || who.is_admin() } else { who.roles.contains(&to) || who.is_admin() };
    if !allowed { return Err(ApiError(StatusCode::FORBIDDEN, format!("approval is routed to `{to}`"))); }
    if !to_initiator && owner == who.sub && !st.settings.allow_self_approval { return Err(ApiError(StatusCode::FORBIDDEN, "four eyes: you started this run; another member of the role decides its approval".into())); }
    let handle = st.temporal.get_workflow_handle::<CoriWorkflow>(run.clone());
    handle.signal(CoriWorkflow::human_decision, DecisionArgs { step: step.clone(), value: value.clone() }, WorkflowSignalOptions::default()).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("signal: {e}")))?;
    sqlx::query("UPDATE approvals SET status = 'decided', decided_by = $1, decided_at = $2 WHERE id = $3").bind(&who.sub).bind(chrono::Utc::now().to_rfc3339()).bind(id).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "approval.decide", id, json!({ "run": run, "step": step, "value": value })).await?;
    Ok(Json(json!({ "ok": true, "run_id": run, "step": step })).into_response())
}

/// Slack interactive callback: buttons in the Slack notification decide an approval. The request is verified with
/// the Slack signing secret (`CORI_SLACK_SIGNING_SECRET`); the Slack user must be mapped to a Cori principal in the
/// settings of the button's realm (`CORI_SLACK_USERS`: `U0123=sam:support_lead,approver;U0456=jean:it_admin`), so roles
/// are never taken from Slack.
pub(crate) async fn slack_actions(InRealm(st): InRealm, headers: HeaderMap, body: String) -> Res {
    let secret = st.cfg.slack_signing_secret.clone().ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Slack actions are not configured (CORI_SLACK_SIGNING_SECRET)".into()))?;
    let ts = headers.get("x-slack-request-timestamp").and_then(|v| v.to_str().ok()).unwrap_or("");
    let sig = headers.get("x-slack-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    let now = chrono::Utc::now().timestamp();
    if ts.parse::<i64>().map(|t| (now - t).abs() > 300).unwrap_or(true) { return Err(ApiError(StatusCode::UNAUTHORIZED, "stale or missing Slack timestamp".into())); }
    let expected = { use hmac::{Hmac, KeyInit, Mac}; let mut m = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).unwrap(); m.update(format!("v0:{ts}:{body}").as_bytes()); format!("v0={}", hex::encode(m.finalize().into_bytes())) };
    if sig != expected { return Err(ApiError(StatusCode::UNAUTHORIZED, "bad Slack signature".into())); }
    let payload_s = body.strip_prefix("payload=").map(|p| urlencoding::decode(p).map(|c| c.into_owned()).unwrap_or_default()).unwrap_or(body.clone());
    let payload: Value = serde_json::from_str(&payload_s).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad Slack payload".into()))?;
    let action = payload["actions"][0]["value"].as_str().unwrap_or("").to_string();
    // the button names the realm of its approval (`names::slack_action`): the decision, and the Slack user's mapping, are that realm's
    let (verb, realm, app, id) = crate::names::parse_slack_action(&action);
    let verb = verb.as_str();
    if !(verb == "approve" || verb == "decline") || app.is_empty() || id.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "unknown action".into())); }
    let st = match (realm, &st.tenancy) {
        (Some(realm), _) => st.in_realm(realm).await?,
        (None, None) => st,
        (None, Some(_)) => return Err(ApiError(StatusCode::BAD_REQUEST, "the action names no organization".into())),
    };
    let slack_user = payload["user"]["id"].as_str().unwrap_or("").to_string();
    let mapping = &st.settings.slack_users;
    let mapped = mapping.split(';').filter_map(|e| e.split_once('=')).find(|(u, _)| u.trim() == slack_user).map(|(_, p)| p.trim().to_string()).ok_or(ApiError(StatusCode::FORBIDDEN, format!("Slack user {slack_user} is not mapped to a {} principal (CORI_SLACK_USERS)", st.brand.name)))?;
    let (sub, roles) = mapped.split_once(':').unwrap_or((&mapped, ""));
    let who = Principal { sub: sub.to_string(), roles: roles.split(',').filter(|r| !r.is_empty()).map(String::from).collect(), email: None, claims: json!({ "slack": slack_user }), token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
    let approved = verb == "approve";
    decide_approval_as(&st, &app, &id, &who, json!({ "approved": approved, "escalated": approved, "note": format!("from Slack by {}", payload["user"]["username"].as_str().unwrap_or(&slack_user)) })).await?;
    Ok(Json(json!({ "replace_original": true, "text": format!("{} {} by {} (via {})", if approved { ":white_check_mark: approved" } else { ":x: declined" }, id, who.sub, st.brand.name) })).into_response())
}

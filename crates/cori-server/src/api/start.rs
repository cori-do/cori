// SPDX-License-Identifier: Apache-2.0
//! An organization's first steps (`GET /v1/start`, the console's "Get started"): an agent connected over MCP, the command
//! line used, an LLM provider (the deployment's starter model until the org adds one), a first app, its first release to
//! try. Each step ticks itself from what the server
//! sees: the providers, apps and versions it keeps, and the clients that call it (`clients_seen`), which every MCP
//! session's start and every command line's call record (`client_seen`).
use super::*;

/// How often one process writes that a kind of client called a realm: often enough to say when it was last seen, and no
/// write per request.
const SEEN_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// Records that a client of `kind` (`mcp`, `cli`) called, as what it says it is (`detail`), for `by`. A failure to write
/// is logged, never the caller's.
pub(crate) async fn client_seen(st: &AppState, kind: &str, detail: &str, by: &str) {
    let key = format!("{}/{kind}", st.realm().map(|r| r.as_str().to_string()).unwrap_or_default());
    {
        let mut seen = st.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.get(&key).is_some_and(|at| at.elapsed() < SEEN_EVERY) { return; }
        seen.insert(key, std::time::Instant::now());
    }
    let detail: String = detail.chars().filter(|c| !c.is_control()).take(120).collect();
    let now = chrono::Utc::now().to_rfc3339();
    let r = sqlx::query("INSERT INTO clients_seen (kind, first_at, last_at, detail, by_sub) VALUES ($1, $2, $2, $3, $4) ON CONFLICT ON CONSTRAINT clients_seen_pkey DO UPDATE SET last_at = EXCLUDED.last_at, detail = EXCLUDED.detail, by_sub = EXCLUDED.by_sub")
        .bind(kind).bind(&now).bind(&detail).bind(by).execute(&st.db).await;
    if let Err(e) = r { tracing::warn!(kind, error = %e, "could not record a client's call"); }
}

/// The first steps, for the caller: the clients its organization's people connected, whether an LLM provider is there
/// (and whether the caller may add one), and among the apps the caller builds the first one and the newest release, with
/// the address to try it at.
pub(crate) async fn get_started(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    let client = async |kind: &str| -> Result<Value, ApiError> {
        let row = sqlx::query("SELECT first_at, last_at, detail, by_sub FROM clients_seen WHERE kind = $1").bind(kind).fetch_optional(&st.db).await?;
        Ok(match row {
            Some(r) => json!({ "done": true, "first_at": r.get::<String, _>("first_at"), "last_at": r.get::<String, _>("last_at"), "detail": r.get::<String, _>("detail"), "by": r.get::<String, _>("by_sub") }),
            None => json!({ "done": false }),
        })
    };
    let providers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM llm_providers WHERE enabled <> 0").fetch_one(&st.db).await?;
    // the deployment's starter model answers until the org has a provider of its own: the step needs nothing then
    let starter = match &st.cfg.llm_starter { Some(s) => s.status(&st.db).await?, None => Value::Null };
    let apps: Vec<String> = built_apps(&st, &who).await?.iter().filter_map(|a| a["id"].as_str().map(String::from)).collect();
    let newest = if apps.is_empty() { None } else {
        sqlx::query("SELECT app, hash, seq FROM versions WHERE app = ANY($1) ORDER BY created_at DESC LIMIT 1").bind(&apps).fetch_optional(&st.db).await?
    };
    let release = match newest {
        Some(r) => {
            let (app, hash): (String, String) = (r.get("app"), r.get("hash"));
            let short = hash.trim_start_matches("sha256:").get(..12).unwrap_or_default().to_string();
            json!({ "done": true, "app": app, "release": r.get::<Option<i64>, _>("seq").map(|s| format!("v{s}")), "preview_url": r.get::<Option<i64>, _>("seq").map(|n| st.release_url(&app, n)).unwrap_or_else(|| format!("{}/apps/{app}/@{short}/", st.front_origin(&app))) })
        }
        None => json!({ "done": false }),
    };
    Ok(Json(json!({
        // the address to give the organization's people: their apps, each opened where it lives
        "portal_url": st.portal_url(),
        "agent": client("mcp").await?,
        "cli": client("cli").await?,
        "llm": { "done": providers > 0 || !starter.is_null(), "own": providers > 0, "starter": starter, "can_add": who.is_admin() && !who.is_agent() },
        "app": { "done": !apps.is_empty(), "id": release.get("app").and_then(|a| a.as_str()).or(apps.first().map(String::as_str)) },
        "release": release,
    })).into_response())
}

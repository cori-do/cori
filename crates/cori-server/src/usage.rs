// SPDX-License-Identifier: Apache-2.0
//! The usage ledger: what a realm used, day by day (`usage`), whatever the retention of runs and captured calls
//! (`CORI_RETENTION_DAYS`) deletes. A realm's limits (`realm::Limits`) are checked against it, and billing reads it
//! (`GET /v1/usage`).
//!
//! - Counters, per app, added to as things happen: `runs` (a run started, a scheduled one included), `run_ms` (the time of
//!   the runs that ended), `llm_calls`, `llm_tokens_in`, `llm_tokens_out` and `llm_eur` (a model call, as the server priced
//!   it: `llm::price`), `deploys`.
//! - Gauges, the day's last reading, for the realm as a whole (`app` empty) and for each app: `storage_bytes` (store
//!   databases, versions' bundles, files), measured every few minutes by the leading replica (`measure`). The realm's is
//!   checked against `Limits::storage_bytes`, an app's against its own limit within `Limits::storage_bytes_per_app`
//!   (`AppStorageLimit`).
//!
//! Recording never fails what it records: a ledger the database refused is logged.
use crate::{api::ApiError, db::Db, realm::Limits, state::AppState};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::Row;

/// The counters of the ledger, as `summary` reports them.
pub const COUNTERS: [&str; 7] = ["runs", "run_ms", "llm_calls", "llm_tokens_in", "llm_tokens_out", "llm_eur", "deploys"];

/// Today (UTC), as the ledger's `day`.
pub fn today() -> String { chrono::Utc::now().format("%Y-%m-%d").to_string() }

/// The first day of this month (UTC).
pub fn month_first() -> String { chrono::Utc::now().format("%Y-%m-01").to_string() }

/// Adds `quantity` to today's `metric` of `app`.
pub async fn add(db: &Db, metric: &str, app: &str, quantity: f64) {
    if quantity == 0.0 { return; }
    let r = sqlx::query("INSERT INTO usage (day, metric, app, quantity) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT usage_pkey DO UPDATE SET quantity = usage.quantity + EXCLUDED.quantity")
        .bind(today()).bind(metric).bind(app).bind(quantity).execute(db).await;
    if let Err(e) = r { tracing::warn!(metric, app, quantity, error = %e, "usage not recorded"); }
}

/// Records a gauge's reading: today's is the last one.
pub async fn gauge(db: &Db, metric: &str, app: &str, quantity: f64) {
    let r = sqlx::query("INSERT INTO usage (day, metric, app, quantity) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT usage_pkey DO UPDATE SET quantity = EXCLUDED.quantity")
        .bind(today()).bind(metric).bind(app).bind(quantity).execute(db).await;
    if let Err(e) = r { tracing::warn!(metric, app, quantity, error = %e, "usage not recorded"); }
}

/// A counter this month (UTC), of `app` or of every app.
pub async fn month(db: &Db, metric: &str, app: Option<&str>) -> Result<f64, sqlx::Error> {
    let q = match app {
        Some(a) => sqlx::query("SELECT COALESCE(SUM(quantity), 0) AS q FROM usage WHERE metric = $1 AND day >= $2 AND app = $3").bind(metric).bind(month_first()).bind(a.to_string()),
        None => sqlx::query("SELECT COALESCE(SUM(quantity), 0) AS q FROM usage WHERE metric = $1 AND day >= $2").bind(metric).bind(month_first()),
    };
    Ok(q.fetch_one(db).await?.get::<f64, _>("q"))
}

/// A gauge's last reading for the realm as a whole, 0 before the first.
pub async fn latest(db: &Db, metric: &str) -> Result<f64, sqlx::Error> { latest_of(db, metric, "").await }

/// A gauge's last reading for one app (`''`: the realm as a whole), 0 before the first.
pub async fn latest_of(db: &Db, metric: &str, app: &str) -> Result<f64, sqlx::Error> {
    Ok(sqlx::query("SELECT quantity FROM usage WHERE metric = $1 AND app = $2 ORDER BY day DESC LIMIT 1").bind(metric).bind(app).fetch_optional(db).await?.map(|r| r.get::<f64, _>("quantity")).unwrap_or(0.0))
}

/// The most storage one app may take: its own limit, set by the organization's admins (`app_limits`), within the
/// organization's for every app (`Limits::storage_bytes_per_app`). 0 is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppStorageLimit { pub own: Option<u64>, pub per_app: u64 }

impl AppStorageLimit {
    /// The lower of the two that are set; 0 when neither is.
    pub fn effective(&self) -> u64 {
        match (self.own, self.per_app) { (Some(o), 0) => o, (Some(o), p) => o.min(p), (None, p) => p }
    }
}

/// `app`'s storage limit under the realm's `limits`.
pub async fn app_storage_limit(db: &Db, limits: &Limits, app: &str) -> Result<AppStorageLimit, sqlx::Error> {
    let own: Option<i64> = sqlx::query_scalar("SELECT storage_bytes FROM app_limits WHERE app = $1").bind(app).fetch_optional(db).await?.flatten();
    Ok(AppStorageLimit { own: own.filter(|n| *n > 0).map(|n| n as u64), per_app: limits.storage_bytes_per_app })
}

/// Whether `used`, plus `adding` bytes, is over `max`: with nothing added (a store write), once `used` reached it.
fn full(used: f64, adding: u64, max: u64) -> bool { if adding == 0 { used >= max as f64 } else { used + adding as f64 > max as f64 } }

/// Why adding `adding` bytes to `app` (`what`: "this bundle", "this file"; 0 bytes: a store write) is refused, the
/// organization's limit first, then the app's; None when it fits. As last measured: a few minutes behind.
pub async fn storage_refusal(db: &Db, limits: &Limits, app: &str, adding: u64, what: &str) -> Result<Option<String>, sqlx::Error> {
    if limits.storage_bytes > 0 {
        let used = latest(db, "storage_bytes").await?;
        if full(used, adding, limits.storage_bytes) {
            return Ok(Some(if adding == 0 { format!("stores, bundles and files take {}, the {} allowed here: writes resume once data or old versions are deleted", human(used), human(limits.storage_bytes as f64)) }
                else { format!("stores, bundles and files take {}, and {what} {} more, over the {} allowed here: delete old versions or data first", human(used), human(adding as f64), human(limits.storage_bytes as f64)) }));
        }
    }
    let max = app_storage_limit(db, limits, app).await?.effective();
    if max > 0 {
        let used = latest_of(db, "storage_bytes", app).await?;
        if full(used, adding, max) {
            return Ok(Some(if adding == 0 { format!("`{app}` takes {} (its stores, bundles and files), the {} allowed for it: its writes resume once its data or old versions are deleted", human(used), human(max as f64)) }
                else { format!("`{app}` takes {} (its stores, bundles and files), and {what} {} more, over the {} allowed for it: delete its old versions or data first", human(used), human(adding as f64), human(max as f64)) }));
        }
    }
    Ok(None)
}

/// An app's storage as the console shows it: what it takes (last measured), its limit (`own`, `per_app`, the `limit`
/// that applies), and the organization's.
pub async fn app_storage(db: &Db, limits: &Limits, app: &str) -> Result<Value, sqlx::Error> {
    let l = app_storage_limit(db, limits, app).await?;
    let n = |v: u64| if v == 0 { Value::Null } else { json!(v) };
    Ok(json!({
        "bytes": latest_of(db, "storage_bytes", app).await? as u64,
        "limit": n(l.effective()), "own": l.own, "per_app": n(l.per_app),
        "organization": { "bytes": latest(db, "storage_bytes").await? as u64, "limit": n(limits.storage_bytes) },
    }))
}

/// A limit reached: 402, saying which.
fn over(msg: String) -> ApiError { ApiError(StatusCode::PAYMENT_REQUIRED, format!("limit reached: {msg}")) }

/// Before an app is created (`Limits::apps`).
pub async fn check_apps(st: &AppState) -> Result<(), ApiError> {
    let max = st.settings.limits.apps;
    if max == 0 { return Ok(()); }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM apps").fetch_one(&st.db).await?;
    if n as u64 >= max { return Err(over(format!("{n} apps, the most allowed here: delete one first"))); }
    Ok(())
}

/// Before a bundle of `bytes` is taken apart (`Limits::bundle_bytes`): 413.
pub fn check_bundle(st: &AppState, bytes: usize) -> Result<(), ApiError> {
    let max = st.settings.limits.bundle_bytes;
    if max > 0 && bytes as u64 > max { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("limit reached: the bundle is {}, over the {} allowed here (fixtures and front builds count: leave out what the app does not need)", human(bytes as f64), human(max as f64)))); }
    Ok(())
}

/// Before the version `hash` of `app` is recorded (`Limits::versions_per_app`, `Limits::storage_bytes`). Deploying a
/// version the app already has again takes no more room.
pub async fn check_deploy(st: &AppState, app: &str, hash: &str, bytes: usize) -> Result<(), ApiError> {
    let l = &st.settings.limits;
    let known: bool = sqlx::query("SELECT 1 FROM versions WHERE app = $1 AND hash = $2").bind(app).bind(hash).fetch_optional(&st.db).await?.is_some();
    if known { return Ok(()); }
    if l.versions_per_app > 0 {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM versions WHERE app = $1").bind(app).fetch_one(&st.db).await?;
        if n as u64 >= l.versions_per_app { return Err(over(format!("`{app}` keeps {n} versions, the most allowed here: delete an old one first (`cori undeploy {app} <version>`)"))); }
    }
    if let Some(why) = storage_refusal(&st.db, l, app, (bytes as u64).max(1), "this bundle").await? { return Err(over(why)); }
    Ok(())
}

/// Before an upload of `bytes` to `app` is announced (`Limits::storage_bytes`, the app's limit): files count with the
/// stores and the bundles.
pub async fn check_upload(st: &AppState, app: &str, bytes: u64) -> Result<(), ApiError> {
    match storage_refusal(&st.db, &st.settings.limits, app, bytes.max(1), "this file").await? { Some(why) => Err(over(why)), None => Ok(()) }
}

/// Before a run starts (`Limits::runs_per_month`).
pub async fn check_run(st: &AppState) -> Result<(), ApiError> {
    let max = st.settings.limits.runs_per_month;
    if max == 0 { return Ok(()); }
    let n = month(&st.db, "runs", None).await?;
    if n >= max as f64 { return Err(over(format!("{n} runs started this month, the most allowed here: runs start again next month"))); }
    Ok(())
}

/// Why a write to one of `app`'s stores is refused now (`Limits::storage_bytes`, the app's limit): reads and deletions
/// stay possible.
pub async fn storage_full(st: &AppState, app: &str) -> Result<Option<String>, ApiError> {
    Ok(storage_refusal(&st.db, &st.settings.limits, app, 0, "").await?.map(|why| format!("limit reached: {why}")))
}

/// Measures the realm's storage, app by app (its store databases, through each one's cluster admin, its versions'
/// bundles and the files its runs were given), and records each app's and their sum (`storage_bytes`).
pub async fn measure(st: &AppState) -> anyhow::Result<f64> {
    // every app has a reading, 0 included: one whose data went does not keep its last one
    let mut by_app: std::collections::BTreeMap<String, f64> = sqlx::query_scalar::<_, String>("SELECT id FROM apps").fetch_all(&st.db).await?.into_iter().map(|a| (a, 0.0)).collect();
    for (app, n) in sqlx::query_as::<_, (String, Option<i64>)>("SELECT app, CAST(SUM(bytes) AS BIGINT) FROM versions GROUP BY app").fetch_all(&st.db).await? { *by_app.entry(app).or_default() += n.unwrap_or(0) as f64; }
    for (app, n) in sqlx::query_as::<_, (String, Option<i64>)>("SELECT app, CAST(SUM(size) AS BIGINT) FROM files WHERE status = 'ready' GROUP BY app").fetch_all(&st.db).await? { *by_app.entry(app).or_default() += n.unwrap_or(0) as f64; }
    for (app, n) in measure_stores(st).await? { *by_app.entry(app).or_default() += n; }
    let total: f64 = by_app.values().sum();
    for (app, n) in &by_app { gauge(&st.db, "storage_bytes", app, *n).await; }
    gauge(&st.db, "storage_bytes", "", total).await;
    Ok(total)
}

/// The size of each app's store databases (live and sandboxes), through each one's cluster admin.
async fn measure_stores(st: &AppState) -> anyhow::Result<std::collections::BTreeMap<String, f64>> {
    let mut by_cluster: std::collections::BTreeMap<String, Vec<(String, String)>> = Default::default();
    for (app, cluster, db) in sqlx::query_as::<_, (String, String, String)>("SELECT app, cluster, db_name FROM store_instances").fetch_all(&st.db).await? { by_cluster.entry(cluster).or_default().push((app, db)); }
    let clusters = st.cfg.store_clusters();
    let mut out: std::collections::BTreeMap<String, f64> = Default::default();
    for (cluster, dbs) in by_cluster {
        let Some(admin) = clusters.get(&cluster) else { tracing::warn!(cluster = %cluster, "storage: a store cluster is not configured"); continue };
        use sqlx::Connection;
        let mut c = sqlx::PgConnection::connect(admin).await?;
        let names: Vec<String> = dbs.iter().map(|(_, d)| d.clone()).collect();
        let sizes: std::collections::HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>("SELECT datname::text, pg_database_size(datname) FROM pg_database WHERE datname = ANY($1)").bind(&names).fetch_all(&mut c).await?.into_iter().collect();
        let _ = c.close().await;
        for (app, db) in dbs { *out.entry(app).or_default() += sizes.get(&db).copied().unwrap_or(0) as f64; }
    }
    Ok(out)
}

/// This month's counters (every app together), the gauges' last readings and the limits.
pub async fn summary(st: &AppState) -> Result<Value, sqlx::Error> {
    let mut month_counters = serde_json::Map::new();
    for m in COUNTERS { month_counters.insert(m.to_string(), json!(month(&st.db, m, None).await?)); }
    // each app's last storage reading (the apps there are now)
    let by_app: serde_json::Map<String, Value> = sqlx::query_as::<_, (String, f64)>("SELECT DISTINCT ON (u.app) u.app, u.quantity FROM usage u JOIN apps a ON a.id = u.app WHERE u.metric = 'storage_bytes' ORDER BY u.app, u.day DESC")
        .fetch_all(&st.db).await?.into_iter().map(|(app, q)| (app, json!(q as u64))).collect();
    Ok(json!({ "month": chrono::Utc::now().format("%Y-%m").to_string(), "counters": month_counters, "storage_bytes": latest(&st.db, "storage_bytes").await?, "storage_bytes_by_app": by_app, "limits": st.settings.limits.json() }))
}

/// A size for people, in decimal units: `850 B`, `12.4 KB`, `3.0 MB`, `1.2 GB`.
pub(crate) fn human(bytes: f64) -> String {
    let (v, unit) = if bytes >= 1e9 { (bytes / 1e9, "GB") } else if bytes >= 1e6 { (bytes / 1e6, "MB") } else if bytes >= 1e3 { (bytes / 1e3, "KB") } else { return format!("{bytes:.0} B") };
    format!("{v:.1} {unit}")
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_ledger_adds_counters_and_keeps_a_gauges_last_reading() {
        let Some(db) = crate::db::test_db().await else { return };
        super::add(&db, "runs", "desk", 1.0).await;
        super::add(&db, "runs", "desk", 2.0).await;
        super::add(&db, "runs", "crm", 1.0).await;
        super::gauge(&db, "storage_bytes", "", 500.0).await;
        super::gauge(&db, "storage_bytes", "", 300.0).await;
        assert_eq!(super::month(&db, "runs", Some("desk")).await.unwrap(), 3.0);
        assert_eq!(super::month(&db, "runs", None).await.unwrap(), 4.0);
        assert_eq!(super::latest(&db, "storage_bytes").await.unwrap(), 300.0, "the last reading, not the day's highest");
        assert_eq!(super::month(&db, "llm_eur", None).await.unwrap(), 0.0);
    }

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!((super::human(850.0), super::human(12_400.0), super::human(3_000_000.0), super::human(1_234_000_000.0)), ("850 B".into(), "12.4 KB".into(), "3.0 MB".into(), "1.2 GB".into()));
    }

    #[test]
    fn an_apps_limit_is_the_lower_of_its_own_and_the_organizations() {
        use super::AppStorageLimit as L;
        assert_eq!(L { own: None, per_app: 0 }.effective(), 0, "none");
        assert_eq!(L { own: None, per_app: 1000 }.effective(), 1000, "the organization's for each app");
        assert_eq!(L { own: Some(500), per_app: 0 }.effective(), 500, "its own");
        assert_eq!(L { own: Some(500), per_app: 1000 }.effective(), 500);
        assert_eq!(L { own: Some(5000), per_app: 1000 }.effective(), 1000, "never past the organization's, whatever was set before");
    }

    /// The organization's limit first, then the app's (its own within the organization's for each app); an addition is
    /// refused when it would go over, a store write once the limit is reached. Needs CORI_STORE_DATABASE_URL.
    #[tokio::test]
    async fn storage_is_refused_past_the_organizations_or_the_apps_limit() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let mut limits = crate::realm::Limits::default();
        super::gauge(&db, "storage_bytes", "", 3_000.0).await;
        super::gauge(&db, "storage_bytes", "desk", 2_000.0).await;
        super::gauge(&db, "storage_bytes", "crm", 1_000.0).await;
        let refusal = |app: &'static str, adding: u64, limits: crate::realm::Limits| { let db = db.clone(); async move { super::storage_refusal(&db, &limits, app, adding, "this file").await.unwrap() } };
        assert_eq!(refusal("desk", 10_000, limits.clone()).await, None, "no limit");
        // the organization's
        limits.storage_bytes = 3_500;
        let why = refusal("crm", 600, limits.clone()).await.unwrap();
        assert!(why.starts_with("stores, bundles and files take") && why.contains("this file"), "{why}");
        assert_eq!(refusal("crm", 400, limits.clone()).await, None);
        assert_eq!(refusal("crm", 0, limits.clone()).await, None, "a store write while under it");
        // the organization's for each app
        limits.storage_bytes_per_app = 2_000;
        let why = refusal("desk", 0, limits.clone()).await.unwrap();
        assert!(why.starts_with("`desk` takes") && why.contains("its writes resume"), "a store write once desk reached it: {why}");
        assert_eq!(refusal("crm", 400, limits.clone()).await, None, "crm is under it");
        // an app's own, lower
        sqlx::query("INSERT INTO app_limits (app, storage_bytes, updated_at, updated_by) VALUES ('crm', 1200, 'now', 'jean')").execute(&db).await.unwrap();
        let why = refusal("crm", 300, limits.clone()).await.unwrap();
        assert!(why.starts_with("`crm` takes") && why.contains("over the"), "{why}");
        assert_eq!(super::app_storage_limit(&db, &limits, "crm").await.unwrap(), super::AppStorageLimit { own: Some(1200), per_app: 2_000 });
        let shown = super::app_storage(&db, &limits, "crm").await.unwrap();
        assert_eq!((shown["bytes"].as_u64(), shown["limit"].as_u64(), shown["own"].as_u64(), shown["per_app"].as_u64()), (Some(1000), Some(1200), Some(1200), Some(2000)), "{shown}");
        assert_eq!((shown["organization"]["bytes"].as_u64(), shown["organization"]["limit"].as_u64()), (Some(3000), Some(3500)));
        // an app never measured: nothing taken yet (the organization's limit lifted, which would refuse it first)
        limits.storage_bytes = 0;
        assert_eq!(refusal("new", 1_500, limits.clone()).await, None);
        assert!(refusal("new", 2_500, limits).await.is_some_and(|w| w.starts_with("`new` takes")));
    }
}

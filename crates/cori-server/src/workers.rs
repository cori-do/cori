// SPDX-License-Identifier: Apache-2.0
//! Who is calling the worker endpoints (events, leases, store calls as a run's user), and what a worker may ask for.
//!
//! A worker is one of: the cluster's shared workers (`X-Cori-Worker: <CORI_INGEST_TOKEN>`), a named pool
//! (`X-Cori-Pool: <name>` + `X-Cori-Pool-Key: <key>`, the key created by `cori pools create`; the shared token is never
//! handed to a pool), or, on a laptop with no token configured, local. A lease names a run and one of the connectors
//! its app declares: the secret and the audience must be that connector's, and a connector pinned to a pool is served
//! by that pool only.
use crate::api::ApiError;
use crate::state::AppState;
use axum::http::{HeaderMap, StatusCode};
use cori_engine::config::{AppConfig, ConnectorConfig};
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Worker {
    /// No worker token configured (laptop): any local process may call.
    Local,
    /// The cluster's shared (and sandbox) workers.
    Shared,
    /// A named pool, authenticated by its own key.
    Pool(String),
}

impl Worker {
    pub fn label(&self) -> String { match self { Worker::Local => "local".into(), Worker::Shared => "shared".into(), Worker::Pool(p) => format!("pool:{p}") } }
}

/// Marks a request that came in on the workers' own address (`ServerConfig::worker_listen`).
#[derive(Debug, Clone, Copy)]
pub struct InternalListener;

/// A worker endpoint's view of where its request came in: `Some` on the workers' address.
pub type Via = Option<axum::Extension<InternalListener>>;

/// Compares two secrets without an early exit on the first differing byte.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn sha_hex(s: &str) -> String { use sha2::Digest; hex::encode(sha2::Sha256::digest(s.as_bytes())) }

/// The worker behind a request, or 401. With a workers' address configured (`CORI_WORKER_LISTEN`), the cluster's token is
/// refused anywhere else, whatever its value: the public address never takes it. Pools use their own keys everywhere.
pub async fn authenticate(st: &AppState, headers: &HeaderMap, via: &Via) -> Result<Worker, ApiError> {
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    if let (Some(pool), Some(key)) = (header("x-cori-pool"), header("x-cori-pool-key")) {
        let row = sqlx::query("SELECT key_hash FROM pools WHERE name = $1").bind(pool).fetch_optional(&st.db).await?;
        return match row {
            Some(r) if ct_eq(r.get::<String, _>("key_hash").as_bytes(), sha_hex(key).as_bytes()) => Ok(Worker::Pool(pool.to_string())),
            _ => Err(ApiError(StatusCode::UNAUTHORIZED, "bad pool key".into())),
        };
    }
    if st.cfg.ingest_token.is_empty() { return Ok(Worker::Local); }
    match header("x-cori-worker") {
        Some(_) if st.cfg.worker_listen.is_some() && via.is_none() => Err(ApiError(StatusCode::UNAUTHORIZED, "the cluster's worker token is accepted on the workers' address only (CORI_WORKER_LISTEN): set the workers' CORI_INTERNAL_URL to it".into())),
        Some(t) if ct_eq(t.as_bytes(), st.cfg.ingest_token.as_bytes()) => Ok(Worker::Shared),
        _ => Err(ApiError(StatusCode::UNAUTHORIZED, "worker token required (X-Cori-Worker, or X-Cori-Pool + X-Cori-Pool-Key)".into())),
    }
}

/// The connectors a run's app may use: its version's `cori.toml` merged with the org registry as a hosted run sees it
/// (the registry's connectors whole, no credential of the app's own).
pub async fn app_connectors(st: &Arc<AppState>, app: &str, version: &str) -> Result<BTreeMap<String, ConnectorConfig>, ApiError> {
    let row = sqlx::query("SELECT cori_toml FROM versions WHERE app = $1 AND hash = $2").bind(app).bind(version).fetch_optional(&st.db).await?
        .ok_or(ApiError(StatusCode::FORBIDDEN, "the run's version is unknown".into()))?;
    let bdir = st.bundles.path(st.db.realm(), version);
    let mut cfg = AppConfig::from_toml(&row.get::<String, _>("cori_toml"), &bdir).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut reg = BTreeMap::new();
    for r in crate::api::registry_rows(st).await? {
        if let (Some(n), Ok(c)) = (r["name"].as_str(), serde_json::from_value::<ConnectorConfig>(r["config"].clone())) { reg.insert(n.to_string(), c); }
    }
    cfg.merge_registry_hosted(&reg, st.cfg.strict_connectors());
    Ok(cfg.connectors)
}

/// A run as the worker endpoints see it.
pub struct RunRef { pub app: String, pub version: String, pub status: String }

pub async fn run_ref(st: &AppState, run_id: &str) -> Result<Option<RunRef>, ApiError> {
    Ok(sqlx::query("SELECT app, version_hash, status FROM runs WHERE id = $1").bind(run_id).fetch_optional(&st.db).await?
        .map(|r| RunRef { app: r.get("app"), version: r.get("version_hash"), status: r.get("status") }))
}

/// Whether `worker` may act for runs of this app at all: shared and local workers serve every app; a pool only apps
/// with a connector pinned to it.
pub fn serves_app(worker: &Worker, connectors: &BTreeMap<String, ConnectorConfig>) -> bool {
    match worker {
        Worker::Local | Worker::Shared => true,
        Worker::Pool(p) => connectors.values().any(|c| c.worker.as_deref() == Some(p.as_str())),
    }
}

/// Whether `worker` may lease for `connector`: a connector pinned to a pool is served by that pool only (or locally);
/// a pool serves only the connectors pinned to it.
pub fn serves_connector(worker: &Worker, connector: &ConnectorConfig) -> bool {
    match (worker, connector.worker.as_deref()) {
        (Worker::Local, _) => true,
        (Worker::Shared, None) => true,
        (Worker::Shared, Some(_)) => false,
        (Worker::Pool(p), Some(pinned)) => p == pinned,
        (Worker::Pool(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(worker: Option<&str>) -> ConnectorConfig { serde_json::from_value(serde_json::json!({ "kind": "mcp", "worker": worker })).unwrap() }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[test]
    fn routing_rules() {
        let pool = Worker::Pool("billing".into());
        assert!(serves_connector(&Worker::Shared, &conn(None)));
        assert!(!serves_connector(&Worker::Shared, &conn(Some("billing"))));
        assert!(serves_connector(&pool, &conn(Some("billing"))));
        assert!(!serves_connector(&pool, &conn(Some("other"))));
        assert!(!serves_connector(&pool, &conn(None)));
        assert!(serves_connector(&Worker::Local, &conn(Some("billing"))));
        let mut app = BTreeMap::new();
        app.insert("helpdesk".to_string(), conn(None));
        assert!(!serves_app(&pool, &app));
        app.insert("billing".to_string(), conn(Some("billing")));
        assert!(serves_app(&pool, &app));
    }
}

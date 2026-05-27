//! `cori serve start`.
//!
//! Boots a local HTTP server on `127.0.0.1:7510` that exposes the
//! registry as a JSON API and serves a minimal embedded web UI. This is
//! the developer-loop UI: see your workflows, click "Run", watch traces
//! update — *not* the management plane (that's v2).
//!
//! ## Endpoints
//! - `GET  /api/workflows`              — list registered workflows
//! - `GET  /api/workflows/:id`          — workflow detail (manifest + compiled DAG)
//! - `POST /api/workflows/:id/run`      — trigger an async run; returns `{ run_id }` immediately
//! - `GET  /api/runs`                   — list recent runs; `?workflow_id=` to filter
//! - `GET  /api/runs/:run_id`           — full run trace (includes `status: "running"` while in-flight)
//!
//! ## Safety
//! The server refuses to bind to a non-loopback address unless
//! `--insecure` is set, with a warning. There is no auth in v1: the
//! assumption is "single user on a developer workstation".

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::registry;

const DEFAULT_BIND: &str = "127.0.0.1:7510";

/// Embedded single-page UI. Vanilla JS + Tailwind via CDN, intentionally
/// minimal — the roadmap calls for a React build but for v1 the tradeoff
/// of "no Node toolchain required for `cargo build`" outweighs the
/// framework choice. Documented in `docs/architecture.md` once written.
const INDEX_HTML: &str = include_str!("../../ui/index.html");

pub fn start(bind: Option<String>, insecure: bool) -> Result<()> {
    let addr_str = bind.as_deref().unwrap_or(DEFAULT_BIND);
    let addr: SocketAddr = addr_str
        .parse()
        .with_context(|| format!("invalid bind address `{addr_str}`"))?;

    let is_loopback =
        matches!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST)) || addr.ip().is_loopback();

    if !is_loopback && !insecure {
        bail!(
            "refusing to bind to non-loopback address `{addr}` without --insecure.\n\
             There is no authentication on the Cori HTTP API in v1; binding to a\n\
             public interface exposes your workflows and registry to anyone on the\n\
             network. Re-run with --insecure if you understand the risk."
        );
    }

    if !is_loopback {
        eprintln!("warning: binding to non-loopback address `{addr}` — no auth, no TLS");
    }

    let app = router();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("axum serve")
    })
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn router() -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/workflows/{id}", get(serve_index))
        .route("/runs/{run_id}", get(serve_index))
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/{id}", get(get_workflow))
        .route("/api/workflows/{id}/run", post(post_run))
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{run_id}", get(get_run))
        .with_state(())
}

// ---------- UI ----------

async fn serve_index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

// ---------- API ----------

type ApiResult<T> = Result<Json<T>, ApiError>;

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn internal(e: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

async fn list_workflows(State(_): State<()>) -> ApiResult<JsonValue> {
    let reg = registry::open().map_err(ApiError::internal)?;
    let rows = reg.list().map_err(ApiError::internal)?;
    Ok(Json(
        serde_json::to_value(rows).map_err(ApiError::internal)?,
    ))
}

async fn get_workflow(Path(id): Path<String>) -> ApiResult<JsonValue> {
    let reg = registry::open().map_err(ApiError::internal)?;
    let detail = reg
        .get(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no workflow `{id}`")))?;
    Ok(Json(serde_json::json!({
        "id": detail.id,
        "version": detail.version,
        "source_path": detail.source_path,
        "registered_at": detail.registered_at,
        "manifest_yaml": detail.manifest_yaml,
        "compiled": detail.compiled,
    })))
}

#[derive(Debug, Deserialize, Default)]
struct RunRequest {
    #[serde(default)]
    params: JsonMap<String, JsonValue>,
    #[serde(default)]
    dry_run: bool,
}

async fn post_run(Path(id): Path<String>, body: Option<Json<RunRequest>>) -> ApiResult<JsonValue> {
    let req = body.map(|Json(b)| b).unwrap_or_default();

    // Look up the workflow up-front so we can fail with 404 *before*
    // spawning a background task.
    let reg = registry::open().map_err(ApiError::internal)?;
    let detail = reg
        .get(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no workflow `{id}`")))?;

    // Merge: manifest defaults < request body overrides.
    let mut initial: JsonMap<String, JsonValue> = JsonMap::new();
    for param in &detail.compiled.manifest.parameters {
        if let Some(default) = &param.default {
            let v = serde_json::to_value(default).map_err(ApiError::internal)?;
            initial.insert(param.name.clone(), v);
        }
    }
    for (k, v) in req.params {
        initial.insert(k, v);
    }
    let initial_params = JsonValue::Object(initial);
    let dry_run_mode = req.dry_run;

    // Drop the registry handle before spawning so the background thread
    // can re-open it without colliding on the WAL.
    drop(reg);

    // Pre-generate the run_id so we can return it immediately and the
    // background task records under the same id.
    let run_id = super::run::new_run_id();
    let workflow_id = id.clone();
    let returned_id = run_id.clone();

    // Execute on a blocking thread: `execute_workflow` is synchronous and
    // does file I/O + subprocess work, which would otherwise block the
    // tokio executor. The task is detached — the trace is persisted to
    // SQLite, which is how the UI observes progress.
    tokio::task::spawn_blocking(move || {
        let _ = super::run::execute_workflow(
            &workflow_id,
            initial_params,
            dry_run_mode,
            false,
            Some(run_id),
        );
    });

    Ok(Json(serde_json::json!({ "run_id": returned_id })))
}

#[derive(Debug, Deserialize)]
struct ListRunsQuery {
    workflow_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    50
}

async fn list_runs(Query(q): Query<ListRunsQuery>) -> ApiResult<JsonValue> {
    if q.limit == 0 || q.limit > 500 {
        return Err(ApiError::bad_request("limit must be between 1 and 500"));
    }
    let reg = registry::open().map_err(ApiError::internal)?;
    let rows = reg
        .list_runs(q.workflow_id.as_deref(), q.limit)
        .map_err(ApiError::internal)?;
    Ok(Json(
        serde_json::to_value(rows).map_err(ApiError::internal)?,
    ))
}

async fn get_run(Path(run_id): Path<String>) -> ApiResult<JsonValue> {
    let reg = registry::open().map_err(ApiError::internal)?;
    let detail = reg
        .get_run(&run_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no run `{run_id}`")))?;
    let trace: JsonValue = serde_json::from_str(&detail.trace_json).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({
        "run_id": detail.run_id,
        "workflow_id": detail.workflow_id,
        "status": detail.status,
        "started_at": detail.started_at,
        "ended_at": detail.ended_at,
        "trace": trace,
    })))
}

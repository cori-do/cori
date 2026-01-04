mod assets;
mod auth;
mod config;
mod middleware;
mod state;

use axum::{Json, Router, routing::get};
use serde_json::json;
use std::sync::Arc;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use crate::{config::AppConfig, state::AppState};
use axum::extract::Extension;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let cfg: AppConfig = config::load_config().unwrap_or_else(|err| {
        tracing::warn!("failed to load config.toml, using defaults: {err:#}");
        AppConfig::default()
    });

    let state = Arc::new(AppState::init(&cfg).await?);

    // Base routes
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/whoami", get(middleware::handlers::whoami));

    // Auth routes
    if cfg.auth.mode == config::AuthMode::Embedded {
        app = app.merge(auth::pocket_idp::router());
    }

    // Example protected endpoint (acts like "MCP tool" for now).
    app = app.route(
        "/mcp/tools/update_record",
        get(middleware::handlers::example_tool_update_record)
            .route_layer(axum::middleware::from_fn(middleware::auth::enforce_double_lock)),
    );

    // Apply shared layers after merging all routes
    app = app
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(Extension(state.clone()));

    let addr = cfg.server.bind.clone();
    tracing::info!("cori-server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "cori-server" }))
}

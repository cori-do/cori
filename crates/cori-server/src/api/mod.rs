// SPDX-License-Identifier: Apache-2.0
//! HTTP API: the router and its error type here; the handlers by area in the submodules (access: who may call what,
//! apps, versions, runs, approvals, front, store_routes, connectors, llm, admin), re-exported as `api::…`. The routes of an
//! binary's modules (`crate::module`) are merged in before the guard, so they sit behind it like the core's.
use crate::{auth::Principal, bundles, db, state::AppState};
use axum::handler::Handler;
use axum::{Json, Router, body::Bytes, extract::{DefaultBodyLimit, Path, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::{get, post}};
use cori_engine::config::AppConfig;
use cori_engine::workflow::{BundleRef, CoriWorkflow, DecisionArgs, WorkflowInput, WorkflowOutput};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::Arc;
use temporalio_client::{WorkflowGetResultOptions, WorkflowSignalOptions, WorkflowStartOptions, WorkflowTerminateOptions};

mod access;
mod apps;
mod versions;
mod runs;
mod steps;
mod approvals;
mod front;
pub(crate) use front::{CONSOLE_HTML, PORTAL_HTML};
mod store_routes;
mod connectors;
mod llm;
mod admin;
mod start;
mod identity;
mod people;
mod sessions;
mod passes;
mod portal;
mod files;
pub(crate) use access::*;
pub(crate) use apps::*;
pub(crate) use versions::*;
pub(crate) use runs::*;
pub(crate) use steps::*;
pub(crate) use approvals::*;
pub(crate) use front::*;
pub(crate) use store_routes::*;
pub(crate) use connectors::*;
pub(crate) use llm::*;
pub(crate) use admin::*;
pub(crate) use start::*;
pub(crate) use identity::*;
pub(crate) use people::*;
pub(crate) use sessions::*;
pub(crate) use passes::*;
pub(crate) use portal::*;
pub(crate) use files::*;
// what a module's handlers use (`crate::module` re-exports them)
pub use access::{Caller, Power, require_builder, require_human, require_power};
pub use apps::AppRecord;

/// The router's state: the deployment's own `AppState`, outside any realm. Handlers never take it: they take the state of
/// their request's realm (`InRealm`), which `realm_layer` resolves before them (the core's single realm: this one).
#[derive(Clone)]
pub struct Global(Arc<AppState>);

/// The state of the request's realm, as a handler argument (`InRealm(st): InRealm`).
pub struct InRealm(pub Arc<AppState>);

/// What `realm_layer` leaves for the handlers: the state of the request's realm.
#[derive(Clone)]
struct RequestRealm(Arc<AppState>);

/// The state a request acts in: its realm's (`realm_layer`), else the router's.
fn request_state(extensions: &axum::http::Extensions, g: &Global) -> Arc<AppState> {
    extensions.get::<RequestRealm>().map(|r| r.0.clone()).unwrap_or_else(|| g.0.clone())
}

impl axum::extract::FromRequestParts<Global> for InRealm {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, g: &Global) -> Result<Self, Self::Rejection> { Ok(InRealm(request_state(&parts.extensions, g))) }
}

/// A refusal or a failure, as its status and its message. A refusal's message is the caller's to read; an unexpected
/// failure's (500: a database error, a path, a peer's answer) is only logged, and the caller gets an id to quote.
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    /// The status and the message a caller is given (see the type): a 500 is logged here, with the id it answers.
    pub fn public(self) -> (StatusCode, String) {
        if self.0 != StatusCode::INTERNAL_SERVER_ERROR { return (self.0, self.1); }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        tracing::error!(error_id = %id, "{}", self.1);
        (self.0, format!("internal error {id} (the server's log has the details)"))
    }
}

impl IntoResponse for ApiError { fn into_response(self) -> Response { let (status, msg) = self.public(); (status, Json(json!({ "error": msg }))).into_response() } }

impl From<anyhow::Error> for ApiError { fn from(e: anyhow::Error) -> Self { ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")) } }

impl From<sqlx::Error> for ApiError { fn from(e: sqlx::Error) -> Self { ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("database: {e}")) } }

type Res = Result<Response, ApiError>;

/// Every route of the server. The app API is a contract with the apps already built, whose fronts each carry a copy of
/// `@cori-do/client` (`front/vendor/cori-client/`): `/v1/auth/config`, `/v1/me`, `/v1/apps/{app}`, `/v1/apps/{app}/me`,
/// the run endpoints (`wf/{wf}/runs`, also under `versions/{hash}/`), `runs`, `runs/{run}` with `events`, `stream` and
/// `decisions/{step}`, `approvals` and `approvals/{id}`, `files` (also under `versions/{hash}/`) and `files/{id}` with
/// `content`, and the front's `__cori/config.json`, `__cori/dev-widget.js` and `callback`. Those change by addition only;
/// the other routes serve the cli and the console, which ship with the server.
/// The largest bundle a deploy uploads.
const BUNDLE_MAX_BYTES: usize = 256 * 1024 * 1024;
/// The largest body of every other route (JSON: run parameters, events, store and MCP calls, which Temporal caps at a
/// few megabytes anyway).
const JSON_MAX_BYTES: usize = 16 * 1024 * 1024;

pub fn router(st: Arc<AppState>) -> Router {
    let g = Global(st);
    let core = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/apps", get(list_apps).post(create_app))
        .route("/v1/audit", get(list_audit))
        .route("/v1/apps/{app}", get(get_app).delete(delete_app))
        .route("/v1/apps/{app}/oidc-client", post(post_app_client))
        .route("/v1/apps/{app}/identity", get(app_identity))
        .route("/v1/apps/{app}/versions", post(deploy.layer(DefaultBodyLimit::max(BUNDLE_MAX_BYTES))).get(list_versions))
        .route("/v1/apps/{app}/versions/{hash}", get(get_version).delete(delete_version))
        .route("/v1/apps/{app}/versions/{hash}/bundle", get(get_bundle))
        .route("/v1/apps/{app}/versions/{hash}/promote", post(promote))
        .route("/v1/apps/{app}/versions/{hash}/reseed", post(reseed))
        .route("/v1/apps/{app}/versions/{hash}/wf/{wf}/runs", post(start_preview_run))
        .route("/v1/apps/{app}/versions/{hash}/files", post(create_release_file_upload))
        .route("/v1/apps/{app}/versions/{hash}/stores/{name}/mcp", post(version_store_mcp))
        .route("/v1/apps/{app}/versions/{hash}/preview-codes", post(preview_code))
        .route("/v1/apps/{app}/preview-roles", get(get_preview_roles).put(put_preview_roles).delete(delete_preview_roles))
        .route("/__cori/oidc/.well-known/openid-configuration", get(session_discovery))
        .route("/__cori/oidc/authorize", get(session_authorize))
        .route("/__cori/oidc/token", post(session_token))
        .route("/__cori/oidc/logout", get(session_logout))
        .route("/v1/portal-codes", post(portal_code))
        .route("/v1/apps/{app}/pass-codes", post(pass_code))
        .route("/__cori/pass", get(pass_return))
        .route("/__cori/portal", get(portal_page))
        .route("/__cori/icons/{app}", get(portal_icon))
        .route("/v1/apps/{app}/stores", get(list_app_stores))
        .route("/v1/apps/{app}/stores/reset", post(reset_live_stores))
        .route("/v1/apps/{app}/stores/{name}/mcp", post(app_store_mcp))
        .route("/v1/apps/{app}/protect", post(protect))
        .route("/v1/apps/{app}/rollback", post(rollback))
        .route("/v1/apps/{app}/promotions", get(list_promotions))
        .route("/v1/apps/{app}/schedules", get(list_schedules))
        .route("/v1/bundles/{hash}/files", get(bundle_files))
        .route("/v1/apps/{app}/client.ts", get(client_ts))
        .route("/apps/{app}", get(front_index))
        .route("/apps/{app}/", get(front_index))
        .route("/apps/{app}/{*path}", get(front_file))
        .route("/apps/{app}/@{hash}", get(preview_index))
        .route("/apps/{app}/@{hash}/", get(preview_index))
        .route("/apps/{app}/@{hash}/{*path}", get(preview_file))
        .route("/v1/apps/{app}/wf/{wf}/runs", post(start_run))
        .route("/v1/apps/{app}/runs", get(list_runs))
        .route("/v1/apps/{app}/runs/{run}", get(get_run))
        .route("/v1/apps/{app}/runs/{run}/decisions/{step}", post(decide))
        .route("/v1/apps/{app}/runs/{run}/events", get(run_events))
        .route("/v1/apps/{app}/runs/{run}/steps", get(run_steps))
        .route("/v1/apps/{app}/runs/{run}/stream", get(run_stream))
        .route("/v1/apps/{app}/approvals", get(list_approvals))
        .route("/v1/apps/{app}/approvals/{id}", post(decide_approval))
        .route("/v1/apps/{app}/files", post(create_file_upload))
        .route("/v1/apps/{app}/files/{id}", get(get_app_file))
        // the upload reads its own body, never more than announced (`files::receive`): no route-wide cap applies
        .route("/v1/apps/{app}/files/{id}/content", get(get_app_file_content).put(put_file_content.layer(DefaultBodyLimit::disable())))
        .route("/v1/runs/{run}/files/{id}", get(worker_run_file))
        .route("/v1/ingest/events", post(ingest))
        .route("/v1/apps/{app}/meters", get(meters))
        .route("/v1/apps/{app}/limits", axum::routing::put(put_app_limits))
        .route("/v1/usage", get(usage))
        .route("/v1/apps/{app}/webhooks/deliveries", get(webhook_deliveries))
        .route("/v1/apps/{app}/llm/calls", get(llm_calls))
        .route("/v1/apps/{app}/llm/bindings", get(list_bindings).post(set_binding))
        .route("/v1/apps/{app}/llm/budget", axum::routing::put(crate::llm::put_budget))
        .route("/v1/llm", get(crate::llm::overview))
        .route("/v1/llm/tiers", axum::routing::put(crate::llm::put_tiers))
        .route("/v1/llm/providers/{name}", axum::routing::put(crate::llm::put_provider).delete(crate::llm::delete_provider))
        .route("/v1/llm/providers/{name}/models", get(crate::llm::provider_models))
        .route("/v1/pools", get(list_pools).post(create_pool))
        .route("/v1/secrets", get(list_secrets).post(put_secret))
        .route("/v1/identity/catalog", get(identity_catalog))
        .route("/v1/identity/events", get(identity_events))
        .route("/v1/identity/events/{id}/retry", post(retry_identity_event))
        .route("/v1/stores", get(list_stores))
        .route("/v1/store-logins/rotate", post(rotate_store_logins))
        .route("/v1/connectors", get(list_connectors).post(declare_connector))
        .route("/v1/connectors/{name}/refresh", post(refresh_connector))
        .route("/v1/connectors/{name}/key", get(connector_key))
        .route("/v1/connector-requests", get(list_connector_requests).post(request_connector))
        .route("/v1/connectors/{name}", axum::routing::delete(delete_connector))
        .route("/v1/connections", get(crate::connections::list_mine))
        .route("/v1/connections/callback", get(crate::connections::callback))
        .route("/v1/connections/{provider}", post(crate::connections::start_mine).delete(crate::connections::delete_mine))
        .route("/v1/apps/{app}/connections", get(crate::connections::list_app))
        .route("/v1/apps/{app}/connections/{provider}", post(crate::connections::start_app).delete(crate::connections::delete_app))
        .route("/v1/export", get(crate::lifecycle::export_route))
        .route("/v1/me", get(me))
        .route("/v1/me/apps", get(my_apps))
        .route("/v1/people", get(list_people))
        .route("/v1/people/{id}", axum::routing::delete(remove_person))
        .route("/v1/people/{id}/roles", axum::routing::put(set_person_roles))
        .route("/v1/invitations", get(list_invitations).post(invite))
        .route("/v1/invitations/{id}", axum::routing::delete(cancel_invitation))
        .route("/v1/invitations/{id}/resend", post(resend_invitation))
        .route("/v1/apps/{app}/access", get(app_access).post(change_app_access))
        .route("/v1/people/{id}/leaving", get(leaving))
        .route("/v1/access-requests", get(list_access_requests))
        .route("/v1/access-requests/{id}", axum::routing::delete(cancel_access_request))
        .route("/v1/access-requests/{id}/approve", post(approve_access_request))
        .route("/v1/access-requests/{id}/refuse", post(refuse_access_request))
        .route("/v1/organization", get(get_organization).patch(patch_organization).delete(delete_organization))
        .route("/v1/organization/leave", post(leave_organization))
        .route("/v1/identity/notify", post(identity_notify))
        .route("/v1/me/organizations", get(my_organizations))
        .route("/v1/apps/{app}/builders", axum::routing::put(put_builders))
        .route("/v1/apps/{app}/me", get(app_me))
        .route("/v1/auth/config", get(auth_config))
        .route("/v1/start", get(get_started))
        .route("/.well-known/oauth-protected-resource", get(oauth_protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(oauth_protected_resource))
        .route("/mcp", post(crate::mcp::handle_post).get(crate::mcp::handle_get).delete(crate::mcp::handle_delete))
        .route("/v1/agent-sessions", get(list_agent_sessions))
        .route("/v1/agent-sessions/{jti}", axum::routing::delete(revoke_agent_session))
        .route("/v1/doctor", get(doctor))
        .route("/v1/integrations/slack/actions", post(slack_actions))
        .route("/v1/stores/{name}/mcp", post(store_mcp))
        .route("/v1/leases", post(lease))
        .route("/v1/pools/{name}/heartbeat", post(pool_heartbeat))
        .route("/", get(console_page));
    // the binary's modules add their routes (a route the core has makes `merge` panic at start); the realm is resolved
    // first (the last route layer runs first), the guard then reads the bearer in it
    let routes = g.0.cfg.modules.iter().fold(core, |r, m| r.merge(m.routes()))
        .route_layer(axum::middleware::from_fn_with_state(g.clone(), bearer_guard))
        .route_layer(axum::middleware::from_fn_with_state(g.clone(), realm_layer))
        .layer(DefaultBodyLimit::max(JSON_MAX_BYTES))
        .layer(cors_layer(&g.0.cfg))
        .layer(axum::middleware::from_fn_with_state(g.clone(), cli_min))
        .with_state(g.clone());
    // a front's host is resolved, and its path placed, before the routes are matched (`front_hosts`): a layer of the
    // router itself would run after them
    let fronts = tower::Layer::layer(&axum::middleware::from_fn_with_state(g, front_hosts), routes);
    Router::new().fallback_service(fronts)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::map_response(nosniff))
}

/// Every response declares its content type and means it: no browser sniffs a JSON answer or a front's file into HTML.
async fn nosniff(mut r: Response) -> Response {
    r.headers_mut().insert(axum::http::header::X_CONTENT_TYPE_OPTIONS, axum::http::HeaderValue::from_static("nosniff"));
    r
}

/// The API's answers name the oldest command line the server serves (`ServerConfig::min_cli`), which an older one reads
/// to say it should be upgraded.
async fn cli_min(State(g): State<Global>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let api = req.uri().path().starts_with("/v1/") || req.uri().path() == "/healthz";
    let mut r = next.run(req).await;
    if api { if let Ok(v) = axum::http::HeaderValue::from_str(&g.0.cfg.min_cli) { r.headers_mut().insert("x-cori-cli-min", v); } }
    r
}

/// Browsers may call the API from the server's own origin (console, hosted fronts), the origins in `CORI_CORS_ORIGINS`
/// (comma-separated, e.g. a front served from another domain), and, under dev auth only, any localhost port (a Vite dev
/// server). Other origins get no CORS headers; tokens travel as bearers, so no credentials are allowed cross-origin.
fn cors_layer(cfg: &crate::ServerConfig) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
    let origin_of = |u: &str| reqwest::Url::parse(u.trim()).ok().map(|u| u.origin().ascii_serialization());
    let mut allowed: Vec<String> = origin_of(&cfg.public_url).into_iter().collect();
    allowed.extend(cfg.cors_origins.iter().filter_map(|o| origin_of(o)));
    let dev = matches!(cfg.auth, crate::auth::AuthMode::Dev);
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            let Ok(o) = origin.to_str() else { return false };
            if allowed.iter().any(|a| a == o) { return true; }
            dev && reqwest::Url::parse(o).ok().and_then(|u| u.host_str().map(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]")).unwrap_or(false)
        }))
        .allow_methods(AllowMethods::mirror_request())
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers([axum::http::HeaderName::from_static("retry-after"), axum::http::HeaderName::from_static("x-cori-idempotent-replay"), axum::http::HeaderName::from_static("mcp-session-id")])
        .max_age(std::time::Duration::from_secs(600))
}

fn sha(s: &str) -> String { use sha2::Digest; hex::encode(sha2::Sha256::digest(s.as_bytes())) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unexpected_failure_is_logged_not_told() {
        let (status, msg) = ApiError::from(anyhow::anyhow!("relation \"secrets\" does not exist")).public();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(msg.starts_with("internal error ") && !msg.contains("secrets"), "{msg}");
        assert_eq!(ApiError(StatusCode::FORBIDDEN, "not yours".into()).public(), (StatusCode::FORBIDDEN, "not yours".to_string()));
    }
}

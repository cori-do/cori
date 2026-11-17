// SPDX-License-Identifier: Apache-2.0
//! Who may call what: the caller's principal, the scope and power checks, the bearer guard (agent and front tokens
//! reach their own routes only), and app membership.
use super::*;

/// The authenticated caller of a request, as a handler argument (`Caller(who): Caller`): `principal` runs before the
/// handler and before its body is read, so an unauthenticated request is refused (401) whatever it carries.
pub struct Caller(pub Principal);

impl axum::extract::FromRequestParts<Global> for Caller {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, g: &Global) -> Result<Self, ApiError> {
        principal(&request_state(&parts.extensions, g), &parts.headers).await.map(Caller)
    }
}

pub(crate) async fn principal(st: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> { principal_for(st, headers, false).await }

/// The caller of the MCP endpoint, whose own resource (`<public url>/mcp`) is an audience it accepts too.
pub(crate) async fn mcp_principal(st: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> { principal_for(st, headers, true).await }

async fn principal_for(st: &AppState, headers: &HeaderMap, mcp: bool) -> Result<Principal, ApiError> {
    let who = st.auth.authenticate_for(headers, mcp).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    // the command line names itself (`cori/<version>`): its organization's first steps see that it was used
    if let Some(ua) = headers.get("user-agent").and_then(|v| v.to_str().ok()).filter(|ua| ua.starts_with("cori/")) { client_seen(st, "cli", ua, &who.sub).await; }
    // the people of the organization, as their tokens name them: what the console shows for the ids it keeps
    person_seen(st, &who).await;
    // someone who left the organization: their tokens from before are refused at once (an hour's token would still pass)
    if !who.is_agent() && let Some(iat) = who.claims.get("iat").and_then(|v| v.as_i64()) && let Some(at) = removed_at(st, &who.sub).await? && iat <= at {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "you left this organization (or were removed from it): sign in again".into()));
    }
    if who.is_agent() {
        // an agent session can be revoked before its token expires (`DELETE /v1/agent-sessions/{jti}`, its app deleted)
        let jti = who.session_id.clone().unwrap_or_default();
        let row = sqlx::query("SELECT revoked_at FROM agent_sessions WHERE jti = $1").bind(&jti).fetch_optional(&st.db).await?;
        match row { Some(r) if r.get::<Option<String>, _>("revoked_at").is_none() => {} Some(_) => return Err(ApiError(StatusCode::UNAUTHORIZED, "agent session revoked: call the `login` tool again".into())), None => return Err(ApiError(StatusCode::UNAUTHORIZED, "unknown agent session: call the `login` tool again".into())) }
    }
    Ok(who)
}

/// Approve, deploy, rollback and every admin operation are human actions.
pub fn require_human(who: &Principal, what: &str) -> Result<(), ApiError> {
    if who.is_agent() { return Err(ApiError(StatusCode::FORBIDDEN, format!("{what} needs a human session (the agent token is limited to {}); run it with `cori login` or in the console", who.scopes.join(", ")))); }
    Ok(())
}

pub(crate) fn require_scope(who: &Principal, scope: &str, app: Option<&str>) -> Result<(), ApiError> {
    if !who.has_scope(scope, app) { return Err(ApiError(StatusCode::FORBIDDEN, format!("this agent session lacks `{scope}`{} (scopes: {})", app.map(|a| format!(" on app `{a}`")).unwrap_or_default(), who.scopes.join(", ")))); }
    Ok(())
}

/// A platform power a route requires, granted by the server's role lists (`CORI_*_ROLES`, `CORI_APPROVER_ROLES`); an agent
/// session holds none.
#[derive(Clone, Copy, Debug)]
pub enum Power { Admin, Operator, Auditor, Billing, Builder, Approver }

/// `who` holds one of `powers`, or a 403 naming `what` and the roles that would grant it: every platform check reads so.
pub fn require_power(st: &AppState, who: &Principal, powers: &[Power], what: &str) -> Result<(), ApiError> {
    match missing_power(who, powers, &st.settings.approver_roles) {
        None => Ok(()),
        Some(roles) => Err(ApiError(StatusCode::FORBIDDEN, format!("{what} needs one of the roles {roles}"))),
    }
}

/// `None` when `who` holds one of `powers`, else the roles that would grant one.
pub(crate) fn missing_power(who: &Principal, powers: &[Power], approver_roles: &[String]) -> Option<String> {
    let r = crate::auth::platform_roles();
    let mut names: Vec<String> = Vec::new();
    for p in powers {
        let (held, granted_by): (bool, Vec<String>) = match p {
            Power::Admin => (who.is_admin(), r.admin.clone()),
            Power::Operator => (who.is_operator(), [r.admin.clone(), r.operator.clone()].concat()),
            Power::Auditor => (who.is_auditor(), [r.admin.clone(), r.auditor.clone()].concat()),
            Power::Billing => (who.is_billing(), [r.admin.clone(), r.auditor.clone(), r.billing.clone()].concat()),
            Power::Builder => (who.is_builder(), r.builder.clone()),
            Power::Approver => (!who.is_agent() && who.holds_role(approver_roles), approver_roles.to_vec()),
        };
        if held { return None; }
        names.extend(granted_by);
    }
    names.sort(); names.dedup();
    Some(names.join(", "))
}

/// The app's owner or an admin: its protection, sign-in client, builders and deletion.
pub(crate) fn require_owner_or_admin(who: &Principal, rec: &AppRecord, what: &str) -> Result<(), ApiError> {
    if who.is_admin() || rec.owner.as_deref() == Some(who.sub.as_str()) { return Ok(()); }
    Err(ApiError(StatusCode::FORBIDDEN, format!("{what} is for `{}`'s owner ({}) or an admin", rec.id, rec.owner.as_deref().unwrap_or("nobody"))))
}

/// Everything an agent session may call, with the scope each route needs: read its app, deploy versions of it,
/// preview them and read their sandboxes, discover connectors. Every other route (promote, approve, live data, live runs,
/// administration) needs a human session, and a path's `{app}` must be the session's own. The remote MCP checks each tool.
pub(crate) const AGENT_ROUTES: &[(&str, &str, Option<&str>)] = &[
    ("GET", "/v1/auth/config", None),
    ("GET", "/v1/me", None),
    ("GET", "/v1/me/apps", None),
    ("GET", "/v1/apps", None),
    ("GET", "/v1/apps/{app}", None),
    ("GET", "/v1/apps/{app}/me", None),
    ("GET", "/v1/connectors", Some("registry:read")),
    ("GET", "/v1/apps/{app}/client.ts", Some("versions:publish")),
    ("POST", "/v1/apps/{app}/versions", Some("versions:publish")),
    ("GET", "/v1/apps/{app}/versions", Some("versions:publish")),
    ("GET", "/v1/apps/{app}/versions/{hash}", Some("versions:publish")),
    ("GET", "/v1/apps/{app}/versions/{hash}/bundle", Some("versions:publish")),
    ("GET", "/v1/apps/{app}/promotions", Some("versions:publish")),
    ("POST", "/v1/apps/{app}/versions/{hash}/reseed", Some("runs:preview")),
    ("POST", "/v1/apps/{app}/versions/{hash}/wf/{wf}/runs", Some("runs:preview")),
    // a release's files (the handlers refuse the live ones to an agent session)
    ("POST", "/v1/apps/{app}/versions/{hash}/files", Some("runs:preview")),
    ("PUT", "/v1/apps/{app}/files/{id}/content", Some("runs:preview")),
    ("GET", "/v1/apps/{app}/files/{id}", Some("runs:preview")),
    ("GET", "/v1/apps/{app}/files/{id}/content", Some("runs:preview")),
    ("GET", "/v1/apps/{app}/stores", Some("stores:read")),
    ("POST", "/v1/apps/{app}/versions/{hash}/stores/{name}/mcp", Some("stores:read")),
    ("POST", "/mcp", None),
    ("GET", "/mcp", None),
    ("DELETE", "/mcp", None),
];

/// What a token issued to an app's own OIDC client may call (`azp` is the app's client: its front signed the user in):
/// that app's runtime routes. A front runs code its builders and their agents wrote: it must not reach the platform
/// (versions, promotions, connectors, secrets, the remote MCP, other apps) with the powers of whoever opens it.
pub(crate) const FRONT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/v1/auth/config"), ("GET", "/v1/me"), ("GET", "/v1/apps/{app}"), ("GET", "/v1/apps/{app}/me"),
    ("POST", "/v1/apps/{app}/wf/{wf}/runs"), ("POST", "/v1/apps/{app}/versions/{hash}/wf/{wf}/runs"),
    ("GET", "/v1/apps/{app}/runs"), ("GET", "/v1/apps/{app}/runs/{run}"), ("POST", "/v1/apps/{app}/runs/{run}/decisions/{step}"),
    ("GET", "/v1/apps/{app}/runs/{run}/events"), ("GET", "/v1/apps/{app}/runs/{run}/stream"),
    ("GET", "/v1/apps/{app}/approvals"), ("POST", "/v1/apps/{app}/approvals/{id}"),
    ("POST", "/v1/apps/{app}/files"), ("POST", "/v1/apps/{app}/versions/{hash}/files"), ("PUT", "/v1/apps/{app}/files/{id}/content"),
    ("GET", "/v1/apps/{app}/files/{id}"), ("GET", "/v1/apps/{app}/files/{id}/content"),
];

/// What a guest may call (someone outside the organization an app was shared with, `CORI_GUEST_ROLES`): their own user,
/// organizations (theirs, and whether they may start one of their own), connections (`connections`) and apps, and the runtime, data and approvals routes
/// of an app they hold a role in. Every other route (the people, the platform, connectors, the remote MCP, previews, other
/// apps) is refused, as for agent sessions and fronts' tokens.
pub(crate) const GUEST_ROUTES: &[(&str, &str)] = &[
    ("GET", "/v1/auth/config"), ("GET", "/v1/me"), ("GET", "/v1/me/apps"), ("GET", "/v1/me/organizations"), ("GET", "/v1/apps/{app}"), ("GET", "/v1/apps/{app}/me"),
    ("POST", "/v1/apps/{app}/wf/{wf}/runs"), ("GET", "/v1/apps/{app}/runs"), ("GET", "/v1/apps/{app}/runs/{run}"), ("POST", "/v1/apps/{app}/runs/{run}/decisions/{step}"),
    ("GET", "/v1/apps/{app}/runs/{run}/events"), ("GET", "/v1/apps/{app}/runs/{run}/stream"), ("GET", "/v1/apps/{app}/runs/{run}/steps"), ("GET", "/v1/apps/{app}/approvals"), ("POST", "/v1/apps/{app}/approvals/{id}"),
    ("GET", "/v1/apps/{app}/stores"), ("POST", "/v1/apps/{app}/stores/{name}/mcp"), ("POST", "/v1/portal-codes"), ("POST", "/v1/apps/{app}/pass-codes"),
    ("GET", "/v1/connections"), ("POST", "/v1/connections/{provider}"), ("DELETE", "/v1/connections/{provider}"),
    ("POST", "/v1/apps/{app}/files"), ("PUT", "/v1/apps/{app}/files/{id}/content"), ("GET", "/v1/apps/{app}/files/{id}"), ("GET", "/v1/apps/{app}/files/{id}/content"),
];

/// What a person of the organization who holds no platform power (`holds_power`) and builds no app may call besides
/// `GUEST_ROUTES`: they reach their apps (their runs, what waits for them there, the data their app roles reach), leaving
/// this organization, and a release's runs and data, which their handlers keep to those who may open a release (anyone
/// under dev auth, where a release is tried as the app's own personas). Creating an app reaches its handler, which refuses
/// them before anything else and names where to start an organization of their own (`create_app_record`). The
/// organization's people, settings and connectors, its first steps and the remote MCP are for those who build its apps or
/// hold a power.
pub(crate) const MEMBER_APP_ROUTES: &[(&str, &str)] = &[
    ("POST", "/v1/organization/leave"), ("POST", "/v1/apps"),
    ("POST", "/v1/apps/{app}/versions/{hash}/wf/{wf}/runs"), ("POST", "/v1/apps/{app}/versions/{hash}/stores/{name}/mcp"), ("POST", "/v1/apps/{app}/versions/{hash}/files"),
];

/// The routes workers call (`crate::workers`). There, under a tenancy, a worker names the realm of the run it works on
/// (`x-cori-realm`).
pub(crate) const WORKER_ROUTES: &[&str] = &["/v1/ingest/events", "/v1/leases", "/v1/stores/{name}/mcp", "/v1/bundles/{hash}/files", "/v1/pools/{name}/heartbeat", "/v1/runs/{run}/files/{id}"];

/// A request to an app's own front host (`CORI_FRONTS_URL`): the app it serves, its realm under a tenancy, and the release
/// a release's host serves (its number: `<app>--v<n><suffix>`), None for the live front.
#[derive(Clone, Debug)]
pub(crate) struct FrontHost { pub app: String, pub realm: Option<crate::realm::Realm>, pub release: Option<i64> }

/// A request to the host of an organization's apps portal: the realm it is (under a tenancy), and the host's origin, which
/// its sessions are bound to (`api::sessions`).
#[derive(Clone, Debug)]
pub(crate) struct PortalHost { pub realm: Option<crate::realm::Realm>, pub origin: String }

/// What a portal's host serves: its page (`/`), its apps' icons, its sign-in (`SESSION_ROUTES`), and of the API the person
/// and their apps.
pub(crate) const PORTAL_ROUTES: &[(&str, &str)] = &[
    ("GET", "/__cori/portal"), ("GET", "/__cori/icons/{app}"), ("GET", "/v1/auth/config"), ("GET", "/v1/me"), ("GET", "/v1/me/apps"),
];

/// Where `method path` goes on a portal's host: its page for every other read (`/` first), the API and the routes of its
/// own as they are, nothing that writes but its sign-in's token.
pub(crate) fn portal_path(method: &axum::http::Method, path: &str) -> FrontPath {
    let read = method == axum::http::Method::GET || method == axum::http::Method::HEAD;
    if path.starts_with("/v1/") || path.starts_with("/__cori/oidc/") || path.starts_with("/__cori/icons/") { return FrontPath::Keep; }
    if read { FrontPath::Front("/__cori/portal".into()) } else { FrontPath::NotServed }
}

/// What a request reaches of an app's runs and approvals: on a release's host its own runs only (they work on its sandbox),
/// on the live front's host the live ones; elsewhere (the console, the cli) all of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunScope { All, Live, Release(String) }

impl RunScope {
    /// What a request on `front` reaches of `app`'s runs (a release's host: that release, by its hash).
    pub(crate) async fn of(st: &AppState, app: &str, front: Option<&FrontHost>) -> Result<Self, ApiError> {
        Ok(match front {
            None => RunScope::All,
            Some(FrontHost { release: None, .. }) => RunScope::Live,
            Some(FrontHost { release: Some(seq), .. }) => RunScope::Release(resolve_version_hash(st, app, &format!("v{seq}")).await?),
        })
    }
    /// Whether a run working on `store_target` is in reach (none recorded: a run of before, or scheduled: live).
    pub(crate) fn sees(&self, store_target: Option<&str>) -> bool {
        match self { RunScope::All => true, RunScope::Live => store_target.unwrap_or(cori_store::LIVE) == cori_store::LIVE, RunScope::Release(h) => store_target == Some(h.as_str()) }
    }
    /// The value a query compares `COALESCE(store_target, 'live')` with, `*` for every run.
    pub(crate) fn target(&self) -> &str { match self { RunScope::All => "*", RunScope::Live => cori_store::LIVE, RunScope::Release(h) => h } }
}

/// Where a request to a front's host goes (`front_hosts`), from its method and path alone.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FrontPath {
    /// as it is: the app's runtime API (`/v1/…`), the sign-in callback, a preview, another app's path (refused later)
    Keep,
    /// an older link under `/apps/<app>/`: the same path at the root (308)
    Moved(String),
    /// the front itself, at the root of its host: served by the `/apps/<app>/…` routes, under this path
    Front(String),
    /// a write outside the API: nothing there takes one
    NotServed,
}

/// Where `method path` goes on the host of `app`'s front. The front is served at the root: `/x` is its `x`
/// (`/__cori/config.json` included). The callback keeps the path its identity provider registered (`names::callback_url`),
/// and previews keep theirs.
pub(crate) fn front_path(app: &str, release: Option<i64>, method: &axum::http::Method, path: &str) -> FrontPath {
    let read = method == axum::http::Method::GET || method == axum::http::Method::HEAD;
    // the API, the host's own sign-in where it has one (`api::sessions`), and its passes (`api::passes`)
    if path == "/v1" || path.starts_with("/v1/") || path == "/__cori/oidc" || path.starts_with("/__cori/oidc/") || path == PASS_ROUTE { return FrontPath::Keep; }
    let prefix = format!("/apps/{app}");
    if path == format!("{prefix}/callback") { return FrontPath::Keep; }
    // a preview link: the live host sends it on to the release's own host (`preview_index`); a release's host serves its
    // own release only, at its root
    if path.starts_with(&format!("{prefix}/@")) { return if release.is_none() && read { FrontPath::Keep } else { FrontPath::NotServed }; }
    if path == prefix || path.starts_with(&format!("{prefix}/")) {
        // one leading slash: `//x` would name another host
        return if read { FrontPath::Moved(format!("/{}", path[prefix.len()..].trim_start_matches('/'))) } else { FrontPath::NotServed };
    }
    if path.starts_with("/apps/") { return FrontPath::Keep; }
    if !read { return FrontPath::NotServed; }
    match release { Some(seq) => FrontPath::Front(format!("{prefix}/@v{seq}{path}")), None => FrontPath::Front(format!("{prefix}{path}")) }
}

/// Before routing, since it may rewrite the path: a request to an app's front host (`CORI_FRONTS_URL`) is resolved once,
/// then served at the root of that host (`front_path`). `realm_layer` takes the resolution from the request's extensions.
pub(crate) async fn front_hosts(State(g): State<Global>, mut req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let st = g.0.clone();
    let host = req.headers().get(axum::http::header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    // an organization's apps portal: the deployment's own (`CORI_PORTAL_URL`), or a realm's under the fronts' domain
    if let Some(portal) = portal_of_host(&st, &host).await {
        match portal_path(req.method(), req.uri().path()) {
            FrontPath::Keep => {}
            FrontPath::Front(path) => if let Err(r) = place(&mut req, &format!("{path}{query}")) { return r },
            _ => return ApiError(StatusCode::NOT_FOUND, format!("`{} {}` is not served on the apps portal", req.method(), req.uri().path())).into_response(),
        }
        req.extensions_mut().insert(portal);
        return next.run(req).await;
    }
    let Some(label) = st.cfg.fronts_url.as_ref().and_then(|f| f.label_of(&host)) else { return next.run(req).await };
    let Some(front) = front_of_label(&st, &label).await else { return ApiError(StatusCode::NOT_FOUND, format!("no app's front at `{label}`")).into_response() };
    match front_path(&front.app, front.release, req.method(), req.uri().path()) {
        FrontPath::Keep => {}
        FrontPath::Moved(to) => return (StatusCode::PERMANENT_REDIRECT, [(axum::http::header::LOCATION, format!("{to}{query}"))]).into_response(),
        FrontPath::NotServed => return ApiError(StatusCode::NOT_FOUND, format!("`{} {}` is not served on an app's front host", req.method(), req.uri().path())).into_response(),
        FrontPath::Front(path) => if let Err(r) = place(&mut req, &format!("{path}{query}")) { return r },
    }
    req.extensions_mut().insert(front);
    next.run(req).await
}

/// The request's path and query, rewritten to `pq` (before routing: `front_hosts`).
fn place(req: &mut axum::extract::Request, pq: &str) -> Result<(), Response> {
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = pq.parse().ok();
    match axum::http::Uri::from_parts(parts) { Ok(u) => { *req.uri_mut() = u; Ok(()) } Err(_) => Err(ApiError(StatusCode::BAD_REQUEST, "bad path".into()).into_response()) }
}

/// The apps portal a request's host is: `CORI_PORTAL_URL`'s host (the core's single realm), or, under a tenancy, a label of
/// the fronts' domain that names a realm's portal (`Tenancy::portal`: one without `--`, which no front's label is).
async fn portal_of_host(st: &AppState, host: &str) -> Option<PortalHost> {
    let host = host.trim().to_ascii_lowercase();
    if let Some(origin) = &st.cfg.portal_url {
        let want = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
        // a proxy that terminates TLS may drop the default port
        if host == want || host.split(':').next() == Some(want) { return Some(PortalHost { realm: None, origin: origin.clone() }); }
    }
    let (tenancy, fronts) = (st.tenancy.as_ref()?, st.cfg.fronts_url.as_ref()?);
    let label = fronts.label_of(&host).filter(|l| !l.contains("--"))?;
    let realm = tenancy.portal(&st.db.outside(), &label).await?;
    Some(PortalHost { realm: Some(realm), origin: fronts.origin(&label) })
}

/// Where a request lands, and the realm it acts in, before the handlers run.
///
/// With `CORI_FRONTS_URL`, every app's front has a host of its own, and the server's own host none: one front's code never
/// shares an origin with another's, nor with the console's tokens. A front host serves its app's front at its root, its
/// previews and callback, and the runtime routes of `FRONT_ROUTES` on that app (`front_hosts` resolved it and placed its
/// path); the server's own host sends a front's path to the front's host (the core) or refuses it (a tenancy: the host
/// names the organization).
///
/// Under a tenancy, the realm a request acts in (`crate::realm`) is left for the handlers (`InRealm`, `Caller`). A front
/// host names it, and a bearer sent there must be of the same realm. Elsewhere an issuer's token names it through
/// `Tenancy::realm_of`; an agent token carries the realm it was minted in, and that realm's key verifies it (in the guard
/// and the handler); a worker names its run's realm on the workers' routes (`x-cori-realm`, heard with a worker's
/// credentials only, which the handler checks in that realm). Refused here: an issuer's token that names no realm (401),
/// a realm the tenancy does not admit (its answer). Anything else acts outside any realm, where the platform database shows
/// nothing; a bearer that does not verify is the handler's to refuse, with its reason.
pub(crate) async fn realm_layer(State(g): State<Global>, mut req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let st = g.0.clone();
    let route = req.extensions().get::<axum::extract::MatchedPath>().map(|m| m.as_str().to_string()).unwrap_or_default();
    let front = req.extensions().get::<FrontHost>().cloned();
    let portal = req.extensions().get::<PortalHost>().cloned();
    if portal.is_some() {
        let allowed = |list: &[(&str, &str)]| list.iter().any(|(m, p)| *m == req.method().as_str() && *p == route);
        if !(allowed(PORTAL_ROUTES) || allowed(SESSION_ROUTES)) { return ApiError(StatusCode::NOT_FOUND, format!("`{} {route}` is not served on the apps portal", req.method())).into_response(); }
    }
    let is_front_route = route == "/apps/{app}" || route.starts_with("/apps/{app}/");
    let is_preview_route = route.starts_with("/apps/{app}/@{hash}");
    if let Some(f) = &front {
        if let Err(r) = front_host_serves(&f.app, req.method().as_str(), &route, req.uri().path()) { return r; }
        // a release's host serves its own release only (`front_hosts` placed its paths under `@v<n>`)
        if let Some(seq) = f.release {
            let own = req.uri().path().split('/').nth(3) == Some(format!("@v{seq}").as_str());
            if is_preview_route != own || (is_front_route && !is_preview_route && route != "/apps/{app}/{*path}") {
                return ApiError(StatusCode::NOT_FOUND, format!("this host serves release v{seq} of `{}` only", f.app)).into_response();
            }
        }
    } else if is_front_route && st.cfg.fronts_url.is_some() {
        // the server's own host serves no front: the core knows each front's host, a tenancy's depends on the organization
        let app = req.uri().path().split('/').nth(2).unwrap_or_default().to_string();
        if st.tenancy.is_some() { return ApiError(StatusCode::NOT_FOUND, format!("fronts are served on their own hosts: `{app}`'s is in its console page")).into_response(); }
        // a preview link goes straight to its release's host (`preview_index` knows its number)
        if is_preview_route { return next.run(req).await; }
        // the live front at the root of its host (the callback keeps its path)
        let path = match front_path(&app, None, &axum::http::Method::GET, req.uri().path()) { FrontPath::Moved(to) => to, _ => req.uri().path().to_string() };
        let to = format!("{}{path}{}", st.front_origin(&app), req.uri().query().map(|q| format!("?{q}")).unwrap_or_default());
        return (StatusCode::PERMANENT_REDIRECT, [(axum::http::header::LOCATION, to)]).into_response();
    }
    let Some(tenancy) = st.tenancy.clone() else { return next.run(req).await };
    let named = match request_realm(&st.auth, &st.db.outside(), tenancy.as_ref(), req.headers(), &route).await { Ok(r) => r, Err(e) => return e.into_response() };
    let realm = match (front.and_then(|f| f.realm).or(portal.and_then(|p| p.realm)), named) {
        (Some(host), Some(bearer)) if host != bearer => return ApiError(StatusCode::FORBIDDEN, "this token is of another organization than this host's".into()).into_response(),
        (Some(host), _) => Some(host),
        (None, named) => named,
    };
    // a command run from an app folder names the organization the folder belongs to: a token of another one is refused
    // rather than act there (the same app id may exist in both)
    if let (Some(realm), Some(want)) = (&realm, req.headers().get("x-cori-organization").and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty())) {
        if want != realm.as_str() { return ApiError(StatusCode::CONFLICT, format!("this command is for organization `{want}`, and your token is for another organization (`{}`): sign in to the folder's organization", realm.as_str())).into_response(); }
    }
    if let Some(realm) = realm {
        match st.in_realm(realm).await { Ok(s) => { req.extensions_mut().insert(RequestRealm(s)); } Err(e) => return e.into_response() }
    }
    next.run(req).await
}

/// The app (and realm, and release) a front host's label names: the release taken out (`names::split_version`), the
/// tenancy's answer for the rest, else (the core's single realm) the rest itself, an app id.
async fn front_of_label(st: &AppState, label: &str) -> Option<FrontHost> {
    let (label, release) = crate::names::split_version(label);
    match &st.tenancy {
        Some(t) => t.front(&st.db.outside(), &label).await.map(|(realm, app)| FrontHost { app, realm: Some(realm), release }),
        None => crate::names::front_app_id_problem(&label).is_none().then(|| FrontHost { app: label, realm: None, release }),
    }
}

/// Whether an app's front host serves `method route` (the matched pattern) on `path`: the app's front, its previews and
/// callback, its sign-in and passes, the runtime routes of `FRONT_ROUTES`, each on that app only.
fn front_host_serves(app: &str, method: &str, route: &str, path: &str) -> Result<(), Response> {
    let front = route == "/apps/{app}" || route.starts_with("/apps/{app}/");
    let allowed = |list: &[(&str, &str)]| list.iter().any(|(m, p)| *m == method && *p == route);
    if !(front || allowed(FRONT_ROUTES) || allowed(SESSION_ROUTES) || (method == "GET" && route == PASS_ROUTE)) {
        return Err(ApiError(StatusCode::NOT_FOUND, format!("`{method} {route}` is not served on an app's front host (the console's host serves it)")).into_response());
    }
    let target = route.split('/').position(|s| s == "{app}").and_then(|i| path.split('/').nth(i));
    match target { Some(t) if t != app => Err(ApiError(StatusCode::NOT_FOUND, format!("this host serves app `{app}` only")).into_response()), _ => Ok(()) }
}

/// The realm a request names (`realm_layer`), None when it names none.
async fn request_realm(auth: &crate::auth::Authenticator, db: &db::Db, tenancy: &dyn crate::realm::Tenancy, headers: &HeaderMap, route: &str) -> Result<Option<crate::realm::Realm>, ApiError> {
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    if let Some(token) = header("authorization").and_then(|v| v.strip_prefix("Bearer ")).map(str::trim) {
        if crate::auth::is_agent_token(token) { return Ok(crate::auth::agent_token_realm(token)); }
        // a front's session names the realm it was signed in in, whose key verifies it
        if crate::auth::is_session_token(token) { return Ok(crate::auth::session_token_claims(token).and_then(|c| c.realm).and_then(|r| crate::realm::Realm::parse(&r))); }
        let Ok(who) = auth.authenticate_for(headers, route == "/mcp").await else { return Ok(None) };
        return tenancy.realm_of(db, &who).await.map(Some);
    }
    if WORKER_ROUTES.contains(&route) && (header("x-cori-worker").is_some() || header("x-cori-pool").is_some()) {
        return match header("x-cori-realm") {
            Some(r) => crate::realm::Realm::parse(r).map(Some).ok_or(ApiError(StatusCode::BAD_REQUEST, "x-cori-realm names no realm".into())),
            None => Ok(None),
        };
    }
    Ok(None)
}

/// Deny by default for agent sessions (`AGENT_ROUTES`), for tokens of an app's own client (`FRONT_ROUTES`), for guests
/// (`GUEST_ROUTES`) and for the people who hold no platform power and build no app (`MEMBER_APP_ROUTES` besides); the
/// others and workers go straight to the handlers' own checks.
pub(crate) async fn bearer_guard(State(g): State<Global>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let st = request_state(req.extensions(), &g);
    let bearer = req.headers().get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|t| t.trim().to_string());
    let agent = bearer.as_deref().map(crate::auth::is_agent_token).unwrap_or(false);
    let Some(route) = req.extensions().get::<axum::extract::MatchedPath>().map(|m| m.as_str().to_string()) else { return next.run(req).await };
    // fronts, the console and the discovery documents never read the bearer
    if !(route.starts_with("/v1/") || route == "/mcp") { return next.run(req).await; }
    // a front's session (`api::sessions`): on the host it was signed in on, the routes of its kind
    let front = req.extensions().get::<FrontHost>().cloned();
    let portal = req.extensions().get::<PortalHost>().cloned();
    let signs_in_here = session_host_origin(&st, front.as_ref(), portal.as_ref());
    if bearer.as_deref().is_some_and(crate::auth::is_session_token) {
        let Ok(who) = st.auth.authenticate(req.headers()).await else { return next.run(req).await };
        return match session_may_call(signs_in_here.as_deref(), &who, req.method().as_str(), &route, req.uri().path()) { Ok(()) => next.run(req).await, Err(e) => e.into_response() };
    }
    // a host that signs its people in through the console hears its own sessions only: no token of the identity provider,
    // the console's or an agent's, reaches a release's code or the portal there
    if bearer.is_some() && matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) && signs_in_here.is_some() {
        return ApiError(StatusCode::FORBIDDEN, "this host hears its own sign-in only (`/__cori/oidc`): reload the page to sign in there".into()).into_response();
    }
    if agent {
        // a token that does not verify, or a revoked session: the handler answers 401 with the reason
        let Ok(who) = st.auth.authenticate(req.headers()).await else { return next.run(req).await };
        return match agent_may_call(&who, req.method().as_str(), &route, req.uri().path()) { Ok(()) => next.run(req).await, Err(e) => e.into_response() };
    }
    if let Some(azp) = bearer.as_deref().and_then(jwt_azp).filter(|a| !is_platform_client(&st.cfg, a)) {
        let app: Option<String> = match sqlx::query("SELECT id FROM apps WHERE client_id = $1").bind(&azp).fetch_optional(&st.db).await {
            Ok(r) => r.map(|r| r.get("id")),
            Err(e) => return ApiError::from(e).into_response(),
        };
        if let Some(app) = app { if let Err(e) = front_may_call(&app, req.method().as_str(), &route, req.uri().path()) { return e.into_response(); } }
    }
    // the person a bearer names, their roles read unverified: they only narrow what it reaches
    let Some(who) = bearer.as_deref().and_then(|t| st.auth.unverified(t)) else { return next.run(req).await };
    // a guest reaches the apps shared with them, nothing else
    if who.is_guest() {
        let app = route.split('/').position(|s| s == "{app}").and_then(|i| req.uri().path().split('/').nth(i)).map(String::from);
        let client: Option<String> = match &app {
            Some(app) => match sqlx::query_scalar::<_, Option<String>>("SELECT client_id FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await {
                Ok(c) => c.flatten(),
                Err(e) => return ApiError::from(e).into_response(),
            },
            None => None,
        };
        if let Err(e) = guest_may_call(&who, req.method().as_str(), &route, app.as_deref(), client.as_deref()) { return e.into_response(); }
    // a person who holds no platform power reaches their apps and their organizations, unless they build an app
    } else if !member_may_call(req.method().as_str(), &route) && !holds_power(&st, &who) {
        match builds_any_app(&st, &who.sub).await {
            Ok(true) => {}
            Ok(false) => return ApiError(StatusCode::FORBIDDEN, format!("your roles open the organization's apps: `{} {route}` is for those who build them or administer it", req.method())).into_response(),
            Err(e) => return e.into_response(),
        }
    }
    next.run(req).await
}

/// Whether a person who holds no platform power and builds no app may call `method route` (the matched pattern): a route
/// of `GUEST_ROUTES` or `MEMBER_APP_ROUTES`. Their handlers check the app's roles.
pub(crate) fn member_may_call(method: &str, route: &str) -> bool {
    GUEST_ROUTES.iter().chain(MEMBER_APP_ROUTES).any(|(m, p)| *m == method && *p == route)
}

/// Whether `who` holds a platform power: administrator, operator, auditor, billing, builder, store administrator or
/// approver (never through an agent session).
pub(crate) fn holds_power(st: &AppState, who: &Principal) -> bool {
    who.is_operator() || who.is_billing() || who.is_builder() || who.is_store_admin() || is_approver(st, who)
}

/// Whether `sub` owns one of the apps or is among its builders.
pub(crate) async fn builds_any_app(st: &AppState, sub: &str) -> Result<bool, ApiError> {
    // `builders` holds a JSON list of ids (`put_builders`): the rows that name `sub` are read again as JSON
    let quoted = serde_json::to_string(sub).unwrap_or_default();
    let rows = sqlx::query("SELECT owner, builders FROM apps WHERE owner = $1 OR strpos(COALESCE(builders, ''), $2) > 0").bind(sub).bind(&quoted).fetch_all(&st.db).await?;
    Ok(rows.iter().any(|r| r.get::<Option<String>, _>("owner").as_deref() == Some(sub)
        || r.get::<Option<String>, _>("builders").and_then(|b| serde_json::from_str::<Vec<String>>(&b).ok()).is_some_and(|b| b.iter().any(|x| x == sub))))
}

/// Whether the guest `who` may call `method route` (the matched pattern), on `app` (whose sign-in client is `client`) when
/// the route names one: a route of `GUEST_ROUTES`, on an app they hold a role in.
pub(crate) fn guest_may_call(who: &Principal, method: &str, route: &str, app: Option<&str>, client: Option<&str>) -> Result<(), ApiError> {
    if !GUEST_ROUTES.iter().any(|(m, p)| *m == method && *p == route) {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("a guest reaches the apps shared with them: `{method} {route}` is not for guests")));
    }
    match app {
        Some(app) if who.app_roles(app, client).is_empty() => Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` is not shared with you"))),
        _ => Ok(()),
    }
}

/// The platform's own clients (the console, the cli, the platform), which are never an app's, whatever a client hook answered.
pub(crate) fn is_platform_client(cfg: &crate::ServerConfig, azp: &str) -> bool { azp == cfg.auth_client_id || azp == cfg.web_client_id || azp == cfg.cli_client_id }

/// An approval only a person decides on the platform: a release's go-live (`promote:`) or an app's deletion (`delete:`).
pub(crate) fn is_platform_approval(id: &str) -> bool { id.starts_with("promote:") || id.starts_with("delete:") }

/// Whether a request may decide (or list) the platform's approvals: not on a front's host, nor with a token of any client
/// but the platform's own. A front runs code its builders and their agents wrote, with the token of whoever opens it: a
/// release opened by an approver must not approve its own go-live. The console and the cli sign in as platform clients;
/// Slack's buttons and `cori promote` come their own ways. A bearer naming no client (dev auth) is a person's.
pub(crate) fn platform_decision(cfg: &crate::ServerConfig, on_front_host: bool, headers: &HeaderMap) -> bool {
    if on_front_host { return false; }
    let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim);
    bearer.and_then(jwt_azp).map(|azp| is_platform_client(cfg, &azp)).unwrap_or(true)
}

/// The `azp` of a JWT bearer, read without verifying it: it only ever narrows what the bearer reaches, and the handler
/// verifies the token.
pub(crate) fn jwt_azp(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&payload).ok()?.get("azp")?.as_str().map(String::from)
}

/// Whether a token of `app`'s own client may call `method route` (the matched pattern) on `path`.
pub(crate) fn front_may_call(app: &str, method: &str, route: &str, path: &str) -> Result<(), ApiError> {
    if !FRONT_ROUTES.iter().any(|(m, p)| *m == method && *p == route) {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("`{method} {route}` is not for an app's front: a token of `{app}`'s sign-in reaches `{app}`'s runs, approvals and user only (the console and `cori login` sign in for the rest)")));
    }
    let target = route.split('/').position(|s| s == "{app}").and_then(|i| path.split('/').nth(i));
    match target { Some(t) if t != app => Err(ApiError(StatusCode::FORBIDDEN, format!("a token of `{app}`'s sign-in does not reach app `{t}`"))), _ => Ok(()) }
}

/// Whether the agent session `who` may call `method route` (the matched pattern) on `path`: a route of `AGENT_ROUTES`, the
/// session's own app in the path, the route's scope in the token.
pub(crate) fn agent_may_call(who: &Principal, method: &str, route: &str, path: &str) -> Result<(), ApiError> {
    let Some((_, _, scope)) = AGENT_ROUTES.iter().find(|(m, p, _)| *m == method && *p == route) else {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("`{method} {route}` needs a human session: an agent session reads its app, deploys and previews versions and reads their sandboxes (run it with `cori login` or in the console)")));
    };
    // app ids are letters, digits, `-` and `_`: the raw path segment is the id
    let app = route.split('/').position(|s| s == "{app}").and_then(|i| path.split('/').nth(i));
    if let Some(app) = app { if who.app.as_deref() != Some(app) { return Err(ApiError(StatusCode::FORBIDDEN, format!("this agent session is bound to app `{}`: `{app}` needs its own session (the `login` tool with app `{app}`)", who.app.as_deref().unwrap_or("")))); } }
    match scope { Some(scope) => require_scope(who, scope, app), None => Ok(()) }
}

// ---------------------------------------------------------------------------
// Apps exist explicitly and have owners: `POST /v1/apps` reserves an id; publishing needs the owner, a builder or an admin.
// ---------------------------------------------------------------------------

/// Whether `who` may approve a release going live (one of the server's approver roles; never an agent session).
pub(crate) fn is_approver(st: &AppState, who: &Principal) -> bool { !who.is_agent() && !st.settings.approver_roles.is_empty() && who.holds_role(&st.settings.approver_roles) }

/// The caller inside `app`: its roles become the app's roles (its client roles under an issuer), its platform roles
/// stay for admin, builder and approver checks. Every handler that checks an app role (endpoints, approval routing,
/// store roles, the run's `ctx.user`) resolves the caller through this.
pub(crate) async fn app_principal(st: &AppState, headers: &HeaderMap, app: &str) -> Result<Principal, ApiError> {
    let who = principal(st, headers).await?;
    scope_to_app(st, who, app).await
}

pub(crate) async fn scope_to_app(st: &AppState, who: Principal, app: &str) -> Result<Principal, ApiError> {
    let client = app_record(st, app).await?.and_then(|r| r.client_id);
    Ok(in_app_asking(st, who, app, client.as_deref()).await)
}

/// `Principal::in_app`, its app roles from `app_roles_of`.
pub(crate) async fn in_app_asking(st: &AppState, who: Principal, app: &str, client: Option<&str>) -> Principal {
    if who.platform.is_some() { return who; }
    let (roles, _) = app_roles_of(st, &who, app, client).await;
    let mut scoped = who.in_app(app, client);
    scoped.roles = roles;
    scoped
}

/// How long the identity provider's answer about a person's roles in an app is reused.
const ROLES_FRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// The roles `who` holds in `app` (whose sign-in client is `client`), and whether the identity provider gave them: their
/// token's (`Principal::app_roles`), else, when their token is a person's of another client than the app's (the console,
/// the cli, an MCP host) and carries none of the app's roles, the receiver's answer to `access.list` about them. A sign-in
/// client that is not given the app clients' roles (Keycloak's "Full scope allowed" off) then still reaches its apps.
pub(crate) async fn app_roles_of(st: &AppState, who: &Principal, app: &str, client: Option<&str>) -> (Vec<String>, bool) {
    let roles = who.app_roles(app, client);
    if !roles.is_empty() || !lacks_app_roles(who, client) { return (roles, false); }
    match directory_app_roles(st, &who.sub, app, client.unwrap_or_default()).await { Some(r) => (r, true), None => (roles, false) }
}

/// Whether `who`'s token may lack the roles of the app whose client is `client` though they hold some: a person's token
/// from the issuer (never dev or off auth), of another client than the app's own. A release's or a portal's session holds
/// what it was given, an agent session what it was minted with.
fn lacks_app_roles(who: &Principal, client: Option<&str>) -> bool {
    client.is_some() && !who.is_agent() && who.platform.is_none() && who.session_id.is_none() && who.claims.is_object() && who.claims["azp"].as_str() != client
}

/// The roles `person` holds in `app` by the receiver's answer to `access.list` (`{ client_id, app, by, person }`, `by` and
/// `person` both the person), reused for `ROLES_FRESH`. None when it does not answer `access.list`, or failed.
pub(crate) async fn directory_app_roles(st: &AppState, person: &str, app: &str, client: &str) -> Option<Vec<String>> {
    let args = json!({ "client_id": client, "app": app, "by": person, "person": person });
    let listed = crate::client_hook::ask_cached(st, "access.list", args, ROLES_FRESH).await.unwrap_or_else(|e| { tracing::warn!(app, error = %e, "access.list for a person's roles"); None })?;
    Some(roles_in_grants(&listed, person))
}

/// `person`'s roles in an answer to `access.list`: the grants naming them (`person` an object with its `id`, or the id).
fn roles_in_grants(listed: &Value, person: &str) -> Vec<String> {
    let mut roles: Vec<String> = listed["grants"].as_array().into_iter().flatten()
        .filter(|g| g["person"]["id"].as_str().or(g["person"].as_str()) == Some(person))
        .flat_map(|g| g["roles"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect::<Vec<_>>()).collect();
    roles.sort(); roles.dedup();
    roles
}

/// The app's owner, one of its builders, or an admin. An agent session builds the one app it is bound to, with the rights
/// its human holds there (an admin's too), and no other.
pub(crate) fn is_owner_or_builder(who: &Principal, rec: &AppRecord) -> bool {
    if who.is_agent() && who.app.as_deref() != Some(rec.id.as_str()) { return false; }
    let roles = crate::auth::platform_roles();
    who.holds_role(&roles.admin) || rec.owner.as_deref() == Some(who.sub.as_str()) || rec.builders.iter().any(|b| b == &who.sub) || (rec.owner.is_none() && who.holds_role(&roles.builder))
}

pub(crate) fn may_create_apps(who: &Principal) -> bool { who.is_admin() || who.is_builder() }

/// The app, when `who` builds it (owner, builders, admins) or `also` lets them in (an approver, an operator…); a 403 otherwise.
pub async fn require_builder(st: &AppState, who: &Principal, app: &str, also: bool, what: &str) -> Result<AppRecord, ApiError> {
    let rec = require_app(st, app).await?;
    if !(also || is_owner_or_builder(who, &rec)) { return Err(ApiError(StatusCode::FORBIDDEN, format!("{what} of `{app}` are for its owner ({}), its builders and admins", rec.owner.as_deref().unwrap_or("nobody")))); }
    Ok(rec)
}

/// Every role name `app` knows: the roles recorded at creation, those its live version declares (endpoint and front allow
/// lists, `[auth] roles`) and its live stores' roles.
pub(crate) async fn known_app_roles(st: &AppState, rec: &AppRecord) -> Result<std::collections::BTreeSet<String>, ApiError> {
    let mut known: std::collections::BTreeSet<String> = rec.roles.iter().cloned().collect();
    if let Some((_, wfs, cori_toml)) = promoted_version(st, &rec.id).await? { known.extend(declared_roles(&wfs, &cori_toml)); }
    for r in sqlx::query("SELECT roles_yaml FROM store_instances WHERE app = $1 AND target = 'live'").bind(&rec.id).fetch_all(&st.db).await? {
        if let Ok(m) = serde_json::from_str::<BTreeMap<String, String>>(&r.get::<String, _>("roles_yaml")) { known.extend(m.into_keys()); }
    }
    Ok(known)
}

/// The app roles that make `who` (scoped to the app) one of its users. Under an issuer every client role of the app
/// counts (the identity provider grants them per app); dev and off auth carry one role list for everything, so only
/// the names the app knows count there.
pub(crate) async fn member_roles(st: &AppState, who: &Principal, rec: &AppRecord) -> Result<Vec<String>, ApiError> {
    if who.claims.is_object() || who.is_agent() { return Ok(who.roles.clone()); }
    let known = known_app_roles(st, rec).await?;
    Ok(who.roles.iter().filter(|r| known.contains(*r)).cloned().collect())
}

/// Whether a store call on `at` may fall back to the read-only inspector role: the app's owner, builders and admins, and
/// approvers on a sandbox; an agent session on sandboxes only (the live data is a person's to look at, and audited).
pub(crate) fn may_inspect(st: &AppState, who: &Principal, rec: &AppRecord, at: &crate::stores::Target) -> bool {
    if at.is_live() { !who.is_agent() && is_owner_or_builder(who, rec) } else { is_owner_or_builder(who, rec) || (!who.is_agent() && is_approver(st, who)) }
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use crate::auth::PrincipalKind;

    #[test]
    fn a_fronts_token_reaches_its_apps_runtime_routes_only() {
        assert!(front_may_call("desk", "POST", "/v1/apps/{app}/wf/{wf}/runs", "/v1/apps/desk/wf/triage/runs").is_ok());
        assert!(front_may_call("desk", "POST", "/v1/apps/{app}/approvals/{id}", "/v1/apps/desk/approvals/a1").is_ok());
        assert!(front_may_call("desk", "GET", "/v1/me", "/v1/me").is_ok());
        assert!(front_may_call("desk", "POST", "/v1/apps/{app}/wf/{wf}/runs", "/v1/apps/other/wf/triage/runs").is_err(), "another app");
        for (m, r) in [("POST", "/v1/apps/{app}/versions"), ("POST", "/v1/apps/{app}/versions/{hash}/promote"), ("POST", "/v1/connectors"), ("POST", "/v1/secrets"), ("POST", "/mcp"), ("GET", "/v1/audit"), ("DELETE", "/v1/apps/{app}")] {
            assert!(front_may_call("desk", m, r, &r.replace("{app}", "desk").replace("{hash}", "h")).is_err(), "{m} {r}");
        }
    }

    #[test]
    fn a_front_host_serves_its_own_app_only() {
        let ok = |m: &str, r: &str, p: &str| front_host_serves("desk", m, r, p).map(|_| 200).unwrap_or_else(|r| r.status().as_u16());
        // its front, previews and callback, and its runtime routes
        assert_eq!(ok("GET", "/apps/{app}/", "/apps/desk/"), 200);
        assert_eq!(ok("GET", "/apps/{app}/{*path}", "/apps/desk/assets/app.js"), 200);
        assert_eq!(ok("GET", "/apps/{app}/@{hash}/{*path}", "/apps/desk/@abc/index.html"), 200);
        assert_eq!(ok("POST", "/v1/apps/{app}/wf/{wf}/runs", "/v1/apps/desk/wf/triage/runs"), 200);
        assert_eq!(ok("GET", "/v1/me", "/v1/me"), 200);
        assert_eq!(ok("GET", "/__cori/pass", "/__cori/pass"), 200);
        assert_eq!(ok("POST", "/__cori/pass", "/__cori/pass"), 404);
        // never the console (its `/` is the front, which `front_hosts` placed under `/apps/<app>/`)
        assert_eq!(ok("GET", "/", "/"), 404);
        // another app, on its routes or its front
        assert_eq!(ok("GET", "/apps/{app}/", "/apps/crm/"), 404);
        assert_eq!(ok("POST", "/v1/apps/{app}/wf/{wf}/runs", "/v1/apps/crm/wf/x/runs"), 404);
        // the platform: the console's host serves it
        for (m, r) in [("POST", "/v1/apps/{app}/versions"), ("GET", "/v1/secrets"), ("POST", "/mcp"), ("POST", "/v1/leases"), ("GET", "/healthz"), ("GET", "/.well-known/oauth-protected-resource")] {
            assert_eq!(ok(m, r, &r.replace("{app}", "desk")), 404, "{m} {r}");
        }
    }

    #[test]
    fn a_guest_reaches_the_apps_shared_with_them_only() {
        let guest = Principal { sub: "pat".into(), roles: vec!["guest".into()], email: None, claims: json!({ "resource_access": { "desk--acme": { "roles": ["support_agent"] } } }), token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        assert!(guest.is_guest());
        let call = |m: &str, r: &str, app: Option<&str>| code(guest_may_call(&guest, m, r, app, app.map(|a| format!("{a}--acme")).as_deref()));
        // their user, organizations and apps, and the runtime, data and approvals of the app shared with them
        for (m, r) in GUEST_ROUTES { assert_eq!(call(m, r, r.contains("{app}").then_some("desk")), 200, "{m} {r}"); }
        // an app not shared with them
        assert_eq!(call("GET", "/v1/apps/{app}", Some("crm")), 403);
        assert_eq!(call("POST", "/v1/apps/{app}/stores/{name}/mcp", Some("crm")), 403);
        // everything else: the people, the platform, connectors, the remote MCP, previews, building
        for (m, r) in [("GET", "/v1/people"), ("GET", "/v1/organization"), ("POST", "/v1/invitations"), ("GET", "/v1/apps/{app}/access"), ("POST", "/v1/apps/{app}/access"),
                       ("POST", "/mcp"), ("GET", "/mcp"), ("GET", "/v1/connectors"), ("POST", "/v1/connectors"), ("GET", "/v1/apps"), ("POST", "/v1/apps"), ("POST", "/v1/apps/{app}/versions"),
                       ("GET", "/v1/apps/{app}/versions"), ("POST", "/v1/apps/{app}/versions/{hash}/wf/{wf}/runs"), ("POST", "/v1/apps/{app}/versions/{hash}/stores/{name}/mcp"), ("GET", "/v1/secrets"),
                       ("GET", "/v1/audit"), ("GET", "/v1/llm"), ("GET", "/v1/start"), ("GET", "/v1/identity/catalog"), ("GET", "/v1/access-requests"), ("DELETE", "/v1/apps/{app}")] {
            assert_eq!(call(m, r, r.contains("{app}").then_some("desk")), 403, "{m} {r}");
        }
        // someone holding the guest role is a guest, whatever else they hold
        let both = Principal { roles: vec!["guest".into(), "it_admin".into()], ..guest.clone() };
        assert!(both.is_guest());
        assert_eq!(code(guest_may_call(&both, "GET", "/v1/people", None, None)), 403);
    }

    #[test]
    fn a_member_who_builds_nothing_reaches_their_apps_and_organizations_only() {
        // what a guest reaches, the organizations they belong to, leaving this one, a release's runs and data (the handlers
        // keep those to whoever may open the release)
        for (m, r) in GUEST_ROUTES.iter().chain(MEMBER_APP_ROUTES) { assert!(member_may_call(m, r), "{m} {r}"); }
        // the organization's people and settings, connectors, the first steps, the remote MCP, building apps (creating one
        // reaches its handler, which refuses them first)
        for (m, r) in [("GET", "/v1/people"), ("GET", "/v1/organization"), ("PATCH", "/v1/organization"), ("GET", "/v1/invitations"), ("GET", "/v1/access-requests"), ("GET", "/v1/connectors"),
                       ("POST", "/v1/connector-requests"), ("GET", "/v1/start"), ("POST", "/mcp"), ("GET", "/v1/apps"), ("GET", "/v1/apps/{app}/versions"), ("POST", "/v1/apps/{app}/versions"),
                       ("GET", "/v1/apps/{app}/access"), ("GET", "/v1/agent-sessions"), ("GET", "/v1/llm"), ("GET", "/v1/audit"), ("GET", "/v1/secrets"), ("DELETE", "/v1/apps/{app}")] {
            assert!(!member_may_call(m, r), "{m} {r}");
        }
        // every route a front's token reaches is one of theirs: an app's front works the same for them
        for (m, r) in FRONT_ROUTES { assert!(member_may_call(m, r), "{m} {r}"); }
    }

    #[test]
    fn a_bearer_is_read_unverified_to_narrow_only() {
        use base64::Engine;
        let enc = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        let tok = format!("{}.{}.sig", enc(&json!({ "alg": "RS256" })), enc(&json!({ "sub": "pat", "realm_access": { "roles": ["guest"] } })));
        let builtin = crate::auth::Authenticator::new(crate::auth::AuthMode::Builtin { issuer: "https://idp.example".into(), audience: None, roles_claim: "realm_access.roles".into(), subject_claim: "sub".into() });
        assert_eq!(builtin.unverified(&tok).map(|p| (p.sub, p.roles)), Some(("pat".to_string(), vec!["guest".to_string()])));
        assert!(builtin.unverified("not a jwt").is_none());
        let by_oid = crate::auth::Authenticator::new(crate::auth::AuthMode::Builtin { issuer: "https://idp.example".into(), audience: None, roles_claim: "realm_access.roles".into(), subject_claim: "oid".into() });
        assert!(by_oid.unverified(&tok).is_none(), "a token without the subject claim names no one");
        let tok_oid = format!("{}.{}.sig", enc(&json!({ "alg": "RS256" })), enc(&json!({ "sub": "pairwise-1", "oid": "pat-oid" })));
        assert_eq!(by_oid.unverified(&tok_oid).map(|p| p.sub), Some("pat-oid".to_string()));
        let dev = crate::auth::Authenticator::new(crate::auth::AuthMode::Dev);
        assert_eq!(dev.unverified("dev:pat:guest,support_agent").map(|p| p.roles), Some(vec!["guest".to_string(), "support_agent".to_string()]));
    }

    #[test]
    fn a_front_is_served_at_the_root_of_its_host() {
        use axum::http::Method;
        let at = |m: &Method, p: &str| front_path("desk", None, m, p);
        // the front: every other path of the host, `__cori/` included
        assert_eq!(at(&Method::GET, "/"), FrontPath::Front("/apps/desk/".into()));
        assert_eq!(at(&Method::GET, "/assets/app-3f9c2b1a.js"), FrontPath::Front("/apps/desk/assets/app-3f9c2b1a.js".into()));
        assert_eq!(at(&Method::GET, "/__cori/config.json"), FrontPath::Front("/apps/desk/__cori/config.json".into()));
        assert_eq!(at(&Method::HEAD, "/healthz"), FrontPath::Front("/apps/desk/healthz".into()), "the app's path, not the server's");
        // the runtime API, the callback its identity provider knows, previews, as they are
        for p in ["/v1/apps/desk/wf/triage/runs", "/v1/me", "/apps/desk/callback", "/apps/desk/@v12/", "/apps/crm/", "/__cori/pass"] { assert_eq!(at(&Method::GET, p), FrontPath::Keep, "{p}"); }
        // a release's host: its release at the root, nothing of another one
        let v12 = |m: &Method, p: &str| front_path("desk", Some(12), m, p);
        assert_eq!(v12(&Method::GET, "/"), FrontPath::Front("/apps/desk/@v12/".into()));
        assert_eq!(v12(&Method::GET, "/__cori/config.json"), FrontPath::Front("/apps/desk/@v12/__cori/config.json".into()));
        assert_eq!(v12(&Method::GET, "/apps/desk/@v13/"), FrontPath::NotServed);
        assert_eq!(v12(&Method::GET, "/apps/desk/@v12/x"), FrontPath::NotServed, "only through its root");
        assert_eq!(v12(&Method::GET, "/apps/desk/x"), FrontPath::Moved("/x".into()));
        assert_eq!(v12(&Method::GET, "/apps/desk/callback"), FrontPath::Keep);
        assert_eq!(v12(&Method::GET, "/__cori/pass"), FrontPath::Keep);
        assert_eq!(v12(&Method::POST, "/v1/apps/desk/wf/triage/runs"), FrontPath::Keep);
        assert_eq!(at(&Method::POST, "/v1/apps/desk/wf/triage/runs"), FrontPath::Keep);
        // older links move to the root, with one slash
        assert_eq!(at(&Method::GET, "/apps/desk"), FrontPath::Moved("/".into()));
        assert_eq!(at(&Method::GET, "/apps/desk/"), FrontPath::Moved("/".into()));
        assert_eq!(at(&Method::GET, "/apps/desk/tickets/42"), FrontPath::Moved("/tickets/42".into()));
        assert_eq!(at(&Method::GET, "/apps/desk//evil.example/x"), FrontPath::Moved("/evil.example/x".into()));
        // nothing outside the API takes a write
        for p in ["/mcp", "/", "/apps/desk/x"] { assert_eq!(at(&Method::POST, p), FrontPath::NotServed, "{p}"); }
    }

    #[test]
    fn a_host_reaches_the_runs_of_its_own_stores() {
        let h = "sha256:ab".to_string();
        assert!(RunScope::All.sees(Some(&h)) && RunScope::All.sees(None));
        assert!(RunScope::Live.sees(None) && RunScope::Live.sees(Some(cori_store::LIVE)) && !RunScope::Live.sees(Some(&h)));
        let r = RunScope::Release(h.clone());
        assert!(r.sees(Some(&h)) && !r.sees(None) && !r.sees(Some(cori_store::LIVE)) && !r.sees(Some("sha256:cd")));
        assert_eq!((RunScope::All.target(), RunScope::Live.target(), r.target()), ("*", cori_store::LIVE, "sha256:ab"));
    }

    #[test]
    fn azp_is_read_from_the_payload() {
        use base64::Engine;
        let enc = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        let tok = format!("{}.{}.sig", enc(&json!({ "alg": "RS256" })), enc(&json!({ "sub": "u", "azp": "support-desk" })));
        assert_eq!(jwt_azp(&tok).as_deref(), Some("support-desk"));
        assert_eq!(jwt_azp("dev:alice:support_agent"), None);
    }

    #[test]
    fn only_the_platforms_clients_decide_a_go_live() {
        use base64::Engine;
        let cfg = crate::ServerConfig::local(std::env::temp_dir());
        let enc = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        let bearer = |t: String| { let mut h = HeaderMap::new(); h.insert("authorization", format!("Bearer {t}").parse().unwrap()); h };
        let of = |azp: &str| bearer(format!("{}.{}.sig", enc(&json!({ "alg": "RS256" })), enc(&json!({ "sub": "sam", "azp": azp }))));
        // the console and the cli decide; an app's own client (what its front holds) never does, whoever signed in
        assert!(platform_decision(&cfg, false, &of(&cfg.web_client_id)));
        assert!(platform_decision(&cfg, false, &of(&cfg.cli_client_id)));
        assert!(!platform_decision(&cfg, false, &of("support-desk")));
        assert!(!platform_decision(&cfg, false, &of("an-mcp-host")), "any client but the platform's");
        // nor anything that arrives on a front's host, the console's token included
        assert!(!platform_decision(&cfg, true, &of(&cfg.web_client_id)));
        // dev auth: a dev token names no client
        assert!(platform_decision(&cfg, false, &bearer("dev:sam:approver".into())));
        assert!(!platform_decision(&cfg, true, &bearer("dev:sam:approver".into())));
        assert!(is_platform_approval("promote:desk:sha256:ab") && is_platform_approval("delete:desk"));
        assert!(!is_platform_approval("store:desk:live:tickets:1") && !is_platform_approval("r1:approve_refund"));
    }
    fn agent(sub: &str, app: &str, roles: &[&str]) -> Principal {
        Principal { sub: sub.into(), roles: roles.iter().map(|r| r.to_string()).collect(), email: None, claims: json!({ "kind": "agent", "app": app, "app_roles": [] }), token: None, kind: PrincipalKind::Agent, scopes: crate::auth::AGENT_SCOPES.iter().map(|s| s.to_string()).collect(), app: Some(app.into()), session_id: Some("j".into()), platform: None }
    }
    fn app(id: &str, owner: &str) -> AppRecord { AppRecord { id: id.into(), owner: Some(owner.into()), builders: vec![], name: id.into(), roles: vec![], template: None, promoted: None, protected: false, created_at: String::new(), client_id: None } }
    fn code(r: Result<(), ApiError>) -> u16 { r.err().map(|e| e.0.as_u16()).unwrap_or(200) }

    #[test]
    fn an_agent_session_calls_its_routes_on_its_app_only() {
        let a = agent("bo", "hello", &["builder"]);
        // the loop: read the app, deploy, preview, read sandboxes, discover connectors, the remote MCP
        assert_eq!(code(agent_may_call(&a, "GET", "/v1/apps/{app}", "/v1/apps/hello")), 200);
        assert_eq!(code(agent_may_call(&a, "POST", "/v1/apps/{app}/versions", "/v1/apps/hello/versions")), 200);
        assert_eq!(code(agent_may_call(&a, "GET", "/v1/apps/{app}/versions/{hash}/bundle", "/v1/apps/hello/versions/promoted/bundle")), 200);
        assert_eq!(code(agent_may_call(&a, "POST", "/v1/apps/{app}/versions/{hash}/stores/{name}/mcp", "/v1/apps/hello/versions/abc/stores/s/mcp")), 200);
        assert_eq!(code(agent_may_call(&a, "GET", "/v1/connectors", "/v1/connectors")), 200);
        assert_eq!(code(agent_may_call(&a, "POST", "/mcp", "/mcp")), 200);
        // another app, whatever the human may do there
        assert_eq!(code(agent_may_call(&a, "GET", "/v1/apps/{app}/versions/{hash}/bundle", "/v1/apps/other/versions/promoted/bundle")), 403);
        // a person's actions: promotion, approvals, live data, live runs, administration
        for (m, r, p) in [("POST", "/v1/apps/{app}/versions/{hash}/promote", "/v1/apps/hello/versions/abc/promote"), ("POST", "/v1/apps/{app}/approvals/{id}", "/v1/apps/hello/approvals/x"),
                          ("POST", "/v1/apps/{app}/stores/{name}/mcp", "/v1/apps/hello/stores/s/mcp"), ("POST", "/v1/apps/{app}/wf/{wf}/runs", "/v1/apps/hello/wf/w/runs"),
                          ("GET", "/v1/apps/{app}/runs", "/v1/apps/hello/runs"), ("GET", "/v1/audit", "/v1/audit"), ("GET", "/v1/secrets", "/v1/secrets"), ("POST", "/v1/connectors", "/v1/connectors")] {
            assert_eq!(code(agent_may_call(&a, m, r, p)), 403, "{m} {r}");
        }
        // a scope the token lacks
        let mut narrow = a.clone(); narrow.scopes = vec!["registry:read".into()];
        assert_eq!(code(agent_may_call(&narrow, "POST", "/v1/apps/{app}/versions", "/v1/apps/hello/versions")), 403);
    }

    /// A tenancy for the tests: a dev principal's realm is the part of its subject after `@`.
    struct ByDomain;
    impl crate::realm::Tenancy for ByDomain {
        fn realm_of<'a>(&'a self, _: &'a db::Db, who: &'a Principal) -> crate::realm::TenancyFuture<'a, Result<crate::realm::Realm, ApiError>> {
            Box::pin(async move { who.sub.split_once('@').and_then(|(_, r)| crate::realm::Realm::parse(r)).ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, format!("`{}` names no organization", who.sub))) })
        }
        fn realms<'a>(&'a self, _: &'a db::Db) -> crate::realm::TenancyFuture<'a, anyhow::Result<Vec<crate::realm::Realm>>> { Box::pin(async { Ok(vec![]) }) }
        fn admit<'a>(&'a self, _: &'a db::Db, _: &'a crate::realm::Realm, _: &'a crate::realm::RealmSettings) -> crate::realm::TenancyFuture<'a, Result<crate::realm::Admitted, ApiError>> { Box::pin(async { Err(ApiError(StatusCode::FORBIDDEN, "test".into())) }) }
    }

    /// A handle on a database nothing connects to (the test tenancy never queries).
    fn unused_db() -> db::Db { db::Db::new(sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://nobody@127.0.0.1:1/none").unwrap()) }

    #[tokio::test]
    async fn a_request_names_its_realm_by_its_bearer_or_its_workers_header() {
        let auth = crate::auth::Authenticator::new(crate::auth::AuthMode::Dev).with_agent_key([3; 32]);
        let realm = |headers: &[(&str, &str)], route: &'static str| {
            let mut h = HeaderMap::new();
            for (k, v) in headers { h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap()); }
            let auth = auth.clone();
            async move { request_realm(&auth, &unused_db(), &ByDomain, &h, route).await.map(|r| r.map(|r| r.as_str().to_string())).map_err(|e| e.0.as_u16()) }
        };
        // a person: the tenancy reads it from their token; a token naming none is refused
        assert_eq!(realm(&[("authorization", "Bearer dev:jean@org-a:builder")], "/v1/apps").await, Ok(Some("org-a".into())));
        assert_eq!(realm(&[("authorization", "Bearer dev:jean:builder")], "/v1/apps").await, Err(401));
        // an agent session: the realm it was minted in (its key is checked later, by that realm's authenticator)
        let human = Principal { sub: "jean".into(), roles: vec![], email: None, claims: Value::Null, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        let minted = auth.mint_agent(&human, "desk", vec![], 600, None, Some(&crate::realm::Realm::new("org-b"))).unwrap().token;
        assert_eq!(realm(&[("authorization", &format!("Bearer {minted}"))], "/mcp").await, Ok(Some("org-b".into())));
        // a worker: its header, on the workers' routes and with a worker's credentials only
        assert_eq!(realm(&[("x-cori-worker", "t"), ("x-cori-realm", "org-a")], "/v1/leases").await, Ok(Some("org-a".into())));
        assert_eq!(realm(&[("x-cori-pool", "p"), ("x-cori-pool-key", "k"), ("x-cori-realm", "org-a")], "/v1/bundles/{hash}/files").await, Ok(Some("org-a".into())));
        assert_eq!(realm(&[("x-cori-worker", "t"), ("x-cori-realm", "org-a")], "/v1/apps").await, Ok(None), "not a workers' route");
        assert_eq!(realm(&[("x-cori-realm", "org-a")], "/v1/leases").await, Ok(None), "no worker credentials");
        assert_eq!(realm(&[("x-cori-worker", "t"), ("x-cori-realm", "org.a")], "/v1/leases").await, Err(400));
        // nothing names one: outside any realm
        assert_eq!(realm(&[], "/v1/auth/config").await, Ok(None));
    }

    #[test]
    fn platform_powers_name_the_roles_that_grant_them() {
        let human = |roles: &[&str]| Principal { kind: PrincipalKind::Human, app: None, scopes: vec![], ..agent("u", "hello", roles) };
        let approvers = vec!["approver".to_string()];
        assert_eq!(missing_power(&human(&["it_admin"]), &[Power::Admin], &approvers), None);
        assert_eq!(missing_power(&human(&["auditor"]), &[Power::Billing], &approvers), None, "billing includes auditors");
        assert_eq!(missing_power(&human(&["builder"]), &[Power::Operator, Power::Builder], &approvers), None);
        assert_eq!(missing_power(&human(&["member"]), &[Power::Admin], &approvers).as_deref(), Some("it_admin, org_admin"));
        assert_eq!(missing_power(&human(&["member"]), &[Power::Approver], &approvers).as_deref(), Some("approver"));
        // an agent session holds no power, whatever its human's roles
        assert!(missing_power(&agent("jean", "hello", &["it_admin", "approver"]), &[Power::Admin], &approvers).is_some());
        assert!(missing_power(&agent("jean", "hello", &["it_admin", "approver"]), &[Power::Approver], &approvers).is_some());
    }

    #[test]
    fn an_agent_session_builds_its_own_app_only() {
        // its human's rights on the app it is bound to (an admin's too), none elsewhere, and no platform power
        let owner = agent("bo", "hello", &["builder"]);
        assert!(is_owner_or_builder(&owner, &app("hello", "bo")));
        assert!(!is_owner_or_builder(&owner, &app("other", "bo")));
        let admin = agent("jean", "hello", &["it_admin"]);
        assert!(is_owner_or_builder(&admin, &app("hello", "bo")));
        assert!(!is_owner_or_builder(&admin, &app("other", "bo")));
        assert!(!admin.is_admin() && !admin.is_store_admin() && !admin.is_auditor() && !admin.is_operator());
        let human = Principal { kind: PrincipalKind::Human, app: None, scopes: vec![], ..admin.clone() };
        assert!(is_owner_or_builder(&human, &app("other", "bo")) && human.is_admin());
    }
    #[test]
    fn a_persons_roles_in_an_app_come_from_the_directory_when_their_token_has_none() {
        let person = |claims: Value| Principal { sub: "u1".into(), roles: vec![], email: None, claims, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        // the console's token (or the cli's, an MCP host's) may lack the app client's roles; the app's own token, an agent
        // session, a release's or a portal's session, a dev token hold theirs
        assert!(lacks_app_roles(&person(json!({ "azp": "cori-web" })), Some("desk")));
        assert!(lacks_app_roles(&person(json!({})), Some("desk")), "a token naming no client");
        assert!(!lacks_app_roles(&person(json!({ "azp": "desk" })), Some("desk")));
        assert!(!lacks_app_roles(&person(json!({ "azp": "cori-web" })), None), "an app with no client yet");
        assert!(!lacks_app_roles(&person(Value::Null), Some("desk")), "dev and off auth");
        assert!(!lacks_app_roles(&agent("u1", "desk", &[]), Some("desk")));
        assert!(!lacks_app_roles(&Principal { session_id: Some("j".into()), platform: Some(vec![]), ..person(json!({ "kind": "preview" })) }, Some("desk")));
        assert!(!lacks_app_roles(&Principal { session_id: Some("j".into()), ..person(json!({ "kind": "portal" })) }, Some("desk")));
        // `access.list`'s grants: the person as an object or an id, others left out, a role once
        let listed = json!({ "grants": [
            { "person": { "id": "u1", "email": "u1@acme.example" }, "roles": ["shopper"] },
            { "person": "u1", "roles": ["catalog_manager", "shopper"] },
            { "person": { "id": "u2" }, "roles": ["admin"] },
        ] });
        assert_eq!(roles_in_grants(&listed, "u1"), vec!["catalog_manager".to_string(), "shopper".to_string()]);
        assert!(roles_in_grants(&listed, "u3").is_empty() && roles_in_grants(&json!({}), "u1").is_empty());
    }
}

// SPDX-License-Identifier: Apache-2.0
//! Signing in on a front's own host where the identity provider is not asked: a release's host (`<app>--v<n>`), and an
//! organization's apps portal. The server is the issuer the page signs in with (`/__cori/oidc`: discovery, the
//! authorization code with PKCE, refresh and the end of a session, as any OIDC client speaks them), and the person proves
//! who they are in the console, where they are signed in: `authorize` sends the browser there (`#/preview/<app>/v<n>`,
//! `#/portal`), the console asks for a code (`POST /v1/apps/{app}/versions/{hash}/preview-codes`, `POST /v1/portal-codes`)
//! and sends the browser back to the host's callback, where the page trades the code for a session. The session is Cori's
//! own (`auth::SessionClaims`) and heard on that host only, so nothing a release's code holds reaches the live data, a
//! go-live or another release, and a portal's reaches the person and their apps only. No identity provider registers
//! anything for them: a release can be used as soon as it is deployed. Those who build an app may try its releases with
//! roles of their choosing (`PUT /v1/apps/{app}/preview-roles`), which their releases' sessions then hold in place of
//! their own: nothing they could not reach by changing the release, and nothing the identity provider hears of.
use super::*;
use crate::auth::SessionClaims;

/// The routes of a front's own sign-in, served on a front's host only.
pub(crate) const SESSION_ROUTES: &[(&str, &str)] = &[
    ("GET", "/__cori/oidc/.well-known/openid-configuration"), ("GET", "/__cori/oidc/authorize"), ("POST", "/__cori/oidc/token"), ("GET", "/__cori/oidc/logout"),
];

/// The client a release's front signs in as (its `config.json`'s `oidc.client_id`), and the portal's.
pub(crate) const PREVIEW_CLIENT: &str = "preview";
pub(crate) const PORTAL_CLIENT: &str = "portal";
/// How long a session's access token lasts; a portal's session no longer (the roles it lists are at most that old).
const ACCESS_SECS: i64 = 3600;
/// How long a release's session is renewed after its sign-in (a reload of the page does not go through the console again).
const PREVIEW_SESSION_SECS: i64 = 8 * 3600;
/// How long a code waits for the page to trade it.
const CODE_SECS: i64 = 60;

/// A host that signs its people in through the console: a release's, or an organization's apps portal.
enum SessionHost { Release { app: String, seq: i64, origin: String }, Portal { origin: String } }

impl SessionHost {
    fn of(st: &AppState, front: Option<&FrontHost>, portal: Option<&PortalHost>) -> Result<Self, ApiError> {
        if let Some(p) = portal { return Ok(SessionHost::Portal { origin: p.origin.clone() }); }
        match front {
            Some(FrontHost { app, release: Some(seq), .. }) => Ok(SessionHost::Release { app: app.clone(), seq: *seq, origin: st.release_origin(app, *seq) }),
            _ => Err(ApiError(StatusCode::NOT_FOUND, "this host signs its people in with the identity provider".into())),
        }
    }
    fn origin(&self) -> &str { match self { SessionHost::Release { origin, .. } | SessionHost::Portal { origin } => origin } }
    fn kind(&self) -> &'static str { match self { SessionHost::Release { .. } => "preview", SessionHost::Portal { .. } => "portal" } }
    fn client(&self) -> &'static str { match self { SessionHost::Release { .. } => PREVIEW_CLIENT, SessionHost::Portal { .. } => PORTAL_CLIENT } }
    fn issuer(&self) -> String { format!("{}/__cori/oidc", self.origin()) }
    /// Where the issuer answers: a release's front's callback (`/apps/<app>/callback`, as its client expects), the portal's
    /// page itself.
    fn callback(&self) -> String { match self { SessionHost::Release { app, origin, .. } => crate::names::callback_url(origin, app), SessionHost::Portal { origin } => format!("{origin}/") } }
    /// The console's page that gives this host its code.
    fn console_page(&self) -> String { match self { SessionHost::Release { app, seq, .. } => format!("#/preview/{app}/v{seq}"), SessionHost::Portal { .. } => "#/portal".into() } }
}

/// The origin of the host a request is on, when that host signs its people in through the console: a release's, a portal.
pub(crate) fn session_host_origin(st: &AppState, front: Option<&FrontHost>, portal: Option<&PortalHost>) -> Option<String> {
    SessionHost::of(st, front, portal).ok().map(|h| h.origin().to_string())
}

/// The `oidc` block of a release's `config.json`: its own host as the issuer, `@cori-do/client` following it as it is.
pub(crate) fn preview_oidc(st: &AppState, app: &str, seq: i64) -> Value {
    let origin = st.release_origin(app, seq);
    json!({ "issuer": format!("{origin}/__cori/oidc"), "client_id": PREVIEW_CLIENT, "resource": origin, "redirect_uri": crate::names::callback_url(&origin, app) })
}

/// A PKCE challenge as S256 makes it: base64url, 43 to 128 characters.
pub(crate) fn challenge_ok(c: &str) -> bool { (43..=128).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') }

/// Whether `verifier` is the one `challenge` was made from (S256).
pub(crate) fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    use sha2::Digest;
    let made = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sha2::Sha256::digest(verifier.as_bytes()));
    !verifier.is_empty() && made.len() == challenge.len() && made.bytes().zip(challenge.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

pub(crate) async fn session_discovery(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, portal: Option<axum::Extension<PortalHost>>) -> Res {
    let h = SessionHost::of(&st, front.as_ref().map(|f| &f.0), portal.as_ref().map(|p| &p.0))?;
    let i = h.issuer();
    Ok(Json(json!({
        "issuer": i, "authorization_endpoint": format!("{i}/authorize"), "token_endpoint": format!("{i}/token"), "end_session_endpoint": format!("{i}/logout"),
        "response_types_supported": ["code"], "grant_types_supported": ["authorization_code", "refresh_token"], "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"], "subject_types_supported": ["public"],
    })).into_response())
}

/// Whether a request loads a page into a frame (`Sec-Fetch-Dest`, as browsers send it): another tool previewing a release.
pub(crate) fn is_framed(headers: &HeaderMap) -> bool {
    matches!(headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()), Some("iframe" | "frame"))
}

/// Where a sign-in that asks who is signed in goes (`ask`: its `redirect_uri`, `code_challenge`, `state`, `organization`):
/// the console's page (`console_page`, `#/preview/<app>/v<n>` or `#/pass/…`). A release's (`seq`) loaded in a frame goes
/// to the deployment's own sign-in page instead when it names one (`CORI_PREVIEW_SIGN_IN_URL`), told which hop it is
/// (`kind`: `preview`, or `pass` for its files), the app and the release: the console is never framed, and that page,
/// on the origin of the tool that frames the release, asks for the code as the person (`preview-codes`, `pass-codes`),
/// which checks that the callback is the release's. A release opened in a tab of its own, and a live front, still go through the console.
pub(crate) fn sign_in_hop(cfg: &crate::ServerConfig, framed: bool, console_page: &str, kind: &str, app: &str, seq: Option<i64>, ask: &[(&str, String)]) -> String {
    let query = |pairs: &[(&str, String)]| pairs.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&");
    match (&cfg.preview_sign_in_url, seq) {
        (Some(page), Some(seq)) if framed => {
            let mut pairs = vec![("kind", kind.to_string()), ("app", app.to_string()), ("release", format!("v{seq}"))];
            pairs.extend(ask.iter().cloned());
            format!("{page}?{}", query(&pairs))
        }
        _ => format!("{}/{console_page}?{}", cfg.public_url.trim_end_matches('/'), query(ask)),
    }
}

/// `GET /__cori/oidc/authorize`: the request checked (this host's client and callback, an S256 challenge), the browser goes
/// to the console, which knows who is signed in (`sign_in_hop`: a framed release, to the deployment's own sign-in page).
/// Under a tenancy the console signs in to this host's organization.
pub(crate) async fn session_authorize(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, portal: Option<axum::Extension<PortalHost>>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    let h = SessionHost::of(&st, front.as_ref().map(|f| &f.0), portal.as_ref().map(|p| &p.0))?;
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let bad = |why: &str| Err(ApiError(StatusCode::BAD_REQUEST, format!("this sign-in request is not this host's: {why}")));
    if get("response_type") != "code" { return bad("response_type is code"); }
    if get("client_id") != h.client() { return bad("unknown client"); }
    if get("redirect_uri") != h.callback() { return bad("redirect_uri is not this host's callback"); }
    if get("code_challenge_method") != "S256" || !challenge_ok(get("code_challenge")) { return bad("an S256 code_challenge is needed"); }
    let mut ask = vec![("redirect_uri", get("redirect_uri").to_string()), ("code_challenge", get("code_challenge").to_string()), ("state", get("state").to_string())];
    if !st.settings.slug.is_empty() { ask.push(("organization", st.settings.slug.clone())); }
    let (app, seq) = match &h { SessionHost::Release { app, seq, .. } => (app.as_str(), Some(*seq)), SessionHost::Portal { .. } => ("", None) };
    let to = sign_in_hop(&st.cfg, is_framed(&headers), &h.console_page(), "preview", app, seq, &ask);
    Ok((StatusCode::FOUND, [(axum::http::header::LOCATION, to), (axum::http::header::CACHE_CONTROL, "no-store".to_string())]).into_response())
}

/// `POST /v1/apps/{app}/versions/{hash}/preview-codes` `{ code_challenge, redirect_uri }`: a code a release's host trades for a
/// session (`session_token`), for the person signed in to the console: the app's owner, its builders, approvers and admins
/// (whoever may run the release on its sandbox), never an agent session. The release's roles are their roles in the app
/// (`app_roles_of`): their console token's, else the identity provider's (`directory`), asked again at each refresh; for
/// those who build the app, the roles they chose to try its releases with, when they chose some (`preview_roles`).
#[derive(serde::Deserialize)]
pub(crate) struct CodeAsk { code_challenge: String, redirect_uri: String }

pub(crate) async fn preview_code(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller, Json(ask): Json<CodeAsk>) -> Res {
    if front.is_some() { return Err(ApiError(StatusCode::NOT_FOUND, "the console's host gives a release's sign-in".into())); }
    require_human(&who, "opening a release")?;
    let rec = require_app(&st, &app).await?;
    if !(is_owner_or_builder(&who, &rec) || is_approver(&st, &who) || who.is_builder()) {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("opening a release of `{app}` needs its owner, a builder, an approver or an admin (you are `{}`)", who.sub)));
    }
    let (version, _, _) = resolve_version(&st, &app, &hash).await?;
    let seq: i64 = sqlx::query_scalar::<_, Option<i64>>("SELECT seq FROM versions WHERE hash = $1 AND app = $2").bind(&version).bind(&app).fetch_optional(&st.db).await?.flatten()
        .ok_or(ApiError(StatusCode::NOT_FOUND, format!("version {hash} of `{app}` has no release number")))?;
    let host = SessionHost::Release { origin: st.release_origin(&app, seq), app: app.clone(), seq };
    if ask.redirect_uri != host.callback() { return Err(ApiError(StatusCode::BAD_REQUEST, "redirect_uri is not the release's callback".into())); }
    if !challenge_ok(&ask.code_challenge) { return Err(ApiError(StatusCode::BAD_REQUEST, "an S256 code_challenge is needed".into())); }
    let (app_roles, directory) = app_roles_of(&st, &who, &app, rec.client_id.as_deref()).await;
    let payload = json!({
        "kind": "preview", "app": app, "seq": seq, "version": version, "sub": who.sub, "email": who.email, "name": who.claims.get("name"),
        "app_roles": app_roles, "directory": directory, "chooses": chooser(&who, &rec), "challenge": ask.code_challenge, "redirect_uri": host.callback(),
    });
    let code = store_code(&st, &payload).await?;
    db::audit(&st.db, &who.sub, "release.signin", &format!("{app}@v{seq}"), json!({})).await?;
    Ok(Json(json!({ "code": code, "redirect_uri": host.callback() })).into_response())
}

/// `POST /v1/portal-codes` `{ code_challenge, redirect_uri }`: a code the organization's apps portal trades for a session,
/// for the person signed in to the console (any member, or a guest: it lists what they may use). The session holds their
/// roles as their console token has them.
pub(crate) async fn portal_code(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, portal: Option<axum::Extension<PortalHost>>, Caller(who): Caller, Json(ask): Json<CodeAsk>) -> Res {
    if front.is_some() || portal.is_some() { return Err(ApiError(StatusCode::NOT_FOUND, "the console's host gives the portal's sign-in".into())); }
    require_human(&who, "opening the apps portal")?;
    let url = st.portal_url().ok_or(ApiError(StatusCode::NOT_FOUND, "this organization has no apps portal".into()))?;
    let host = SessionHost::Portal { origin: url.trim_end_matches('/').to_string() };
    if ask.redirect_uri != host.callback() { return Err(ApiError(StatusCode::BAD_REQUEST, "redirect_uri is not the portal's".into())); }
    if !challenge_ok(&ask.code_challenge) { return Err(ApiError(StatusCode::BAD_REQUEST, "an S256 code_challenge is needed".into())); }
    let payload = json!({
        "kind": "portal", "origin": host.origin(), "sub": who.sub, "email": who.email, "name": who.claims.get("name"),
        "roles": who.platform_roles(), "client_roles": who.client_roles().unwrap_or_default(),
        "challenge": ask.code_challenge, "redirect_uri": host.callback(),
    });
    let code = store_code(&st, &payload).await?;
    db::audit(&st.db, &who.sub, "portal.signin", host.origin(), json!({})).await?;
    Ok(Json(json!({ "code": code, "redirect_uri": host.callback() })).into_response())
}

/// Keeps `payload` under a new code, good for one token for `CODE_SECS`; the expired ones go.
pub(crate) async fn store_code(st: &AppState, payload: &Value) -> Result<String, ApiError> {
    let code = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, rand::random::<[u8; 32]>());
    let now = chrono::Utc::now().timestamp();
    sqlx::query("DELETE FROM front_codes WHERE expires_at < $1").bind(now.to_string()).execute(&st.db).await?;
    sqlx::query("INSERT INTO front_codes (code_hash, payload, expires_at) VALUES ($1, $2, $3)").bind(sha(&code)).bind(payload.to_string()).bind((now + CODE_SECS).to_string()).execute(&st.db).await?;
    Ok(code)
}

/// The payload of `code`, once: the row goes as it is read, and an expired one gives nothing.
pub(crate) async fn take_code(st: &AppState, code: &str) -> Result<Option<Value>, ApiError> {
    let row = sqlx::query("DELETE FROM front_codes WHERE code_hash = $1 RETURNING payload, expires_at").bind(sha(code)).fetch_optional(&st.db).await?;
    Ok(row.filter(|r| r.get::<String, _>("expires_at").parse::<i64>().unwrap_or(0) >= chrono::Utc::now().timestamp()).and_then(|r| serde_json::from_str(&r.get::<String, _>("payload")).ok()))
}

/// What a session's refresh token keeps, sealed for its host (the host's origin as associated data): the person and what
/// they hold there (`app_roles`, their own), whether the identity provider gave a release's roles (`directory`: they are
/// asked again at each refresh), whether they may try a release with roles of their choosing (`chooses`), since when (the
/// access tokens' `iat`, so that someone who left is refused at once), until when.
#[derive(serde::Serialize, serde::Deserialize)]
struct Renewal {
    kind: String, sub: String, email: Option<String>, name: Option<String>, app: Option<String>, version: Option<String>, app_roles: Vec<String>,
    #[serde(default)] directory: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] chooses: Option<Chooser>,
    #[serde(default)] roles: Vec<String>, #[serde(default)] client_roles: BTreeMap<String, Vec<String>>, since: i64, until: i64,
}

/// Why a person may try an app's releases with roles of their choosing (`chooser`): they build it by name (its owner, one of
/// its builders: asked again at each refresh), or by a platform role they held when they signed in (an admin; a builder,
/// of an app nobody owns), which a release's session does not keep.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Chooser { Named, Platform }

fn renewal_key(st: &AppState) -> [u8; 32] { st.broker.derive("front-session-renewals") }

#[derive(serde::Deserialize)]
pub(crate) struct TokenForm {
    grant_type: String,
    #[serde(default)] code: String,
    #[serde(default)] code_verifier: String,
    #[serde(default)] redirect_uri: String,
    #[serde(default)] refresh_token: String,
}

/// `POST /__cori/oidc/token`: a code (with its verifier) or a refresh token for a session of this host.
pub(crate) async fn session_token(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, portal: Option<axum::Extension<PortalHost>>, axum::Form(f): axum::Form<TokenForm>) -> Res {
    let h = SessionHost::of(&st, front.as_ref().map(|f| &f.0), portal.as_ref().map(|p| &p.0))?;
    let deny = |why: &str| Ok((StatusCode::BAD_REQUEST, [(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({ "error": "invalid_grant", "error_description": why }))).into_response());
    let now = chrono::Utc::now().timestamp();
    let renewal = match f.grant_type.as_str() {
        "authorization_code" => {
            let Some(p) = take_code(&st, &f.code).await? else { return deny("unknown or expired code: sign in again") };
            if f.redirect_uri != h.callback() || p["redirect_uri"].as_str() != Some(h.callback().as_str()) { return deny("the code is not for this callback"); }
            let ours = match &h {
                SessionHost::Release { app, seq, .. } => p["kind"] == "preview" && p["app"].as_str() == Some(app.as_str()) && p["seq"].as_i64() == Some(*seq),
                SessionHost::Portal { origin } => p["kind"] == "portal" && p["origin"].as_str() == Some(origin.as_str()),
            };
            if !ours { return deny("the code is another host's"); }
            if !pkce_ok(&f.code_verifier, p["challenge"].as_str().unwrap_or("")) { return deny("the code_verifier does not match the code's challenge"); }
            let text = |k: &str| p[k].as_str().map(String::from);
            let list = |k: &str| p[k].as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
            let until = now + if matches!(h, SessionHost::Portal { .. }) { ACCESS_SECS } else { PREVIEW_SESSION_SECS };
            Renewal { kind: h.kind().into(), sub: text("sub").unwrap_or_default(), email: text("email"), name: text("name"), app: text("app"), version: text("version"), app_roles: list("app_roles"), directory: p["directory"].as_bool().unwrap_or(false), chooses: serde_json::from_value(p["chooses"].clone()).unwrap_or(None), roles: list("roles"), client_roles: serde_json::from_value(p["client_roles"].clone()).unwrap_or_default(), since: now, until }
        }
        "refresh_token" => {
            let Ok(r) = crate::secrets::open_native(&renewal_key(&st), h.origin(), &f.refresh_token).map_err(|_| ()).and_then(|j| serde_json::from_str::<Renewal>(&j).map_err(|_| ())) else { return deny("unknown refresh token") };
            if r.until <= now || r.kind != h.kind() { return deny("the session ended: sign in again"); }
            let mut r = r;
            // roles the identity provider gave follow its grants (a failed answer keeps the session's)
            if r.directory && let SessionHost::Release { app, .. } = &h && let Some(client) = app_record(&st, app).await?.and_then(|a| a.client_id)
                && let Some(roles) = directory_app_roles(&st, &r.sub, app, &client).await { r.app_roles = roles; }
            r
        }
        _ => return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": "unsupported_grant_type" }))).into_response()),
    };
    if renewal.sub.is_empty() { return deny("the code names nobody"); }
    // the roles chosen to try the app's releases with, read at each sign-in and refresh (a reload of the page)
    let app_roles = match &h {
        SessionHost::Release { app, .. } => preview_roles(&st, app, &renewal).await?.unwrap_or_else(|| renewal.app_roles.clone()),
        SessionHost::Portal { .. } => renewal.app_roles.clone(),
    };
    let exp = (now + ACCESS_SECS).min(renewal.until);
    let claims = SessionClaims {
        iss: "cori".into(), kind: renewal.kind.clone(), sub: renewal.sub.clone(), aud: h.origin().to_string(), iat: renewal.since, exp, jti: uuid::Uuid::new_v4().to_string(),
        email: renewal.email.clone(), name: renewal.name.clone(), realm: st.realm().map(|r| r.as_str().to_string()), app: renewal.app.clone(), version: renewal.version.clone(), app_roles,
        roles: renewal.roles.clone(), client_roles: renewal.client_roles.clone(),
    };
    let access = st.auth.mint_session(&claims).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let refresh = crate::secrets::seal_native(&renewal_key(&st), h.origin(), &serde_json::to_string(&renewal).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?, rand::random())?;
    Ok(([(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({ "access_token": access, "token_type": "Bearer", "expires_in": exp - now, "refresh_token": refresh }))).into_response())
}

/// `GET /__cori/oidc/logout`: back to the page named, when it is this host's, else its root. The session holds nothing on
/// the server: the page drops its tokens.
pub(crate) async fn session_logout(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, portal: Option<axum::Extension<PortalHost>>, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    let h = SessionHost::of(&st, front.as_ref().map(|f| &f.0), portal.as_ref().map(|p| &p.0))?;
    let root = format!("{}/", h.origin());
    let back = q.get("post_logout_redirect_uri").filter(|u| u.starts_with(&root) && !u[root.len()..].starts_with('/')).cloned().unwrap_or(root);
    Ok((StatusCode::FOUND, [(axum::http::header::LOCATION, back)]).into_response())
}

/// What a portal's session reaches: the person and their apps.
const PORTAL_API: &[(&str, &str)] = &[("GET", "/v1/auth/config"), ("GET", "/v1/me"), ("GET", "/v1/me/apps")];

/// Whether a session may call `method route` (the matched pattern) on `path`, the request being on the host of origin `here`:
/// only the host it was signed in on (its `aud`), and there a release's session the runtime routes of its app
/// (`FRONT_ROUTES`), a portal's the person and their apps (`PORTAL_API`).
pub(crate) fn session_may_call(here: Option<&str>, who: &Principal, method: &str, route: &str, path: &str) -> Result<(), ApiError> {
    if here.is_none() || here != who.claims["aud"].as_str() { return Err(ApiError(StatusCode::FORBIDDEN, "this session is another host's: it is heard where it was signed in only".into())); }
    match who.claims["kind"].as_str() {
        Some("preview") => front_may_call(who.app.as_deref().unwrap_or_default(), method, route, path),
        Some("portal") if PORTAL_API.iter().any(|(m, p)| *m == method && *p == route) => Ok(()),
        Some("portal") => Err(ApiError(StatusCode::FORBIDDEN, format!("`{method} {route}` is not for the apps portal: it lists your apps, which you open on their own hosts"))),
        _ => Err(ApiError(StatusCode::FORBIDDEN, "unknown kind of session".into())),
    }
}

/// The app's owner or one of its builders, by name.
fn builds_by_name(rec: &AppRecord, sub: &str) -> bool { rec.owner.as_deref() == Some(sub) || rec.builders.iter().any(|b| b == sub) }

/// Whether `who` may try `rec`'s releases with roles of their choosing, and why: whoever builds the app
/// (`is_owner_or_builder`), who may change its releases anyway, so a role chosen there gives them nothing they lack. A
/// person's own token only: an agent's, a release's or a portal's session holds what it was given.
fn chooser(who: &Principal, rec: &AppRecord) -> Option<Chooser> {
    if who.is_agent() || who.session_id.is_some() || !is_owner_or_builder(who, rec) { return None; }
    Some(if builds_by_name(rec, &who.sub) { Chooser::Named } else { Chooser::Platform })
}

/// The roles `sub` chose to try `app`'s releases with (`PUT /v1/apps/{app}/preview-roles`), the deployment's own aside;
/// None when they keep their own.
async fn chosen_roles(st: &AppState, app: &str, sub: &str) -> Result<Option<Vec<String>>, ApiError> {
    let Some(j) = sqlx::query_scalar::<_, String>("SELECT roles FROM preview_roles WHERE app = $1 AND sub = $2").bind(app).bind(sub).fetch_optional(&st.db).await? else { return Ok(None) };
    let ignore = crate::client_hook::deployment_roles(st);
    Ok(Some(serde_json::from_str::<Vec<String>>(&j).unwrap_or_default().into_iter().filter(|r| !ignore.contains(r)).collect()))
}

/// The roles a release's session of `app` holds in place of its person's own (`r.app_roles`): those they chose, while they
/// may choose (by name, they still build the app); None otherwise.
async fn preview_roles(st: &AppState, app: &str, r: &Renewal) -> Result<Option<Vec<String>>, ApiError> {
    let may = match r.chooses {
        None => false,
        Some(Chooser::Platform) => true,
        Some(Chooser::Named) => app_record(st, app).await?.is_some_and(|rec| builds_by_name(&rec, &r.sub)),
    };
    if !may { return Ok(None); }
    chosen_roles(st, app, &r.sub).await
}

/// The app, when `who` may try its releases with roles of their choosing (`chooser`); a 403 otherwise.
async fn may_choose(st: &AppState, who: &Principal, app: &str) -> Result<AppRecord, ApiError> {
    require_human(who, "trying an app's releases with other roles")?;
    let rec = require_app(st, app).await?;
    if chooser(who, &rec).is_none() {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("trying `{app}`'s releases with other roles is for its owner ({}), its builders and admins", rec.owner.as_deref().unwrap_or("nobody"))));
    }
    Ok(rec)
}

/// What the preview roles' routes answer: the roles the caller tries the app's releases with (`roles`; null: their own),
/// those they may choose (`choices`) and their own in the app (`own`, as `app_roles_of` finds them).
async fn preview_roles_answer(st: &AppState, who: &Principal, rec: &AppRecord, choices: Vec<String>) -> Res {
    let roles = chosen_roles(st, &rec.id, &who.sub).await?;
    let (own, _) = app_roles_of(st, who, &rec.id, rec.client_id.as_deref()).await;
    Ok(Json(json!({ "app": rec.id, "roles": roles, "choices": choices, "own": own })).into_response())
}

/// `GET /v1/apps/{app}/preview-roles[?release=v<n>]`: the roles the caller tries the app's releases with, and those they may
/// choose (`choices`): that release's, else those of any version of the app. Its owner, its builders and admins.
pub(crate) async fn get_preview_roles(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    let rec = may_choose(&st, &who, &app).await?;
    let choices: Vec<String> = match q.get("release").map(|r| r.trim()).filter(|r| !r.is_empty()) {
        Some(release) => { let (hash, _, _) = resolve_version(&st, &app, release).await?; crate::client_hook::recorded_roles(&st, &hash).await?.into_keys().collect() }
        None => crate::client_hook::release_roles(&st, &app).await?.into_iter().collect(),
    };
    preview_roles_answer(&st, &who, &rec, choices).await
}

/// `PUT /v1/apps/{app}/preview-roles` `{ roles }`: the caller's sessions of the app's releases hold these roles (`[]`: none)
/// in place of their own, from their next sign-in or refresh: a reload of the release's page. Roles of the app's versions
/// only. The app, its live data and the identity provider are untouched. Its owner, its builders and admins.
pub(crate) async fn put_preview_roles(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    let rec = may_choose(&st, &who, &app).await?;
    let asked = b["roles"].as_array().ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`roles`: the app's roles to try its releases with (`[]` for none)".into()))?;
    let choices = crate::client_hook::release_roles(&st, &app).await?;
    let mut roles = Vec::new();
    for r in asked {
        let r = r.as_str().map(str::trim).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`roles`: names of the app's roles".into()))?;
        if !choices.contains(r) {
            let known = if choices.is_empty() { "it has none yet".to_string() } else { choices.iter().cloned().collect::<Vec<_>>().join(", ") };
            return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{r}` is not one of `{app}`'s roles ({known})")));
        }
        roles.push(r.to_string());
    }
    roles.sort(); roles.dedup();
    sqlx::query("INSERT INTO preview_roles (app, sub, roles, updated_at) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT preview_roles_pkey DO UPDATE SET roles = EXCLUDED.roles, updated_at = EXCLUDED.updated_at")
        .bind(&app).bind(&who.sub).bind(json!(roles).to_string()).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "release.preview_roles", &app, json!({ "roles": roles })).await?;
    preview_roles_answer(&st, &who, &rec, choices.into_iter().collect()).await
}

/// `DELETE /v1/apps/{app}/preview-roles`: the caller's sessions of the app's releases hold their own roles again, from their
/// next sign-in or refresh.
pub(crate) async fn delete_preview_roles(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    let rec = may_choose(&st, &who, &app).await?;
    if sqlx::query("DELETE FROM preview_roles WHERE app = $1 AND sub = $2").bind(&app).bind(&who.sub).execute(&st.db).await?.rows_affected() > 0 {
        db::audit(&st.db, &who.sub, "release.preview_roles", &app, json!({ "roles": null })).await?;
    }
    preview_roles_answer(&st, &who, &rec, crate::client_hook::release_roles(&st, &app).await?.into_iter().collect()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_traded_with_the_verifier_it_was_made_for() {
        use sha2::Digest;
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sha2::Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM", "RFC 7636's example");
        assert!(challenge_ok(&challenge) && pkce_ok(verifier, &challenge));
        assert!(!pkce_ok("another-verifier-of-a-fine-length-0123456789", &challenge));
        assert!(!pkce_ok("", &challenge));
        for bad in ["", "short", "has spaces in it, which base64url never holds, so no ok", &"a".repeat(129)] { assert!(!challenge_ok(bad), "{bad}"); }
    }

    #[test]
    fn a_session_is_heard_on_its_own_host_only() {
        let mut cfg = crate::ServerConfig::local(std::env::temp_dir());
        cfg.fronts_url = crate::names::FrontsUrl::parse("https://*.cori.example").ok();
        let (v12, v13, live) = (crate::names::release_origin(&cfg, "", "desk", 12), crate::names::release_origin(&cfg, "", "desk", 13), crate::names::front_origin(&cfg, "", "desk"));
        let sam = Principal { sub: "sam".into(), roles: vec!["support_agent".into()], email: None, claims: json!({ "kind": "preview", "aud": v12 }), token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: Some("desk".into()), session_id: Some("j".into()), platform: Some(vec![]) };
        let code = |here: Option<&str>, m: &str, r: &str| session_may_call(here, &sam, m, r, &r.replace("{app}", "desk").replace("{wf}", "triage").replace("{hash}", "v12")).map(|_| 200).unwrap_or(403);
        assert_eq!(code(Some(&v12), "POST", "/v1/apps/{app}/wf/{wf}/runs"), 200);
        assert_eq!(code(Some(&v12), "GET", "/v1/apps/{app}/approvals"), 200);
        assert_eq!(code(Some(&v13), "POST", "/v1/apps/{app}/wf/{wf}/runs"), 403, "another release");
        assert_eq!(code(Some(&live), "GET", "/v1/apps/{app}/runs"), 403, "the live host");
        assert_eq!(code(None, "GET", "/v1/me"), 403, "the console's host");
        assert_eq!(code(Some(&v12), "POST", "/v1/apps/{app}/versions/{hash}/promote"), 403, "the platform");
        assert_eq!(session_may_call(Some(&v12), &sam, "GET", "/v1/apps/{app}/runs", "/v1/apps/crm/runs").map(|_| 200).unwrap_or(403), 403, "another app");
        // a portal's: the person and their apps, on the portal only
        let portal = "https://acme.cori.example";
        let ann = Principal { claims: json!({ "kind": "portal", "aud": portal }), app: None, platform: None, ..sam.clone() };
        let at = |here: &str, m: &str, r: &str| session_may_call(Some(here), &ann, m, r, r).map(|_| 200).unwrap_or(403);
        assert_eq!((at(portal, "GET", "/v1/me/apps"), at(portal, "GET", "/v1/me")), (200, 200));
        for (m, r) in [("GET", "/v1/apps/{app}/runs"), ("POST", "/v1/apps/{app}/wf/{wf}/runs"), ("GET", "/v1/people"), ("POST", "/v1/portal-codes")] { assert_eq!(at(portal, m, r), 403, "{m} {r}"); }
        assert_eq!(at(&v12, "GET", "/v1/me/apps"), 403, "a release's host");
    }

    #[test]
    fn those_who_build_an_app_choose_the_roles_they_try_its_releases_with() {
        let person = |sub: &str, roles: &[&str]| Principal { sub: sub.into(), roles: roles.iter().map(|r| r.to_string()).collect(), email: None, claims: json!({}), token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        let app = |owner: Option<&str>| AppRecord { id: "desk".into(), owner: owner.map(String::from), builders: vec!["bo".into()], name: "desk".into(), roles: vec![], template: None, promoted: None, protected: false, created_at: String::new(), client_id: Some("desk".into()) };
        let desk = app(Some("jean"));
        assert_eq!(chooser(&person("jean", &[]), &desk), Some(Chooser::Named), "its owner");
        assert_eq!(chooser(&person("bo", &[]), &desk), Some(Chooser::Named), "a builder");
        assert_eq!(chooser(&person("ada", &["it_admin"]), &desk), Some(Chooser::Platform), "an admin");
        assert_eq!(chooser(&person("ben", &["builder"]), &app(None)), Some(Chooser::Platform), "a builder, of an app nobody owns");
        for (who, why) in [(person("ben", &["builder"]), "a builder of other apps"), (person("sam", &["approver"]), "an approver"), (person("alice", &[]), "a user of the app")] {
            assert_eq!(chooser(&who, &desk), None, "{why}");
        }
        let agent = Principal { kind: crate::auth::PrincipalKind::Agent, session_id: Some("j".into()), app: Some("desk".into()), ..person("jean", &[]) };
        assert_eq!(chooser(&agent, &desk), None, "an agent session");
        let release = Principal { claims: json!({ "kind": "preview" }), session_id: Some("j".into()), app: Some("desk".into()), platform: Some(vec![]), ..person("jean", &[]) };
        assert_eq!(chooser(&release, &desk), None, "a release's session");
        // a refresh token sealed before choices were kept reads as one without
        let old: Renewal = serde_json::from_str(r#"{"kind":"preview","sub":"jean","email":null,"name":null,"app":"desk","version":"sha256:ab","app_roles":["app_user"],"directory":true,"since":1,"until":2}"#).unwrap();
        assert_eq!((old.chooses, old.app_roles.as_slice()), (None, ["app_user".to_string()].as_slice()));
        let named = Renewal { chooses: Some(Chooser::Named), ..old };
        assert!(serde_json::to_string(&named).unwrap().contains(r#""chooses":"named""#));
    }

    #[test]
    fn a_framed_release_signs_in_through_the_deployments_own_page() {
        let mut cfg = crate::ServerConfig::local(std::env::temp_dir());
        cfg.public_url = "https://cori.example/".into();
        let ask = [("redirect_uri", "https://desk--v3.cori.example/apps/desk/callback".to_string()), ("code_challenge", "c".repeat(43)), ("state", "s t".into())];
        let console = "https://cori.example/#/preview/desk/v3?redirect_uri=https%3A%2F%2Fdesk--v3.cori.example%2Fapps%2Fdesk%2Fcallback&code_challenge=".to_string() + &"c".repeat(43) + "&state=s%20t";
        // unset: the console, framed or not
        assert_eq!(sign_in_hop(&cfg, true, "#/preview/desk/v3", "preview", "desk", Some(3), &ask), console);
        cfg.preview_sign_in_url = crate::names::sign_in_page("https://tools.example/cori/sign-in").ok();
        assert_eq!(sign_in_hop(&cfg, false, "#/preview/desk/v3", "preview", "desk", Some(3), &ask), console, "a tab of its own");
        assert_eq!(sign_in_hop(&cfg, true, "#/preview/desk/v3", "preview", "desk", Some(3), &ask),
            "https://tools.example/cori/sign-in?kind=preview&app=desk&release=v3&redirect_uri=https%3A%2F%2Fdesk--v3.cori.example%2Fapps%2Fdesk%2Fcallback&code_challenge=".to_string() + &"c".repeat(43) + "&state=s%20t");
        assert!(sign_in_hop(&cfg, true, "#/pass/desk", "pass", "desk", None, &ask).starts_with("https://cori.example/#/pass/desk?"), "a live front");
        assert!(sign_in_hop(&cfg, true, "#/portal", "preview", "", None, &ask).starts_with("https://cori.example/#/portal?"), "the portal");
        let framed = |dest: &str| { let mut h = HeaderMap::new(); h.insert("sec-fetch-dest", dest.parse().unwrap()); is_framed(&h) };
        assert!(framed("iframe") && framed("frame"));
        assert!(!framed("document") && !is_framed(&HeaderMap::new()));
    }
}

// SPDX-License-Identifier: Apache-2.0
//! A front's files for the people who may open it (`CORI_FRONTS_PRIVATE`, `ServerConfig::fronts_gated`). A front signs
//! its user in by itself and keeps the tokens in the page, which a browser never sends when it loads a page, a script or
//! an image: so a front host first gives the browser a pass of its own, a cookie, and serves its files to that browser
//! only. A page load without one goes to the console (`#/pass/<app>[/v<n>]`; a framed release, to the deployment's own
//! sign-in page when it names one, `sessions::sign_in_hop`), where the person is signed in: the console
//! asks for a code for that host (`POST /v1/apps/{app}/pass-codes`, which checks that they may open the app, or the
//! release) and sends the browser back to the host's `/__cori/pass`, which trades the code for the pass and returns to
//! the page asked for. The code is bound to the browser that left (PKCE: its verifier waits in a cookie of the host), the
//! pass to its host (sealed under a key of the realm's, the host's origin as associated data), and it lasts `PASS_SECS`.
//! So nobody outside the organization is shown a page its builders deployed; the app's own sign-in and its data are as
//! they were.
use super::*;

/// The route of a front host that trades a code for its pass.
pub(crate) const PASS_ROUTE: &str = "/__cori/pass";
/// How long a pass lasts: past a working day; then the next page load goes through the console again, which asks the
/// identity provider once more who the person is.
const PASS_SECS: i64 = 12 * 3600;
/// How long a browser has to come back from the console.
const FLOW_SECS: i64 = 600;

/// What a pass holds: who, for which front (the live one, or a release), since when, until when.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct Pass { sub: String, app: String, #[serde(default)] release: Option<i64>, iat: i64, exp: i64 }

fn pass_key(st: &AppState) -> [u8; 32] { st.broker.derive("front-passes") }

/// A pass sealed for the host of `origin`: it opens there only.
fn seal(key: &[u8; 32], origin: &str, pass: &Pass) -> Result<String, ApiError> {
    let json = serde_json::to_string(pass).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(crate::secrets::seal_native(key, origin, &json, rand::random())?)
}

fn open(key: &[u8; 32], origin: &str, sealed: &str) -> Option<Pass> {
    crate::secrets::open_native(key, origin, sealed).ok().and_then(|j| serde_json::from_str(&j).ok())
}

/// The origin of the host a front host's request is on: the live front's, or a release's.
fn host_origin(st: &AppState, front: &FrontHost) -> String {
    match front.release { Some(seq) => st.release_origin(&front.app, seq), None => st.front_origin(&front.app) }
}

/// The names of a host's pass and of its sign-ins under way (each one's own, `<name>_<flow>`): `__Host-` ones over https,
/// which only that host sets and which only travel to it, plain ones over http (a laptop).
fn cookie_names(origin: &str) -> (&'static str, &'static str) {
    if origin.starts_with("https://") { ("__Host-cori_pass", "__Host-cori_flow") } else { ("cori_pass", "cori_flow") }
}

/// A `Set-Cookie` of the host of `origin`: for its pages only (no script reads it), sent along when another site links
/// there (`Lax`: the console sends the browser back), over https only when the host is.
fn set_cookie(origin: &str, name: &str, value: &str, max_age: i64) -> String {
    format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{}", if origin.starts_with("https://") { "; Secure" } else { "" })
}

/// The value of cookie `name` among a request's.
fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(axum::http::header::COOKIE).iter().filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';')).filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

/// Whether a request loads a page (a navigation, which may go through the console), rather than a file of a page already
/// shown (a script, a style, a fetch, which can only be refused): `Sec-Fetch-Mode` as browsers send it, else what it accepts.
fn is_page_load(headers: &HeaderMap) -> bool {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or("");
    match h("sec-fetch-mode") { "" => h("accept").contains("text/html"), mode => mode == "navigate" }
}

/// The path (and query) a front host's request named, at the root of that host: `front_hosts` placed it under
/// `/apps/<app>` (`/apps/<app>/@v<n>` on a release's host).
fn asked_path(front: &FrontHost, uri: &axum::http::Uri) -> String {
    let placed = match front.release { Some(seq) => format!("/apps/{}/@v{seq}", front.app), None => format!("/apps/{}", front.app) };
    let rest = uri.path().strip_prefix(&placed).filter(|r| r.starts_with('/')).unwrap_or("/");
    format!("{rest}{}", uri.query().map(|q| format!("?{q}")).unwrap_or_default())
}

/// A sign-in's `state`: the flow (its cookie's suffix) and the page to come back to, base64url.
fn flow_state(flow: &str, back: &str) -> String { format!("{flow}.{}", base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, back)) }

/// The flow and the page of a `state` (`flow_state`). The page stays on the host: one path, never `//` (another host) nor
/// a backslash, which some browsers read as a slash; anything else comes back to the root.
fn read_state(state: &str) -> Option<(String, String)> {
    let (flow, back) = state.split_once('.')?;
    if flow.len() != 16 || !flow.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    let back = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, back).ok().and_then(|b| String::from_utf8(b).ok())
        .filter(|b| b.starts_with('/') && !b.starts_with("//") && !b.contains('\\') && !b.chars().any(char::is_control)).unwrap_or_else(|| "/".into());
    Some((flow.to_string(), back))
}

/// Before a front host serves a file (`front_index`, `front_file`, a release's `preview_*`): None when it may (no gate, or
/// the browser holds a pass of this host for what it serves), else the answer. A page load goes to the console for a pass,
/// its verifier kept in a cookie of this sign-in's own; anything else gets a 401, since a page already shown cannot
/// follow a sign-in.
pub(crate) fn admit(st: &AppState, front: Option<&FrontHost>, headers: &HeaderMap, uri: &axum::http::Uri) -> Option<Response> {
    let front = front?;
    if !st.cfg.fronts_gated() { return None; }
    let origin = host_origin(st, front);
    let (pass_name, flow_name) = cookie_names(&origin);
    if cookie(headers, pass_name).and_then(|v| open(&pass_key(st), &origin, v)).is_some_and(|p| valid_for(&p, front, chrono::Utc::now().timestamp())) { return None; }
    if !is_page_load(headers) {
        return Some((StatusCode::UNAUTHORIZED, [(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({ "error": "this app's files are for the people who may open it: load its page to sign in" }))).into_response());
    }
    let verifier = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, rand::random::<[u8; 32]>());
    let challenge = { use sha2::Digest; base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sha2::Sha256::digest(verifier.as_bytes())) };
    let flow = hex::encode(rand::random::<[u8; 8]>());
    let mut ask = vec![("redirect_uri", format!("{origin}{PASS_ROUTE}")), ("code_challenge", challenge), ("state", flow_state(&flow, &asked_path(front, uri)))];
    // under a tenancy the console signs in to this host's organization
    if !st.settings.slug.is_empty() { ask.push(("organization", st.settings.slug.clone())); }
    let page = match front.release { Some(seq) => format!("#/pass/{}/v{seq}", front.app), None => format!("#/pass/{}", front.app) };
    // a framed release: the deployment's own sign-in page, when it names one
    let to = super::sessions::sign_in_hop(&st.cfg, super::sessions::is_framed(headers), &page, "pass", &front.app, front.release, &ask);
    Some((StatusCode::FOUND, [(axum::http::header::LOCATION, to), (axum::http::header::CACHE_CONTROL, "no-store".into()), (axum::http::header::SET_COOKIE, set_cookie(&origin, &format!("{flow_name}_{flow}"), &verifier, FLOW_SECS))]).into_response())
}

/// Whether `pass` lets its browser load what `front` serves, at `now`.
fn valid_for(pass: &Pass, front: &FrontHost, now: i64) -> bool { pass.exp > now && pass.app == front.app && pass.release == front.release }

/// `GET /__cori/pass?code=…&state=…` on a front host: the code the console got (`pass_code`) traded, with the verifier its
/// browser kept, for this host's pass; then back to the page it asked for.
pub(crate) async fn pass_return(InRealm(st): InRealm, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    let Some(axum::Extension(front)) = front else { return Err(ApiError(StatusCode::NOT_FOUND, "an app's front host gives its passes".into())) };
    if !st.cfg.fronts_gated() { return Err(ApiError(StatusCode::NOT_FOUND, "this server's fronts are open to anyone: there is no pass to take".into())); }
    let origin = host_origin(&st, &front);
    let (pass_name, flow_name) = cookie_names(&origin);
    let name = st.brand.name.clone();
    // shown where the front is (another tool's frame, say), as the front's own pages are
    let csp = format!("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; {}", front_frame_ancestors(&st.cfg));
    let problem = |why: &str, back: &str| -> Res {
        let body = format!("<p>{}</p><p><a href=\"{}\">Open it again</a></p>", html_escape(why), html_escape(back));
        Ok((StatusCode::BAD_REQUEST, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()), (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
            (axum::http::header::CONTENT_SECURITY_POLICY, csp.clone())],
            notice_page(&name, &front.app, &format!("{} did not open", front.app), &body)).into_response())
    };
    let Some((flow, back)) = q.get("state").and_then(|s| read_state(s)) else { return problem("This sign-in came back without what it left with.", "/") };
    let flow_cookie = format!("{flow_name}_{flow}");
    let now = chrono::Utc::now().timestamp();
    let back_with = |cookies: Vec<String>| -> Res {
        let mut r = (StatusCode::SEE_OTHER, [(axum::http::header::LOCATION, back.clone()), (axum::http::header::CACHE_CONTROL, "no-store".into())]).into_response();
        for c in cookies { if let Ok(v) = axum::http::HeaderValue::from_str(&c) { r.headers_mut().append(axum::http::header::SET_COOKIE, v); } }
        Ok(r)
    };
    let done = set_cookie(&origin, &flow_cookie, "", 0);
    // a pass this browser got meanwhile (another tab's sign-in): back to the page at once
    if cookie(&headers, pass_name).and_then(|v| open(&pass_key(&st), &origin, v)).is_some_and(|p| valid_for(&p, &front, now)) { return back_with(vec![done]); }
    let Some(verifier) = cookie(&headers, &flow_cookie).map(String::from) else {
        return problem("This browser came back without the sign-in it started here, which lasts ten minutes and needs the site's cookies.", &back);
    };
    let Some(p) = take_code(&st, q.get("code").map(String::as_str).unwrap_or("")).await? else { return problem("This sign-in expired.", &back) };
    let ours = p["kind"] == "pass" && p["origin"].as_str() == Some(origin.as_str()) && p["app"].as_str() == Some(front.app.as_str()) && p["release"].as_i64() == front.release;
    if !ours || !pkce_ok(&verifier, p["challenge"].as_str().unwrap_or("")) { return problem("This sign-in was for another page or another browser.", &back); }
    let sub = p["sub"].as_str().unwrap_or_default().to_string();
    if sub.is_empty() { return problem("This sign-in names nobody.", &back); }
    let pass = seal(&pass_key(&st), &origin, &Pass { sub, app: front.app.clone(), release: front.release, iat: now, exp: now + PASS_SECS })?;
    back_with(vec![set_cookie(&origin, pass_name, &pass, PASS_SECS), done])
}

/// `POST /v1/apps/{app}/pass-codes` `{ code_challenge, redirect_uri, release? }`: a code a front host trades for its pass
/// (`pass_return`), for the person signed in to the console, when they may open what that host serves. The live front: its
/// builders (owner, builders, admins), and whoever holds one of the app's roles (one of `[front] allow.roles`, when it
/// lists some), a guest it is shared with included. A release (`v<n>`): whoever may open a release (`preview_code`). Never
/// an agent session, nor a front's own token (the guard), nor from a front's host.
#[derive(serde::Deserialize)]
pub(crate) struct PassAsk { code_challenge: String, redirect_uri: String, #[serde(default)] release: Option<String> }

pub(crate) async fn pass_code(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller, Json(ask): Json<PassAsk>) -> Res {
    if front.is_some() { return Err(ApiError(StatusCode::NOT_FOUND, "the console's host gives a front's passes".into())); }
    if !st.cfg.fronts_gated() { return Err(ApiError(StatusCode::NOT_FOUND, "this server's fronts are open to anyone: there is no pass to give".into())); }
    require_human(&who, "opening an app")?;
    let rec = require_app(&st, &app).await?;
    let (origin, release) = match ask.release.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => {
            if !(is_owner_or_builder(&who, &rec) || is_approver(&st, &who) || who.is_builder()) {
                return Err(ApiError(StatusCode::FORBIDDEN, format!("opening a release of `{app}` needs its owner, a builder, an approver or an admin (you are `{}`)", who.sub)));
            }
            let (version, _, _) = resolve_version(&st, &app, r).await?;
            let seq: i64 = sqlx::query_scalar::<_, Option<i64>>("SELECT seq FROM versions WHERE hash = $1 AND app = $2").bind(&version).bind(&app).fetch_optional(&st.db).await?.flatten()
                .ok_or(ApiError(StatusCode::NOT_FOUND, format!("version {r} of `{app}` has no release number")))?;
            (st.release_origin(&app, seq), Some(seq))
        }
        None => {
            let (_, _, cori_toml) = promoted_version(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("`{app}` has nothing live yet: its builders open its releases")))?;
            if !is_owner_or_builder(&who, &rec) {
                let roles = member_roles(&st, &in_app_asking(&st, who.clone(), &app, rec.client_id.as_deref()).await, &rec).await?;
                let allow = front_allow_roles(&cori_toml);
                let may = if allow.is_empty() { !roles.is_empty() } else { roles.iter().any(|r| allow.contains(r)) };
                if !may {
                    let whom = if allow.is_empty() { "the people who hold one of its roles".to_string() } else { format!("the people who hold one of the roles {}", allow.join(", ")) };
                    return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}` opens for {whom}: ask its owner for access")));
                }
            }
            (st.front_origin(&app), None)
        }
    };
    let redirect = format!("{origin}{PASS_ROUTE}");
    if ask.redirect_uri != redirect { return Err(ApiError(StatusCode::BAD_REQUEST, "redirect_uri is not that front's".into())); }
    if !challenge_ok(&ask.code_challenge) { return Err(ApiError(StatusCode::BAD_REQUEST, "an S256 code_challenge is needed".into())); }
    let payload = json!({ "kind": "pass", "app": app, "release": release, "origin": origin, "sub": who.sub, "challenge": ask.code_challenge });
    let code = store_code(&st, &payload).await?;
    db::audit(&st.db, &who.sub, "front.pass", &match release { Some(n) => format!("{app}@v{n}"), None => app.clone() }, json!({})).await?;
    Ok(Json(json!({ "code": code, "redirect_uri": redirect })).into_response())
}

/// `[front] allow.roles` of a version's cori.toml: the roles its front is for, none when it does not say.
fn front_allow_roles(cori_toml: &str) -> Vec<String> {
    let t: toml::Value = toml::from_str(cori_toml).unwrap_or(toml::Value::Table(Default::default()));
    t.get("front").and_then(|f| f.get("allow")).and_then(|a| a.get("roles")).and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs { h.append(*k, axum::http::HeaderValue::from_str(v).unwrap()); }
        h
    }

    #[test]
    fn a_pass_opens_on_its_own_host_for_what_it_names_until_it_ends() {
        let (key, other_key) = ([7u8; 32], [8u8; 32]);
        let (desk, crm) = ("https://desk--acme.cori.example", "https://crm--acme.cori.example");
        let pass = Pass { sub: "alice".into(), app: "desk".into(), release: None, iat: 100, exp: 200 };
        let sealed = seal(&key, desk, &pass).map_err(|e| e.1).unwrap();
        assert!(sealed.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)), "a cookie's value as it is: {sealed}");
        assert_eq!(open(&key, desk, &sealed), Some(Pass { sub: "alice".into(), app: "desk".into(), release: None, iat: 100, exp: 200 }));
        assert_eq!(open(&key, crm, &sealed), None, "another host");
        assert_eq!(open(&other_key, desk, &sealed), None, "another realm's key");
        let mut tampered = sealed.clone().into_bytes();
        let last = tampered.len() - 2;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(open(&key, desk, &String::from_utf8(tampered).unwrap()), None, "changed");
        let live = FrontHost { app: "desk".into(), realm: None, release: None };
        assert!(valid_for(&pass, &live, 150));
        assert!(!valid_for(&pass, &live, 200), "ended");
        assert!(!valid_for(&pass, &FrontHost { release: Some(3), ..live.clone() }, 150), "a release of it");
        assert!(!valid_for(&pass, &FrontHost { app: "crm".into(), ..live }, 150), "another app");
    }

    #[test]
    fn a_page_load_goes_to_sign_in_and_a_file_is_refused() {
        assert!(is_page_load(&headers(&[("sec-fetch-mode", "navigate"), ("accept", "*/*")])));
        for mode in ["cors", "no-cors", "same-origin"] { assert!(!is_page_load(&headers(&[("sec-fetch-mode", mode), ("accept", "text/html")])), "{mode}"); }
        // without fetch metadata (an older browser, a tool): what it accepts
        assert!(is_page_load(&headers(&[("accept", "text/html,application/xhtml+xml")])));
        assert!(!is_page_load(&headers(&[("accept", "application/json")])));
        assert!(!is_page_load(&HeaderMap::new()));
    }

    #[test]
    fn a_cookie_is_read_among_the_others() {
        let h = headers(&[("cookie", "a=1; cori_pass=x.y-z; b=2"), ("cookie", "cori_flow_0123456789abcdef=v")]);
        assert_eq!(cookie(&h, "cori_pass"), Some("x.y-z"));
        assert_eq!(cookie(&h, "cori_flow_0123456789abcdef"), Some("v"));
        assert_eq!(cookie(&h, "cori"), None);
        assert_eq!(cookie(&HeaderMap::new(), "cori_pass"), None);
    }

    #[test]
    fn its_cookies_are_the_hosts_own() {
        assert_eq!(cookie_names("https://desk--acme.cori.example"), ("__Host-cori_pass", "__Host-cori_flow"));
        assert_eq!(set_cookie("https://desk--acme.cori.example", "__Host-cori_pass", "v", 60), "__Host-cori_pass=v; Path=/; Max-Age=60; HttpOnly; SameSite=Lax; Secure");
        // a laptop's fronts over http: no prefix, which would want Secure
        assert_eq!(cookie_names("http://desk.localhost:8187"), ("cori_pass", "cori_flow"));
        assert_eq!(set_cookie("http://desk.localhost:8187", "cori_pass", "", 0), "cori_pass=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax");
    }

    #[test]
    fn the_page_to_come_back_to_stays_on_the_host() {
        let flow = "0123456789abcdef";
        let round = |back: &str| read_state(&flow_state(flow, back)).map(|(_, b)| b);
        assert_eq!(read_state(&flow_state(flow, "/tickets/42?tab=open")), Some((flow.to_string(), "/tickets/42?tab=open".to_string())));
        for elsewhere in ["//evil.example/x", "/\\evil.example", "https://evil.example/", "evil", "", "/a\nb"] { assert_eq!(round(elsewhere).as_deref(), Some("/"), "{elsewhere:?}"); }
        let enc = |s: &str| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, s);
        assert_eq!(read_state(&format!("{flow}.not base64")).map(|(_, b)| b).as_deref(), Some("/"));
        for bad in ["", "no-dot", &format!("short.{}", enc("/")), &format!("0123456789abcdeg.{}", enc("/"))] { assert_eq!(read_state(bad), None, "{bad}"); }
    }

    #[test]
    fn the_path_asked_is_the_one_at_the_root_of_the_host() {
        let live = FrontHost { app: "desk".into(), realm: None, release: None };
        let at = |f: &FrontHost, u: &str| asked_path(f, &u.parse().unwrap());
        assert_eq!(at(&live, "/apps/desk/"), "/");
        assert_eq!(at(&live, "/apps/desk"), "/");
        assert_eq!(at(&live, "/apps/desk/tickets/42?tab=open"), "/tickets/42?tab=open");
        let v12 = FrontHost { release: Some(12), ..live.clone() };
        assert_eq!(at(&v12, "/apps/desk/@v12/"), "/");
        assert_eq!(at(&v12, "/apps/desk/@v12/x?y=1"), "/x?y=1");
        assert_eq!(at(&live, "/apps/desks/x"), "/", "another app's prefix");
    }

    #[test]
    fn a_front_says_whom_it_is_for() {
        assert_eq!(front_allow_roles("[front]\nallow = { roles = [\"support_agent\", \"support_lead\"] }\n"), vec!["support_agent", "support_lead"]);
        assert!(front_allow_roles("[app]\nid = \"desk\"\n").is_empty());
        assert!(front_allow_roles("not toml [").is_empty());
    }
}

// SPDX-License-Identifier: Apache-2.0
//! An app's front and its previews, its generated client, the dev config, and the console page.
use super::*;

/// A release's front (`/apps/{app}/@{hash}/…`, `hash` a release number `v12` or a hash prefix): served on its own host when
/// fronts have hosts (`front_hosts` placed the path there), which a link of the live host or the server's own is sent on
/// to; else under the server's origin.
pub(crate) async fn preview_index(InRealm(st): InRealm, Path((app, hash)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, uri: axum::http::Uri) -> Res { preview(&st, &app, &hash, "index.html", front.map(|f| f.0), &headers, &uri).await }

pub(crate) async fn preview_file(InRealm(st): InRealm, Path((app, hash, path)): Path<(String, String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, uri: axum::http::Uri) -> Res { preview(&st, &app, &hash, &path, front.map(|f| f.0), &headers, &uri).await }

async fn preview(st: &AppState, app: &str, hash: &str, path: &str, front: Option<FrontHost>, headers: &HeaderMap, uri: &axum::http::Uri) -> Res {
    if st.cfg.fronts_url.is_some() && front.as_ref().and_then(|f| f.release).is_none() {
        let (h, _, _) = resolve_version(st, app, hash).await?;
        let seq: Option<i64> = sqlx::query_scalar("SELECT seq FROM versions WHERE hash = $1 AND app = $2").bind(&h).bind(app).fetch_optional(&st.db).await?.flatten();
        let seq = seq.ok_or(ApiError(StatusCode::NOT_FOUND, format!("version {hash} of `{app}` has no release number")))?;
        let rest = uri.path().splitn(4, '/').nth(3).and_then(|r| r.split_once('/')).map(|(_, r)| r).unwrap_or("");
        let to = format!("{}{rest}{}", st.release_url(app, seq), uri.query().map(|q| format!("?{q}")).unwrap_or_default());
        return Ok((StatusCode::PERMANENT_REDIRECT, [(axum::http::header::LOCATION, to)]).into_response());
    }
    // a release's own host: its files for those who may open the release (`passes`)
    if let Some(r) = admit(st, front.as_ref(), headers, uri) { return Ok(r); }
    serve_front_of(st, app, Some(hash), path, front.as_ref()).await
}

pub(crate) async fn client_ts(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, false, "the workflow schemas").await?;
    let (_, wfs, _) = promoted_version(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("app `{app}` has no promoted version")))?;
    Ok(([("content-type", "text/typescript; charset=utf-8")], crate::clientgen::client_ts(&app, &wfs)).into_response())
}

/// Hosted front: `dist/` of the promoted bundle, SPA fallback to index.html, `/__cori/config.json` injected. On a front's
/// own host (`front`) it is served at the root (`front_hosts`).
/// With `CORI_FRONTS_PRIVATE`, a front host first wants a pass of its own (`passes::admit`); the callback, where the app's
/// own sign-in ends, never does.
pub(crate) async fn front_index(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, uri: axum::http::Uri) -> Res {
    if let Some(r) = admit(&st, front.as_ref().map(|f| &f.0), &headers, &uri) { return Ok(r); }
    serve_front(&st, &app, "index.html", front.map(|f| f.0).as_ref()).await
}

pub(crate) async fn front_file(InRealm(st): InRealm, Path((app, path)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, headers: HeaderMap, uri: axum::http::Uri) -> Res {
    if path == "callback" { return front_callback(&app, uri.query().unwrap_or(""), front.is_some()); }
    if let Some(r) = admit(&st, front.as_ref().map(|f| &f.0), &headers, &uri) { return Ok(r); }
    serve_front(&st, &app, &path, front.map(|f| f.0).as_ref()).await
}

/// `GET /apps/{app}/callback`: the fixed OIDC redirect of an app with its own client. The front put the page to return to
/// in `state` (`<nonce>.<base64url path>`); this sends the browser there with the issuer's query (`code`, `state` or
/// `error`), where `@cori-do/client` finishes the sign-in. Only paths of this app's front are accepted (no open redirect):
/// on its own host (`rooted`) any path of that host, else those under `/apps/<app>/`.
pub(crate) fn front_callback(app: &str, query: &str, rooted: bool) -> Res {
    let state = query.split('&').find_map(|kv| kv.strip_prefix("state=")).map(|v| urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_default()).unwrap_or_default();
    let back = state.split_once('.').and_then(|(_, b)| base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, b).ok()).and_then(|b| String::from_utf8(b).ok());
    let back = callback_back(app, back, rooted);
    let sep = if back.contains('?') { '&' } else { '?' };
    Ok((StatusCode::SEE_OTHER, [("location", format!("{back}{sep}{query}")), ("cache-control", "no-store".to_string())]).into_response())
}

/// The page a sign-in returns to (`front_callback`): a path of this app's front, never `//…` (another host's address), `\`
/// or `..`. On the front's own host an older state's `/apps/<app>/…` moves to the root, and a preview's `/apps/<app>/@…`
/// stays.
fn callback_back(app: &str, back: Option<String>, rooted: bool) -> String {
    let root = format!("/apps/{app}/");
    let back = back.filter(|b| b.starts_with('/') && !b.contains("//") && !b.contains('\\') && !b.contains("/.."));
    if !rooted { return match back { Some(b) if b == root.trim_end_matches('/') || b.starts_with(&root) => b, _ => root }; }
    let Some(b) = back else { return "/".into() };
    match b.strip_prefix(root.trim_end_matches('/')) {
        Some(rest) if rest.starts_with("/@") => b,
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') => format!("/{}", rest.trim_start_matches('/')),
        _ => b,
    }
}

pub(crate) async fn serve_front(st: &AppState, app: &str, path: &str, host: Option<&FrontHost>) -> Res { serve_front_of(st, app, None, path, host).await }

/// A front's file: the promoted release's, or `version`'s; `host` the front's own host when fronts have hosts (it is then
/// served at that host's root, and calls the API there).
pub(crate) async fn serve_front_of(st: &AppState, app: &str, version: Option<&str>, path: &str, host: Option<&FrontHost>) -> Res {
    let (hash, wfs, cori_toml) = match version { Some(v) => resolve_version(st, app, v).await?, None => promoted_version(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("app `{app}` has no promoted version: open a version's preview (`cori releases {app}`) or promote one")))? };
    // a gated front's files are each person's (`passes`): a shared cache (a CDN in front of the fronts) must not keep them
    // for whoever asks next
    let private = host.is_some() && st.cfg.fronts_gated();
    if path == "__cori/config.json" {
        // the front calls the API on the origin it is served from (its own host with `CORI_FRONTS_URL`): same-origin requests
        // a release's host calls the API on its own origin, where its runs are its release's
        let base = match host.and_then(|h| h.release) { Some(seq) => st.release_origin(app, seq), None => st.front_origin(app) };
        let mut v = json!({ "app": app, "baseUrl": base, "release": host.and_then(|h| h.release).map(|n| format!("v{n}")), "portal_url": st.portal_url(), "auth": match st.cfg.auth { crate::auth::AuthMode::Off => "off", crate::auth::AuthMode::Dev => "dev", _ => "builtin" }, "version": if version.is_some() { Some(hash.clone()) } else { None }, "preview": version.is_some() });
        // dev mode only: what the front's dev widget needs to impersonate a role (the same `dev:<sub>:<roles>` tokens the console uses)
        if matches!(st.cfg.auth, crate::auth::AuthMode::Dev) { v["dev"] = dev_config(st, app, &wfs, &cori_toml).await?; }
        // builtin: where the front signs its user in (`@cori-do/client` login): a release's host is its own issuer, its person
        // known from the console (`api::sessions`); the live front signs in as the app's own client
        if let (crate::auth::AuthMode::Builtin { .. }, Some(seq)) = (&st.cfg.auth, host.and_then(|h| h.release)) {
            v["oidc"] = preview_oidc(st, app, seq);
        } else if let crate::auth::AuthMode::Builtin { issuer, .. } = &st.cfg.auth {
            let client = match app_record(st, app).await?.and_then(|r| r.client_id) {
                Some(c) => c,
                None if client_pending(st, app).await?.is_some() => return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, format!("app `{app}`'s sign-in client is being set up by the organization's identity provider"))),
                None => return Err(ApiError(StatusCode::CONFLICT, format!("app `{app}` has no sign-in client: `cori apps sync-client {app}`"))),
            };
            v["oidc"] = json!({ "issuer": issuer, "client_id": client, "redirect_uri": crate::client_hook::callback_url(st, app) });
            // the resource the sign-in names (`CORI_AUTH_RESOURCE`), and the deployment's own scopes (`CORI_AUTH_SCOPES`),
            // which the client asks for besides its own
            if let Some(r) = st.cfg.sign_in_resource() { v["oidc"]["resource"] = json!(r); }
            if !st.cfg.auth_scopes.is_empty() { v["oidc"]["scopes"] = json!(st.cfg.auth_scopes); }
        }
        let mut r = Json(v).into_response();
        if private { r.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("private, no-cache")); }
        return Ok(r);
    }
    let is_dev = matches!(st.cfg.auth, crate::auth::AuthMode::Dev);
    if path == "__cori/dev-widget.js" {
        if !is_dev { return Err(ApiError(StatusCode::NOT_FOUND, "the dev widget only exists under dev auth".into())); }
        return Ok(([("content-type", "text/javascript; charset=utf-8"), ("cache-control", "no-cache")], DEV_WIDGET_JS).into_response());
    }
    let dist = st.bundles.dir(st.db.realm(), &hash).await?.join("dist");
    let mut file = below_dist(&dist, path).ok_or(ApiError(StatusCode::BAD_REQUEST, "bad path".into()))?;
    if !file.is_file() { file = dist.join("index.html"); }
    // a front whose sign-in client the organization is still creating cannot sign anyone in: say so, rather than fail later
    if file.ends_with("index.html") && matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) && host.and_then(|h| h.release).is_none() && let Some(note) = client_pending(st, app).await? {
        let name = app_record(st, app).await?.map(|r| r.name).unwrap_or_else(|| app.to_string());
        return Ok(([("content-type", "text/html; charset=utf-8".to_string()), ("cache-control", "no-store".to_string())], front_headers(&st.cfg), pending_page(&st.brand.name, &name, &note)).into_response());
    }
    let mut bytes = tokio::fs::read(&file).await.map_err(|_| ApiError(StatusCode::NOT_FOUND, "this app has no front (no dist/ in the bundle)".into()))?;
    // dev auth: every hosted front gets the dev widget, whatever it is built with (an absolute src: the SPA fallback serves index.html under any path)
    if is_dev && file.ends_with("index.html") {
        let src = match version { _ if host.is_some() => "/__cori/dev-widget.js".to_string(), Some(v) => format!("/apps/{app}/@{v}/__cori/dev-widget.js"), None => format!("/apps/{app}/__cori/dev-widget.js") };
        let tag = format!("<script src=\"{src}\" data-cori-app=\"{app}\" data-cori-dev-widget></script>");
        let html = String::from_utf8_lossy(&bytes).into_owned();
        bytes = match html.rfind("</body>") { Some(i) => format!("{}{tag}\n{}", &html[..i], &html[i..]), None => format!("{html}\n{tag}") }.into_bytes();
    }
    let mime = mime_guess::from_path(&file).first_or_octet_stream().to_string();
    // only a content-hashed name (`app-3f9c2b1a.js`, as bundlers emit) may be cached for good: a plain name is reused across versions
    let hashed = file.file_name().and_then(|n| n.to_str()).map(|n| n.split(['-', '.']).any(|p| p.len() >= 8 && p.chars().all(|c| c.is_ascii_hexdigit()))).unwrap_or(false);
    let cache = match (hashed && !file.ends_with("index.html"), private) {
        (true, false) => "public, max-age=31536000, immutable", (true, true) => "private, max-age=31536000, immutable",
        (false, false) => "no-cache", (false, true) => "private, no-cache",
    };
    let mut r = ([("content-type", mime), ("cache-control", cache.to_string())], front_headers(&st.cfg), bytes).into_response();
    // a release that is not (or no longer) what the app serves is for its builders and approvers: never in a search engine
    if version.is_some() { r.headers_mut().insert("x-robots-tag", axum::http::HeaderValue::from_static("noindex")); }
    Ok(r)
}

/// A file of a front's `dist/` named by a request's path: relative, with plain components only. A path the route captured
/// as `/etc/passwd` (`/apps/x//etc/passwd`) would make `join` leave `dist/` altogether, and `..` would climb out of it.
fn below_dist(dist: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    let rel = std::path::Path::new(path);
    let plain = !path.is_empty() && !path.contains('\\') && rel.components().all(|c| matches!(c, std::path::Component::Normal(_)));
    plain.then(|| dist.join(rel))
}

/// The identity provider's last word while `app`'s sign-in client is pending (`client_hook`): None once it has one.
async fn client_pending(st: &AppState, app: &str) -> Result<Option<String>, ApiError> {
    let r = sqlx::query("SELECT client_id, client_status, client_note FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await?;
    Ok(r.filter(|r| r.get::<Option<String>, _>("client_id").is_none() && r.get::<Option<String>, _>("client_status").as_deref() == Some("pending")).map(|r| r.get::<Option<String>, _>("client_note").unwrap_or_default()))
}

/// The page a front shows while its sign-in client is pending: no script, nothing loaded from elsewhere.
fn pending_page(brand: &str, app: &str, note: &str) -> String {
    let note = if note.is_empty() || note.starts_with("http ") { String::new() } else { format!("<p class=\"note\">{}</p>", html_escape(note)) };
    notice_page(brand, app, &format!("{app} is almost ready"), &format!("<p>Its sign-in is being set up by your organization's identity team. This page works once they are done: try again in a little while.</p>{note}"))
}

/// Text made safe inside HTML, attributes included.
pub(crate) fn html_escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

/// A page of the server's own on a front's host, when it cannot show the front: `heading` (text), then `body` (HTML the
/// caller escaped). No script, nothing loaded from elsewhere.
pub(crate) fn notice_page(brand: &str, app: &str, heading: &str, body: &str) -> String {
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{app} · {brand}</title>
<style>body{{font-family:system-ui,sans-serif;margin:0;min-height:100vh;display:grid;place-items:center;background:#f7f7f8;color:#1f2328}}main{{max-width:32rem;padding:2rem}}h1{{font-size:1.25rem}}p{{line-height:1.5}}a{{color:inherit}}.note{{color:#57606a;font-size:.9rem}}@media (prefers-color-scheme:dark){{body{{background:#0d1117;color:#e6edf3}}.note{{color:#8b949e}}}}</style></head>
<body><main><h1>{heading}</h1>{body}<p class="note">{brand}</p></main></body></html>"#, app = html_escape(app), brand = html_escape(brand), heading = html_escape(heading))
}

/// The `dev` block of `/__cori/config.json` under dev auth: every role the app knows (declared on the app, allowed on
/// an endpoint, or allowed on the front), the workflows with their endpoint allow lists, the front allow list and the
/// connectors answered from fixtures. Unauthenticated, like the rest of the file; only served when the auth mode is `dev`.
pub(crate) async fn dev_config(st: &AppState, app: &str, wfs: &[bundles::WorkflowEntry], cori_toml: &str) -> Result<Value, ApiError> {
    let strings = |v: Option<&Value>| v.and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
    let toml_v: toml::Value = toml::from_str(cori_toml).unwrap_or(toml::Value::Table(Default::default()));
    let front_roles: Vec<String> = toml_v.get("front").and_then(|f| f.get("allow")).and_then(|a| a.get("roles")).and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let rec = app_record(st, app).await?;
    // who may open a preview (`/apps/{app}/@{hash}/`): the widget defaults to the owner there, keeping the front's roles
    let (owner, builders) = rec.as_ref().map(|r| (r.owner.clone(), r.builders.clone())).unwrap_or_default();
    let mut roles: Vec<String> = rec.map(|r| r.roles).unwrap_or_default();
    let workflows: Vec<Value> = wfs.iter().map(|w| {
        let ep = w.compiled.manifest.endpoint.as_ref();
        let allow = strings(ep.and_then(|e| e.allow.as_ref()).and_then(|a| a.get("roles")));
        roles.extend(allow.iter().cloned());
        json!({ "id": w.id, "description": w.compiled.manifest.description.trim(), "endpoint": ep.map(|e| json!({ "mode": e.mode, "auth": e.auth, "roles": allow })), "schedule": w.compiled.manifest.schedule })
    }).collect();
    roles.extend(front_roles.iter().cloned());
    let personas = dev_personas(&toml_v);
    for p in &personas { roles.extend(strings(p.get("roles"))); }
    roles.sort(); roles.dedup();
    // store connectors are served by the store database when one is configured; their fixture is not in play
    let mocks: Vec<String> = if !st.cfg.dev_mocks { vec![] } else { AppConfig::from_toml(cori_toml, std::path::Path::new(".")).map(|c| c.connectors.iter().filter(|(n, c)| c.dev.as_ref().map(|d| d.mock.is_some()).unwrap_or(false) && !st.cfg.no_mocks.contains(n) && !(c.kind == "store" && st.cfg.store_database_url.is_some())).map(|(n, _)| n.clone()).collect()).unwrap_or_default() };
    Ok(json!({ "roles": roles, "front_roles": front_roles, "personas": personas, "owner": owner, "builders": builders, "workflows": workflows, "mocks": mocks, "hint": "Authorization: Bearer dev:<sub>:<role,role>" }))
}

/// `[[dev.personas]] sub, roles, name?, title?` of cori.toml: the users the dev widget (and a front's own switcher)
/// offers under dev auth, matching the store's seed. The first one is the default identity of the app.
pub(crate) fn dev_personas(toml_v: &toml::Value) -> Vec<Value> {
    let list = toml_v.get("dev").and_then(|d| d.get("personas")).and_then(|p| p.as_array()).cloned().unwrap_or_default();
    list.iter().filter_map(|p| {
        let sub = p.get("sub")?.as_str()?.trim();
        if sub.is_empty() { return None; }
        let roles: Vec<&str> = p.get("roles").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|r| r.as_str()).collect()).unwrap_or_default();
        let text = |k: &str| p.get(k).and_then(|v| v.as_str()).map(String::from);
        Some(json!({ "sub": sub, "roles": roles, "name": text("name").unwrap_or_else(|| sub.to_string()), "title": text("title") }))
    }).collect()
}

/// The console, with the deployment's brand written in (`brand`). Its policy runs its own inline scripts only (by hash),
/// talks to this server and the issuer, and lets no page frame it.
pub(crate) async fn console_page(InRealm(st): InRealm) -> Res {
    let issuer = match &st.cfg.auth { crate::auth::AuthMode::Builtin { issuer, .. } => reqwest::Url::parse(issuer).ok().map(|u| format!(" {}", u.origin().ascii_serialization())).unwrap_or_default(), _ => String::new() };
    let csp = format!("default-src 'self'; script-src {}; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self'{issuer}; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'", st.brand.console_scripts);
    Ok(([("content-type", "text/html; charset=utf-8".to_string()), ("content-security-policy", csp), ("x-frame-options", "DENY".into()), ("referrer-policy", "no-referrer".into())], st.brand.console_html.clone()).into_response())
}

/// The headers of every page and file of a front: framed by itself and the hosts of `CORI_FRAME_ANCESTORS` only (an
/// MCP host's preview widget), no `<base>` or plugin, and the origin alone as referrer elsewhere. What the front's own
/// code loads and calls is its builders' choice.
fn front_headers(cfg: &crate::ServerConfig) -> [(&'static str, String); 2] {
    [("content-security-policy", format!("{}; base-uri 'self'; object-src 'none'", front_frame_ancestors(cfg))), ("referrer-policy", "strict-origin-when-cross-origin".into())]
}

/// Who may frame a front host's pages: itself and the hosts of `CORI_FRAME_ANCESTORS`.
pub(crate) fn front_frame_ancestors(cfg: &crate::ServerConfig) -> String {
    let extra: String = cfg.frame_ancestors.iter().map(|a| format!(" {a}")).collect();
    format!("frame-ancestors 'self'{extra}")
}

/// The console: a React app in `console/`, built (`npm run build`) into this one self-contained page, committed so the
/// crate builds without Node.
pub(crate) const CONSOLE_HTML: &str = include_str!("../../console/dist/index.html");

/// The apps portal (`api::portal`): the console's second page, built the same way (`console/portal.html`).
pub(crate) const PORTAL_HTML: &str = include_str!("../../console/dist-portal/portal.html");

/// Injected into every hosted `index.html` under dev auth; served at `<front>/__cori/dev-widget.js`.
pub(crate) const DEV_WIDGET_JS: &str = include_str!("../../assets/dev-widget.js");

#[cfg(test)]
mod dev_tests {
    use super::*;

    #[test]
    fn a_sign_in_returns_to_a_page_of_the_app_only() {
        let at = |b: &str, rooted| callback_back("desk", Some(b.to_string()), rooted);
        // on the front's own host: any path there, an older state moved to the root, a preview's kept
        assert_eq!(at("/tickets/42?tab=notes", true), "/tickets/42?tab=notes");
        assert_eq!(at("/apps/desk/tickets/42", true), "/tickets/42");
        assert_eq!(at("/apps/desk", true), "/");
        assert_eq!(at("/apps/desk?x=1", true), "/?x=1");
        assert_eq!(at("/apps/desk/@v12/new", true), "/apps/desk/@v12/new");
        for bad in ["//evil.example/x", "https://evil.example/", "/a/../../x", "/a\\b", "evil"] { assert_eq!(at(bad, true), "/", "{bad}"); }
        assert_eq!(callback_back("desk", None, true), "/");
        // on the server's own origin: the app's pages only
        assert_eq!(at("/apps/desk/tickets/42", false), "/apps/desk/tickets/42");
        assert_eq!(at("/apps/crm/", false), "/apps/desk/");
        assert_eq!(at("/tickets/42", false), "/apps/desk/");
    }

    #[test]
    fn a_fronts_files_are_read_below_its_dist_only() {
        let dist = std::path::Path::new("/data/bundles/h/dist");
        assert_eq!(below_dist(dist, "index.html"), Some(dist.join("index.html")));
        assert_eq!(below_dist(dist, "assets/app-3f9c2b1a.js"), Some(dist.join("assets/app-3f9c2b1a.js")));
        assert_eq!(below_dist(dist, "tickets/42"), Some(dist.join("tickets/42")), "a route of the app: the SPA fallback answers");
        for bad in ["/etc/hosts", "/proc/self/environ", "../cori.toml", "assets/../../x", "./index.html", "a\\b", ""] { assert_eq!(below_dist(dist, bad), None, "{bad}"); }
    }
    #[test]
    fn dev_personas_from_cori_toml() {
        let t: toml::Value = toml::from_str("[[dev.personas]]\nsub = \"alice\"\nroles = [\"member\"]\nname = \"Alice Martin\"\n\n[[dev.personas]]\nsub = \"maya\"\nroles = [\"manager\", \"member\"]\n\n[[dev.personas]]\nroles = [\"x\"]\n").unwrap();
        let p = dev_personas(&t);
        assert_eq!(p.len(), 2);                                        // an entry without a sub is dropped
        assert_eq!(p[0], json!({ "sub": "alice", "roles": ["member"], "name": "Alice Martin", "title": null }));
        assert_eq!(p[1]["name"], "maya");                                  // the name defaults to the sub
        assert!(dev_personas(&toml::from_str("[app]\nid = \"a\"").unwrap()).is_empty());
    }
}

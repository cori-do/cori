// SPDX-License-Identifier: Apache-2.0
//! The organization's apps portal: where its people find the apps they may use. One page of Cori's own on a host of its
//! own (`CORI_PORTAL_URL`, or a realm's label under the fronts' domain, `Tenancy::portal`), which signs its person in through
//! the console (`api::sessions`) and lists `GET /v1/me/apps`; each app's icon is served here from its live release.
use super::*;

/// `GET /` of a portal's host (`front_hosts` places it at `/__cori/portal`): the page, with the deployment's brand. Its
/// policy runs its own inline scripts only, talks to its own host, and lets no page frame it.
pub(crate) async fn portal_page(InRealm(st): InRealm, portal: Option<axum::Extension<PortalHost>>) -> Res {
    if portal.is_none() { return Err(ApiError(StatusCode::NOT_FOUND, "the apps portal is served on its own host".into())); }
    let csp = format!("default-src 'self'; script-src {}; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'", st.brand.portal_scripts);
    Ok(([("content-type", "text/html; charset=utf-8".to_string()), ("content-security-policy", csp), ("x-frame-options", "DENY".into()), ("referrer-policy", "no-referrer".into()), ("x-robots-tag", "noindex".into()), ("cache-control", "no-cache".into())], st.brand.portal_html.clone()).into_response())
}

/// The icons an app's front may carry, in the order they are looked for in its `dist/`.
const ICONS: [(&str, &str); 3] = [("favicon.svg", "image/svg+xml"), ("favicon.png", "image/png"), ("favicon.ico", "image/x-icon")];
const ICON_MAX_BYTES: u64 = 256 * 1024;

/// `GET /__cori/icons/{app}` on a portal's host: the live release's favicon, else 404 (the page then draws the app's
/// initial). Its builders wrote it: an SVG opened by itself runs nothing (`sandbox`).
pub(crate) async fn portal_icon(InRealm(st): InRealm, Path(app): Path<String>, portal: Option<axum::Extension<PortalHost>>) -> Res {
    if portal.is_none() { return Err(ApiError(StatusCode::NOT_FOUND, "icons are served on the apps portal".into())); }
    let Some((hash, _, _)) = promoted_version(&st, &app).await? else { return Err(ApiError(StatusCode::NOT_FOUND, "no live release".into())) };
    let dist = st.bundles.dir(st.db.realm(), &hash).await?.join("dist");
    for (name, mime) in ICONS {
        let f = dist.join(name);
        if !std::fs::metadata(&f).map(|m| m.is_file() && m.len() <= ICON_MAX_BYTES).unwrap_or(false) { continue; }
        let bytes = tokio::fs::read(&f).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(([("content-type", mime), ("cache-control", "public, max-age=300"), ("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; sandbox")], bytes).into_response());
    }
    Err(ApiError(StatusCode::NOT_FOUND, "this app's front has no icon".into()))
}

/// An app's description for the portal (`[app] description` of a release's cori.toml): its first line, cut at
/// `cori_compiler::DESCRIPTION_MAX` characters.
pub(crate) fn app_description(cori_toml: &str) -> Option<String> {
    let t: toml::Value = toml::from_str(cori_toml).ok()?;
    let d = t.get("app")?.get("description")?.as_str()?.lines().next()?.trim();
    (!d.is_empty()).then(|| d.chars().take(cori_compiler::DESCRIPTION_MAX).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_description_is_its_first_line_cut_short() {
        assert_eq!(app_description("[app]\nid = \"a\"\ndescription = \"Submit and decide team requests\"\n").as_deref(), Some("Submit and decide team requests"));
        assert_eq!(app_description("[app]\nid = \"a\"\ndescription = \"\"\"one\ntwo\"\"\"\n").as_deref(), Some("one"));
        assert_eq!(app_description(&format!("[app]\ndescription = \"{}\"\n", "a".repeat(200))).map(|d| d.chars().count()), Some(140));
        assert_eq!(app_description("[app]\nid = \"a\"\n"), None);
        assert_eq!(app_description("[app]\ndescription = 3\n"), None);
    }
}

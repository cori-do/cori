// SPDX-License-Identifier: Apache-2.0
//! The product's brand: the name, logos and colours people see in the console, notifications and MCP pages. Cori's by
//! default; a deployment sets its own at runtime with a brand file (`CORI_BRAND_FILE`), so one image serves every brand.
//!
//! ```json
//! { "name": "Acme Flow",
//!   "logo": "logo.svg", "logo_dark": "logo-white.svg", "mark": "mark.svg", "favicon": "favicon.png",
//!   "colors": { "50": "#eef2ff", "100": "#e0e7ff", "500": "#6366f1", "600": "#4f46e5", "700": "#4338ca" } }
//! ```
//!
//! Every field is optional. An image is a file next to the brand file (inlined at startup), a `data:image/…` URL or an
//! `https://` URL. `colors` are the console's accent shades. Protocol names (`cori` headers, the `cori` command) stay,
//! and under another brand the console's footer says "Powered by Cori".
use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const DEFAULT_NAME: &str = "Cori";
const SHADES: [&str; 5] = ["50", "100", "500", "600", "700"];
const MAX_IMAGE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrandFile {
    name: Option<String>,
    logo: Option<String>,
    logo_dark: Option<String>,
    mark: Option<String>,
    favicon: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

/// What the console reads from `window.__CORI_BRAND__`; images are URLs (data: or https:).
#[derive(Debug, Clone, Serialize)]
struct ConsoleBrand {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_dark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mark: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Brand {
    pub name: String,
    /// The console page with the brand written in, rendered once at startup.
    pub console_html: Bytes,
    /// The CSP sources of that page's inline scripts, by hash (`'sha256-…'`, space-separated): the console's policy runs
    /// those and no other script.
    pub console_scripts: String,
    /// The apps portal's page with the brand written in, and its inline scripts' hashes, as the console's.
    pub portal_html: Bytes,
    pub portal_scripts: String,
}

impl Default for Brand {
    fn default() -> Self {
        Self { name: DEFAULT_NAME.into(), console_html: Bytes::from_static(crate::api::CONSOLE_HTML.as_bytes()), console_scripts: script_hashes(crate::api::CONSOLE_HTML),
            portal_html: Bytes::from_static(crate::api::PORTAL_HTML.as_bytes()), portal_scripts: script_hashes(crate::api::PORTAL_HTML) }
    }
}

/// The hashes of `html`'s inline scripts (those without `src`), as CSP sources.
fn script_hashes(html: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        let Some(open) = rest[i..].find('>').map(|j| i + j) else { break };
        let Some(close) = rest[open + 1..].find("</script>").map(|j| open + 1 + j) else { break };
        if !rest[i..open].contains(" src=") { out.push(format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(rest[open + 1..close].as_bytes())))); }
        rest = &rest[close + "</script>".len()..];
    }
    out.join(" ")
}

impl Brand {
    /// The brand of `path`, or Cori's without one.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else { return Ok(Self::default()) };
        let text = std::fs::read_to_string(path).with_context(|| format!("reading the brand file {}", path.display()))?;
        let file: BrandFile = serde_json::from_str(&text).with_context(|| format!("the brand file {}", path.display()))?;
        Self::from_file(file, path.parent().unwrap_or(Path::new(".")))
    }

    fn from_file(f: BrandFile, dir: &Path) -> Result<Self> {
        let name = match f.name.as_deref().map(str::trim) {
            None | Some("") => DEFAULT_NAME.to_string(),
            Some(n) if n.chars().count() > 64 || n.chars().any(char::is_control) => bail!("brand name: at most 64 characters, no control characters"),
            Some(n) => n.to_string(),
        };
        let image = |field: &str, v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|v| image_url(field, v, dir)).transpose();
        let console = ConsoleBrand { name: name.clone(), logo: image("logo", &f.logo)?, logo_dark: image("logo_dark", &f.logo_dark)?, mark: image("mark", &f.mark)? };
        let favicon = image("favicon", &f.favicon)?;
        for (shade, color) in &f.colors {
            if !SHADES.contains(&shade.as_str()) { bail!("brand colors: unknown shade `{shade}` (expected {})", SHADES.join(", ")); }
            if !css_color(color) { bail!("brand colors.{shade}: `{color}` is not a CSS colour"); }
        }
        let html = console_html(crate::api::CONSOLE_HTML, "console", &console, favicon.as_deref(), &f.colors);
        let portal = console_html(crate::api::PORTAL_HTML, "apps", &console, favicon.as_deref(), &f.colors);
        Ok(Self { name, console_scripts: script_hashes(&html), console_html: Bytes::from(html), portal_scripts: script_hashes(&portal), portal_html: Bytes::from(portal) })
    }

    /// Whether this is Cori's own brand (nothing to rewrite).
    pub fn is_default(&self) -> bool { self.name == DEFAULT_NAME }
}

/// A file next to the brand file becomes a data: URL; data:image and https: URLs pass as they are.
fn image_url(field: &str, v: &str, dir: &Path) -> Result<String> {
    if v.starts_with("data:image/") || v.starts_with("https://") { return Ok(v.to_string()); }
    if v.contains("://") || v.starts_with("data:") { bail!("brand {field}: a file, a data:image/… URL or an https:// URL"); }
    let path = dir.join(v);
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        _ => bail!("brand {field}: {v} is not an svg, png, jpg, webp, gif or ico file"),
    };
    let len = std::fs::metadata(&path).with_context(|| format!("brand {field}: {}", path.display()))?.len();
    if len > MAX_IMAGE_BYTES { bail!("brand {field}: {} is over {} KiB", path.display(), MAX_IMAGE_BYTES / 1024); }
    let bytes = std::fs::read(&path).with_context(|| format!("brand {field}: {}", path.display()))?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// A colour value that cannot leave its declaration: `#4f46e5`, `rgb(79 70 229)`, `oklch(0.51 0.23 277)`.
fn css_color(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || " #(),.%/-".contains(c))
}

fn escape_html(s: &str) -> String { s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;") }

/// A page of the console's build (the console, the apps portal: `what` ends its title) with the brand: its title and
/// favicon, `window.__CORI_BRAND__` for the header, and the accent shades as unlayered custom properties (they win over
/// the theme layer's).
fn console_html(page: &str, what: &str, b: &ConsoleBrand, favicon: Option<&str>, colors: &BTreeMap<String, String>) -> String {
    let mut html = page.replacen(&format!("<title>Cori {what}</title>"), &format!("<title>{} {what}</title>", escape_html(&b.name)), 1);
    if let Some(icon) = favicon
        && let Some(start) = html.find("<link rel=\"icon\"")
        && let Some(len) = html[start..].find('>')
    {
        html.replace_range(start..=start + len, &format!("<link rel=\"icon\" href=\"{}\" />", escape_html(icon)));
    }
    // JSON in a script element: `<` escaped so that no string can close it
    let mut head = format!("<script>window.__CORI_BRAND__={}</script>", serde_json::to_string(b).unwrap_or_default().replace('<', "\\u003c"));
    if !colors.is_empty() {
        head.push_str("<style>:root{");
        for (shade, color) in colors { head.push_str(&format!("--color-brand-{shade}:{color};")); }
        head.push_str("}</style>");
    }
    html.replacen("<head>", &format!("<head>{head}"), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_runs_its_own_inline_scripts_only() {
        use base64::Engine;
        use sha2::Digest;
        let html = r#"<head><script type="module" crossorigin>app()</script><script src="/x.js"></script><script>window.a=1</script></head>"#;
        let h = |s: &str| format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(s.as_bytes())));
        assert_eq!(script_hashes(html), format!("{} {}", h("app()"), h("window.a=1")), "inline scripts only, by their exact text");
        // the console has one inline script (a `<script>` inside its code is no tag); a brand adds its own
        assert_eq!(Brand::default().console_scripts.split(' ').count(), 1);
        let branded = Brand::from_file(serde_json::from_str(r#"{ "name": "Acme" }"#).unwrap(), std::path::Path::new(".")).unwrap();
        assert_eq!(branded.console_scripts.split(' ').count(), 2);
    }

    fn brand(json: &str, dir: &Path) -> Result<Brand> { Brand::from_file(serde_json::from_str(json)?, dir) }

    #[test]
    fn without_a_brand_file_the_console_is_cori_s() {
        let b = Brand::load(None).unwrap();
        assert!(b.is_default());
        assert_eq!(&b.console_html[..], crate::api::CONSOLE_HTML.as_bytes());
    }

    #[test]
    fn a_brand_file_names_the_console_and_sets_its_colours_and_images() {
        let dir = std::env::temp_dir().join(format!("cori-brand-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("logo.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let b = brand(r##"{ "name": "Acme <Flow>", "logo": "logo.svg", "favicon": "https://cdn.acme.test/f.png", "colors": { "600": "#123456" } }"##, &dir).unwrap();
        let html = String::from_utf8(b.console_html.to_vec()).unwrap();
        assert_eq!(b.name, "Acme <Flow>");
        assert!(html.contains("<title>Acme &lt;Flow&gt; console</title>"));
        assert!(html.contains("<link rel=\"icon\" href=\"https://cdn.acme.test/f.png\" />"));
        assert!(html.contains("--color-brand-600:#123456;"));
        assert!(html.contains("\"logo\":\"data:image/svg+xml;base64,"));
        // the name cannot close the script element it is written into
        assert!(html.contains("Acme \\u003cFlow>"));
        // the apps portal, the console's second page, carries the same brand and runs its own scripts only
        let portal = String::from_utf8(b.portal_html.to_vec()).unwrap();
        assert!(portal.contains("<title>Acme &lt;Flow&gt; apps</title>") && portal.contains("--color-brand-600:#123456;") && portal.contains("Acme \\u003cFlow>"));
        assert_eq!(b.portal_scripts.split(' ').count(), 2, "its module and the brand's: {}", b.portal_scripts);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_brand_file_cannot_inject_css_or_reach_other_urls() {
        let dir = Path::new(".");
        assert!(brand(r#"{ "colors": { "600": "red;}body{display:none" } }"#, dir).is_err());
        assert!(brand(r##"{ "colors": { "900": "#000" } }"##, dir).is_err());
        assert!(brand(r#"{ "logo": "http://acme.test/logo.png" }"#, dir).is_err());
        assert!(brand(r#"{ "logo": "data:text/html,<script>" }"#, dir).is_err());
        assert!(brand(r#"{ "logo": "logo.exe" }"#, dir).is_err());
        assert!(brand(r#"{ "title": "typo" }"#, dir).is_err());
    }
}

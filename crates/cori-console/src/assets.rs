//! Static asset serving for the embedded SPA.
//!
//! Assets live in `packages/console/build/client/`, embedded at compile
//! time by `rust-embed`. URLs that don't match an asset fall back to
//! `index.html` so client-side routing handles deep links. The router
//! mounts this fallback **only outside** `/api/*` — see `api/mod.rs`
//! for the explicit `/api/{*rest}` 404 carve-out.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../packages/console/build/client"]
pub struct Assets;

/// SPA fallback handler. Tries the requested path; if missing (or
/// missing a `.` extension, signalling a route deep-link rather than
/// an asset) falls back to `index.html` so RR7 can route client-side.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if !path.is_empty()
        && let Some(file) = Assets::get(path)
    {
        return file_response(path, file.data.into_owned());
    }
    match Assets::get("index.html") {
        Some(file) => file_response("index.html", file.data.into_owned()),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Cori Console SPA not built. Run `pnpm --filter @cori-do/console build`.",
        )
            .into_response(),
    }
}

fn file_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = content_type_for(path);
    let mut res = Response::new(Body::from(bytes));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(mime),
    );
    // The hashed asset filenames are cache-safe; `index.html` is not.
    let is_hashed_asset = path.starts_with("assets/") && !path.ends_with(".html");
    let cache = if is_hashed_asset {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}

fn content_type_for(path: &str) -> &'static str {
    let lower = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match lower.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

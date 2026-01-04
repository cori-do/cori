use axum::response::Html;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
pub struct Assets;

pub fn get_utf8(path: &str) -> anyhow::Result<String> {
    let file = Assets::get(path).ok_or_else(|| anyhow::anyhow!("missing embedded asset: {path}"))?;
    let s = std::str::from_utf8(file.data.as_ref())?;
    Ok(s.to_string())
}

pub async fn raw_login_html() -> anyhow::Result<Html<String>> {
    Ok(Html(get_utf8("login.html")?))
}



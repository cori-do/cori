//! `cori login`.
//!
//! v1 ships an OSS-first product: every command works without login.
//! `cori login` exists so users can opt into the (future) Cori cloud
//! features and so the muscle memory `curl install → login → demo` works
//! on day one. Until the auth server is live we accept a paste-in token
//! and persist it to `~/.cori/config.toml`. The flow degrades gracefully
//! when no browser is available.
//!
//! Storage format:
//!
//! ```toml
//! [account]
//! token = "..."
//! base_url = "https://app.cori.do"
//! ```

use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};

use crate::config::Config;

const DEFAULT_BASE_URL: &str = "https://app.cori.do";
const LOGIN_PATH: &str = "/cli-login";

pub fn run() -> Result<()> {
    let base_url = std::env::var("CORI_LOGIN_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let nonce = random_nonce();
    let url = format!("{base_url}{LOGIN_PATH}?nonce={nonce}");

    println!("Cori login is optional — every CLI command works without it.");
    println!("Logging in attaches this install to a Cori cloud account for");
    println!("future hub features (sharing runbooks, hosted workers).\n");

    let opened = open_browser(&url);
    if opened {
        println!("✓ Opened your browser to:");
    } else {
        println!("Could not open a browser automatically. Visit this URL:");
    }
    println!("    {url}\n");
    println!("After signing in, copy the token shown on that page and paste it below.");
    println!("(Or press Ctrl-C to cancel — you can keep using Cori in local mode.)\n");

    print!("Token: ");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let mut buf = String::new();
    stdin
        .lock()
        .read_line(&mut buf)
        .context("reading token from stdin")?;
    let token = buf.trim();
    if token.is_empty() {
        bail!("no token entered — aborted");
    }

    let mut cfg = Config::load().context("loading ~/.cori/config.toml")?;
    cfg.set("account.token", token)
        .context("storing token in config")?;
    cfg.set("account.base_url", &base_url)
        .context("storing base_url in config")?;
    cfg.save().context("writing ~/.cori/config.toml")?;

    println!("\n✓ Signed in. Token stored in ~/.cori/config.toml under [account].");
    println!("  Use `cori config get account.token` to verify or `cori config set` to overwrite.");
    Ok(())
}

/// Best-effort browser opener. Returns `true` if the OS-specific
/// `open`/`xdg-open`/`start` succeeded.
fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "cmd";

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        std::process::Command::new(cmd)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut c| c.wait().map(|s| s.success()).unwrap_or(false))
            .unwrap_or(false)
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(cmd)
            .args(["/C", "start", "", url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut c| c.wait().map(|s| s.success()).unwrap_or(false))
            .unwrap_or(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        false
    }
}

/// Pseudo-random nonce for the URL — only needs to be unguessable enough
/// to bind the browser session to this CLI invocation. Uses the system
/// clock plus the process id; not crypto-grade but adequate for v1.
fn random_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{nanos:x}-{pid:x}")
}

//! OAuth2 authorization-code + PKCE flow (RFC 7636).
//!
//! Targets the user-interactive case: a `Person` worker has a browser
//! and a controlling terminal. We open the system browser to the
//! authorization endpoint, listen on a transient `127.0.0.1:<port>`
//! redirect URI, capture the `code` parameter, and POST it to the
//! token endpoint together with the PKCE verifier.
//!
//! Client secrets are intentionally **never** used here — Cori is a
//! public client on the user's machine; PKCE is the security primitive.
//!
//! The implementation is deliberately minimal — it implements only the
//! flow the v1 acceptance test needs (browser-interactive PKCE for one
//! MCP server). Device grant and client-credentials are stubs in their
//! own modules.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::types::{AuthKind, Token};

#[derive(Debug, Error)]
pub enum PkceError {
    #[error("could not bind a redirect listener on 127.0.0.1: {0}")]
    Bind(#[source] std::io::Error),
    #[error("could not open the system browser: {0}")]
    Browser(String),
    #[error("authorization callback never arrived (timeout after {0:?})")]
    Timeout(Duration),
    #[error("authorization server returned an error: {error} — {description}")]
    AuthServer { error: String, description: String },
    #[error("token endpoint HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("token endpoint returned status {status}: {body}")]
    TokenResponse { status: u16, body: String },
    #[error("token response was not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("PKCE flow aborted: {0}")]
    Aborted(String),
}

pub type Result<T> = std::result::Result<T, PkceError>;

/// Inputs to a single PKCE run.
pub struct PkceRequest {
    /// Authorization endpoint URL (e.g. `https://auth.notion.so/oauth/authorize`).
    pub authorization_endpoint: String,
    /// Token endpoint URL (e.g. `https://auth.notion.so/oauth/token`).
    pub token_endpoint: String,
    /// OAuth client id Cori was registered as for this server.
    pub client_id: String,
    /// Space-separated scope list to request.
    pub scopes: Vec<String>,
    /// Pre-formatted "human" label printed before the browser opens
    /// (e.g. `"Notion"`). Used only for terminal output.
    pub display_name: String,
    /// Maximum time we'll wait for the redirect callback.
    pub timeout: Duration,
}

/// Run the full PKCE dance: open browser → wait for redirect → exchange
/// code → return [`Token`].
pub fn run(req: &PkceRequest) -> Result<Token> {
    // 1. Bind a localhost listener on an OS-chosen port.
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).map_err(PkceError::Bind)?;
    let port = listener.local_addr().map_err(PkceError::Bind)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 2. Generate PKCE verifier + challenge.
    let verifier = generate_verifier();
    let challenge = derive_challenge(&verifier);
    let state = generate_state();

    // 3. Build the authorization URL.
    let mut auth_url = url::Url::parse(&req.authorization_endpoint)
        .map_err(|e| PkceError::Aborted(format!("invalid authorization endpoint: {e}")))?;
    {
        let mut q = auth_url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &req.client_id);
        q.append_pair("redirect_uri", &redirect_uri);
        q.append_pair("code_challenge", &challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("state", &state);
        if !req.scopes.is_empty() {
            q.append_pair("scope", &req.scopes.join(" "));
        }
    }

    eprintln!(
        "Opening browser to authorize {}…\nIf the browser doesn't open, visit:\n  {}",
        req.display_name, auth_url
    );

    // 4. Open the system browser. Best-effort — we still listen even if
    //    the open fails so the user can paste the URL manually.
    if let Err(e) = webbrowser::open(auth_url.as_str()) {
        eprintln!("(could not open browser automatically: {e})");
    }

    // 5. Wait for the redirect — single request, then return.
    let callback = wait_for_callback(&listener, req.timeout)?;
    if callback.state != state {
        return Err(PkceError::Aborted(
            "redirect `state` did not match the PKCE request (possible CSRF)".to_string(),
        ));
    }
    if let Some(err) = callback.error {
        let description = callback.error_description.unwrap_or_default();
        return Err(PkceError::AuthServer {
            error: err,
            description,
        });
    }
    let code = callback.code.ok_or_else(|| {
        PkceError::Aborted("redirect URI carried neither `code` nor `error`".to_string())
    })?;

    // 6. Exchange the code for a token at the token endpoint.
    let token = exchange_code(req, &code, &redirect_uri, &verifier)?;
    Ok(token)
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

fn generate_verifier() -> String {
    let mut bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn derive_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.1 listener — accept one GET /callback?... request, parse
// the query string, and reply with a small success page.
// ---------------------------------------------------------------------------

struct Callback {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

fn wait_for_callback(listener: &TcpListener, timeout: Duration) -> Result<Callback> {
    listener.set_nonblocking(true).map_err(PkceError::Bind)?;
    let started = Instant::now();
    loop {
        if started.elapsed() >= timeout {
            return Err(PkceError::Timeout(timeout));
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(PkceError::Bind)?;
                return handle_one_request(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(PkceError::Bind(e)),
        }
    }
}

fn handle_one_request(mut stream: TcpStream) -> Result<Callback> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(PkceError::Bind)?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(PkceError::Bind)?;
    // Drain headers (we don't need them).
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).map_err(PkceError::Bind)?;
        if n == 0 || buf == "\r\n" || buf == "\n" {
            break;
        }
    }

    // Parse "GET /callback?code=...&state=... HTTP/1.1"
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let url = url::Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|e| PkceError::Aborted(format!("malformed callback URL: {e}")))?;

    let mut code = None;
    let mut state = String::new();
    let mut error = None;
    let mut error_description = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = v.into_owned(),
            "error" => error = Some(v.into_owned()),
            "error_description" => error_description = Some(v.into_owned()),
            _ => {}
        }
    }

    let body = if error.is_some() {
        "<html><body><h2>Sign-in failed.</h2><p>You can close this tab and return to the terminal.</p></body></html>"
    } else {
        "<html><body><h2>Signed in to Cori.</h2><p>You can close this tab and return to the terminal.</p></body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    // Best-effort: read & discard any remaining bytes the browser sent.
    let mut sink = Vec::new();
    let _ = reader.get_mut().take(1024).read_to_end(&mut sink);

    Ok(Callback {
        code,
        state,
        error,
        error_description,
    })
}

// ---------------------------------------------------------------------------
// Token-endpoint code exchange
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

fn exchange_code(
    req: &PkceRequest,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Token> {
    let client = reqwest::blocking::Client::new();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", req.client_id.as_str())
        .append_pair("code_verifier", verifier)
        .finish();
    let resp = client
        .post(&req.token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()?;
    let status = resp.status();
    let body = resp.text()?;
    if !status.is_success() {
        return Err(PkceError::TokenResponse {
            status: status.as_u16(),
            body,
        });
    }
    let tr: TokenResponse = serde_json::from_str(&body)?;
    let expires_at = tr
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s));
    Ok(Token {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        token_type: tr.token_type.unwrap_or_else(|| "Bearer".to_string()),
        expires_at,
        scope: tr.scope.or_else(|| {
            if req.scopes.is_empty() {
                None
            } else {
                Some(req.scopes.join(" "))
            }
        }),
        auth_kind: AuthKind::Pkce,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_url_safe_and_long_enough() {
        let v = generate_verifier();
        assert!(v.len() >= 43, "verifier too short: {}", v.len());
        for c in v.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-URL-safe char {c}"
            );
        }
    }

    #[test]
    fn pkce_challenge_is_deterministic_for_verifier() {
        let v = "abc123".to_string();
        assert_eq!(derive_challenge(&v), derive_challenge(&v));
    }
}

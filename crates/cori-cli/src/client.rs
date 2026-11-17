// SPDX-License-Identifier: Apache-2.0
//! The CLI's one way to call a Cori server: one HTTP client with timeouts, the bearer token, and one reading of every
//! answer. A status other than 2xx is an error carrying the server's message (on 401, how to sign in again); a JSON-RPC
//! error (a store's MCP endpoint) is an error too; a body that is not JSON (a proxy's page) comes back as its text.
use anyhow::{Result, anyhow};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What the command line calls itself on every request, so that a server knows which ones are in use.
pub const USER_AGENT: &str = concat!("cori/", env!("CARGO_PKG_VERSION"));

/// Reads the oldest command line a server serves (`x-cori-cli-min`, on its API's answers) and says once, when this one
/// is older, that it should be upgraded.
pub(crate) fn note_cli_min(headers: &reqwest::header::HeaderMap) {
    static SAID: std::sync::Once = std::sync::Once::new();
    let Some(min) = headers.get("x-cori-cli-min").and_then(|v| v.to_str().ok()) else { return };
    if older(env!("CARGO_PKG_VERSION"), min) {
        SAID.call_once(|| eprintln!("warning: this server serves cori {min} or newer, and this one is {}: upgrade it (`npm install -g @cori-do/cli`)", env!("CARGO_PKG_VERSION")));
    }
}

/// Whether version `a` comes before `b` (`x.y.z`, a leading `v` allowed; a pre-release or build suffix is ignored, and a
/// missing part counts as 0).
pub(crate) fn older(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> [u64; 3] {
        let mut out = [0; 3];
        let core = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
        for (i, p) in core.split('.').take(3).enumerate() { out[i] = p.parse().unwrap_or(0); }
        out
    }
    parts(a) < parts(b)
}

pub struct Api { base: String, token: String, http: reqwest::Client, app_root: Option<PathBuf> }

impl Api {
    /// Calls that may take a while (a deploy compiles and seeds on the server): 5 minutes.
    pub fn new(server: &str, token: &str) -> Self { Self::with_timeout(server, token, Duration::from_secs(300)) }

    /// Calls that give up after `timeout` (a best-effort lookup).
    pub fn with_timeout(server: &str, token: &str, timeout: Duration) -> Self {
        let http = cori_engine::http::builder().user_agent(USER_AGENT).connect_timeout(timeout.min(Duration::from_secs(10))).timeout(timeout).build().unwrap_or_default();
        Self { base: server.trim_end_matches('/').to_string(), token: token.to_string(), http, app_root: None }
    }

    /// The app folder the calls are for: an expired session's error names its session file.
    pub fn for_app(mut self, root: Option<&Path>) -> Self { self.app_root = root.map(Path::to_path_buf); self }

    /// A request with the token, for calls that send or read bytes (deploy, pull) or read a status themselves (`call`). From
    /// an app folder of an organization, it names that organization (`x-cori-organization`, `CORI_ORGANIZATION`): a server
    /// hosting several refuses a token of another one rather than act there.
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let r = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        match std::env::var("CORI_ORGANIZATION").ok().filter(|o| !o.trim().is_empty()) { Some(o) => r.header("x-cori-organization", o.trim()), None => r }
    }

    pub async fn get(&self, path: &str) -> Result<Value> { self.send(self.request(Method::GET, path)).await }
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> { self.send(self.request(Method::POST, path).json(body)).await }
    pub async fn delete(&self, path: &str) -> Result<Value> { self.send(self.request(Method::DELETE, path)).await }

    /// A text answer (a generated client, a JSONL dataset).
    pub async fn text(&self, path: &str) -> Result<String> {
        let (status, text) = self.raw(self.request(Method::GET, path)).await?;
        if !status.is_success() { return Err(self.error(status, &parse(&text))); }
        Ok(text)
    }

    /// A JSON-RPC call to a store's MCP endpoint: its `result`, or its `error` as an error.
    pub async fn rpc(&self, path: &str, method: &str, params: Value) -> Result<Value> {
        let v = self.post(path, &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })).await?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) { return Err(anyhow!("{}", e.get("message").and_then(|m| m.as_str()).or(e.as_str()).unwrap_or("the call failed"))); }
        Ok(v["result"].clone())
    }

    /// The JSON body of a 2xx answer, else the server's error.
    pub async fn send(&self, req: RequestBuilder) -> Result<Value> {
        let (status, body) = self.call(req).await?;
        if !status.is_success() { return Err(self.error(status, &body)); }
        Ok(body)
    }

    /// The status and the body, for a caller that answers some statuses itself (409 on create, 404 on approve).
    pub async fn call(&self, req: RequestBuilder) -> Result<(StatusCode, Value)> {
        let (status, text) = self.raw(req).await?;
        Ok((status, parse(&text)))
    }

    /// The status and the text of an answer, for a caller that reads both (a route that may be missing on the server).
    pub async fn text_of(&self, req: RequestBuilder) -> Result<(StatusCode, String)> { self.raw(req).await }

    /// The error a failed call reports: the server's message, and on 401 how to sign in again.
    pub fn error(&self, status: StatusCode, body: &Value) -> anyhow::Error { crate::commands::server_error(status, body, self.app_root.as_deref(), &self.base) }

    async fn raw(&self, req: RequestBuilder) -> Result<(StatusCode, String)> {
        let r = req.send().await.map_err(|e| anyhow!("{} is not reachable: {e}", self.base))?;
        note_cli_min(r.headers());
        let status = r.status();
        Ok((status, r.text().await.unwrap_or_default()))
    }
}

/// A body as JSON when it is JSON, else as its text, else null.
pub(crate) fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| if text.trim().is_empty() { Value::Null } else { Value::String(text.trim().to_string()) })
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_compare_by_their_numbers() {
        use super::older;
        assert!(older("0.1.0", "0.2.0") && older("0.9.9", "0.10.0") && older("1.2.3", "v1.2.4"));
        assert!(!older("0.2.0", "0.2.0") && !older("0.3.0", "0.2.9"));
        // a pre-release counts as its release; a missing part as 0
        assert!(!older("0.2.0-rc.1", "0.2.0") && !older("0.2", "0.2.0") && older("0.2", "0.2.1"));
    }

    #[test]
    fn bodies_are_json_or_their_text() {
        assert_eq!(super::parse(r#"{"error":"no"}"#)["error"], "no");
        assert_eq!(super::parse("<html>502 Bad Gateway</html>\n"), "<html>502 Bad Gateway</html>");
        assert!(super::parse("  ").is_null());
    }
}

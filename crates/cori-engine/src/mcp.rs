// SPDX-License-Identifier: Apache-2.0
//! Minimal MCP client: `tools/call` over stdio (spawned server) or streamable HTTP (JSON-RPC POST).
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL: &str = "2025-03-26";

/// `egress`: what an HTTP server's address may be (`crate::egress`); a stdio server is a program on this worker.
pub async fn call_tool(c: &crate::config::ResolvedConnector, root: &Path, tool: &str, args: &Value, bearer: Option<&str>, egress: crate::egress::Egress) -> Result<Value> {
    if let Some(url) = &c.url { return call_http(url, tool, args, bearer, egress).await; }
    if let Some(cmd) = &c.command { return call_stdio(cmd, root, tool, args).await; }
    Err(anyhow!("MissingCapabilityError: mcp connector `{}` has neither `url` (HTTP) nor `command` (stdio) nor a dev mock", c.name))
}

/// `tools/list` on a connector: the tool declarations (name, description, inputSchema, outputSchema, annotations).
pub async fn list_tools(c: &crate::config::ResolvedConnector, root: &Path, bearer: Option<&str>, egress: crate::egress::Egress) -> Result<Vec<Value>> {
    let v = if let Some(url) = &c.url { rpc_http(url, "tools/list", json!({}), bearer, egress).await? }
        else if let Some(cmd) = &c.command { rpc_stdio(cmd, root, "tools/list", json!({})).await? }
        else { return Err(anyhow!("MissingCapabilityError: mcp connector `{}` has neither `url` (HTTP) nor `command` (stdio)", c.name)) };
    if let Some(e) = v.get("error") { return Err(anyhow!("StepFailedError: mcp error: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(""))); }
    Ok(v["result"]["tools"].as_array().cloned().unwrap_or_default())
}

/// A `tools/call` answer's result: its structured content, else its text (parsed when JSON); an error answer fails.
pub(crate) fn tool_result(v: Value) -> Result<Value> {
    if let Some(e) = v.get("error") { return Err(anyhow!("StepFailedError: mcp error {}: {}", e.get("code").cloned().unwrap_or(Value::Null), e.get("message").and_then(|m| m.as_str()).unwrap_or(""))); }
    let r = v.get("result").cloned().unwrap_or(Value::Null);
    if r.get("isError").and_then(|b| b.as_bool()) == Some(true) { return Err(anyhow!("StepFailedError: tool returned an error: {}", text_of(&r))); }
    // structuredContent wins; else the first JSON-looking text block; else the text
    if let Some(sc) = r.get("structuredContent") { return Ok(sc.clone()); }
    let t = text_of(&r);
    Ok(serde_json::from_str(&t).unwrap_or(Value::String(t)))
}

fn text_of(r: &Value) -> String { r.get("content").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")).unwrap_or_default() }

async fn call_http(url: &str, tool: &str, args: &Value, bearer: Option<&str>, egress: crate::egress::Egress) -> Result<Value> {
    tool_result(rpc_http(url, "tools/call", json!({ "name": tool, "arguments": args }), bearer, egress).await?)
}

async fn rpc_http(url: &str, method: &str, params: Value, bearer: Option<&str>, egress: crate::egress::Egress) -> Result<Value> {
    let url = reqwest::Url::parse(url).map_err(|e| anyhow!("MissingCapabilityError: mcp url `{url}`: {e}"))?;
    let client = crate::egress::client_for(&url, egress, None).await?;
    let mut req = client.post(url).header("accept", "application/json, text/event-stream").json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }));
    if let Some(b) = bearer { req = req.bearer_auth(b); }
    let res = req.send().await.map_err(|e| anyhow!("Transient: mcp http: {e}"))?;
    let status = res.status().as_u16();
    if status == 401 || status == 403 {
        // the server's own words say what to fix (an API not enabled for the project, a scope not granted)
        let challenge = res.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).map(String::from);
        let reason = refusal_reason(&res.text().await.unwrap_or_default()).or(challenge.map(|c| c.chars().take(300).collect()));
        return Err(anyhow!("AuthenticationError: mcp server returned {status}{}", reason.map(|r| format!(": {r}")).unwrap_or_default()));
    }
    if status >= 500 || status == 429 { return Err(anyhow!("Transient: mcp server returned {status}")); }
    let ct = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = res.text().await.map_err(|e| anyhow!("Transient: {e}"))?;
    let v: Value = if ct.contains("text/event-stream") {
        let line = body.lines().filter_map(|l| l.strip_prefix("data:")).map(|l| l.trim()).find(|l| l.starts_with('{')).ok_or_else(|| anyhow!("StepFailedError: empty mcp event stream"))?;
        serde_json::from_str(line)?
    } else { serde_json::from_str(&body).map_err(|e| anyhow!("StepFailedError: mcp response is not JSON: {e}"))? };
    Ok(v)
}

/// What a refusal's JSON body says (a JSON-RPC or an API's `error.message`, an OAuth `error_description`), shortened.
fn refusal_reason(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let m = v["error"]["message"].as_str().or(v["error_description"].as_str()).or(v["error"].as_str()).filter(|m| !m.trim().is_empty())?;
    Some(m.trim().chars().take(300).collect())
}

async fn call_stdio(cmd: &[String], root: &Path, tool: &str, args: &Value) -> Result<Value> {
    tool_result(rpc_stdio(cmd, root, "tools/call", json!({ "name": tool, "arguments": args })).await?)
}

/// One JSON-RPC request to an MCP server over stdio, started for it in the app folder of `root` (`sandbox::mcp_stdio`).
pub(crate) async fn rpc_stdio(cmd: &[String], root: &Path, method: &str, params: Value) -> Result<Value> {
    let (bin, rest) = cmd.split_first().ok_or_else(|| anyhow!("InvalidInputError: empty mcp command"))?;
    // commands are relative to the app root (the nearest folder with cori.toml, as for the runner's read access)
    let cwd = cori_compiler::app_root_of(root);
    let mut child = crate::events::scrub_env(tokio::process::Command::new(bin).args(rest).current_dir(cwd)).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn().map_err(|e| anyhow!("MissingCapabilityError: cannot start mcp server `{bin}`: {e}"))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let stderr = child.stderr.take().unwrap();
    let err_buf = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    { let eb = err_buf.clone(); tokio::spawn(async move { let mut r = BufReader::new(stderr); let mut l = String::new(); while r.read_line(&mut l).await.unwrap_or(0) > 0 { let mut g = eb.lock().await; if g.len() < 4000 { g.push_str(&l); } l.clear(); } }); }
    async fn rpc(stdin: &mut tokio::process::ChildStdin, stdout: &mut BufReader<tokio::process::ChildStdout>, msg: Value, expect_id: Option<u64>) -> Result<Option<Value>> {
        stdin.write_all((serde_json::to_string(&msg)? + "\n").as_bytes()).await?;
        stdin.flush().await?;
        let Some(id) = expect_id else { return Ok(None) };
        loop {
            let mut line = String::new();
            if stdout.read_line(&mut line).await? == 0 { return Err(anyhow!("StepFailedError: mcp server closed")); }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) { return Ok(Some(v)); }
        }
    }
    // a server that never answers initialize must not hold the step forever: the same limit as the call
    let init = match tokio::time::timeout(std::time::Duration::from_secs(60), rpc(&mut stdin, &mut stdout, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": { "name": "cori", "version": "2" } } }), Some(1))).await.unwrap_or_else(|_| Err(anyhow!("Transient: mcp server did not answer initialize within 60 s"))) {
        Ok(v) => v,
        Err(e) => { tokio::time::sleep(std::time::Duration::from_millis(50)).await; let err = err_buf.lock().await.clone(); return Err(anyhow!("{e}{}", if err.trim().is_empty() { String::new() } else { format!(": {}", err.trim().lines().last().unwrap_or("")) })); }
    };
    if let Some(e) = init.as_ref().and_then(|v| v.get("error")) { return Err(anyhow!("StepFailedError: mcp initialize failed: {e}")); }
    rpc(&mut stdin, &mut stdout, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), None).await?;
    let res = tokio::time::timeout(std::time::Duration::from_secs(60), rpc(&mut stdin, &mut stdout, json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }), Some(2))).await.map_err(|_| anyhow!("Transient: mcp call timed out"))??;
    let _ = child.kill().await;
    Ok(res.unwrap_or(Value::Null))
}

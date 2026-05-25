//! Dispatch an `mcp_tool` step.
//!
//! Phase 4 keeps the MCP client deliberately minimal: for each
//! `mcp_tool` step we spawn the configured server as a stdio child,
//! perform the standard MCP `initialize` handshake, send one
//! `tools/call`, and shut the child down. A long-lived connection is
//! deferred to Phase 6 (the worker process holds connections across
//! runs).
//!
//! Server configuration lives in `~/.cori/mcp-servers.json`:
//!
//! ```json
//! {
//!   "servers": {
//!     "slack": {
//!       "command": ["slack-mcp-server"],
//!       "args": ["--workspace", "veepee"],
//!       "env": { "SLACK_TOKEN": "xoxb-..." }
//!     }
//!   }
//! }
//! ```
//!
//! The capability check (in [`crate::capabilities`]) refuses to start a
//! workflow whose `mcp_servers` declaration names a server absent from
//! that file, so by the time we reach [`run`] the lookup always
//! succeeds.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

use crate::capabilities::Capabilities;
use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result};

/// MCP server launch spec — read from `~/.cori/mcp-servers.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServerConfig {
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ArgsSpec {
    server: String,
    tool: String,
    #[serde(default)]
    args: JsonValue,
}

pub fn run(
    runtime: &Runtime,
    capabilities: &Capabilities,
    step_file_path: &Path,
    input: &JsonValue,
) -> Result<ActivityOutcome> {
    let started = Instant::now();

    // 1. Resolve args via the runner.
    let args_call =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::McpArgs, input)?;
    let spec: ArgsSpec =
        serde_json::from_value(args_call.output.clone()).map_err(|e| BrokerError::BadEnvelope {
            envelope: args_call.output.to_string(),
            source: e,
        })?;

    let server_cfg = capabilities.mcp_servers.get(&spec.server).ok_or_else(|| {
        BrokerError::CapabilityDenied {
            kind: "MCP server",
            name: spec.server.clone(),
            hint: format!(
                "declare `{server}` in ~/.cori/mcp-servers.json with a `command` to launch it",
                server = spec.server
            ),
        }
    })?;

    // 2. Spawn + call.
    let output = call_tool(server_cfg, &spec.tool, &spec.args)?;

    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output,
        duration: started.elapsed(),
        stderr: args_call.stderr,
        cost_eur: None,
        usage: None,
    })
}

/// Spawn one MCP server, perform `initialize`, call one tool, return its
/// result. Best-effort termination of the child at the end.
fn call_tool(cfg: &McpServerConfig, tool: &str, args: &JsonValue) -> Result<JsonValue> {
    let bin = cfg.command.first().ok_or_else(|| BrokerError::StepFailed {
        message: "MCP server config has an empty `command`".to_string(),
        stack: None,
    })?;

    let mut cmd = Command::new(bin);
    cmd.args(&cfg.command[1..])
        .args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| BrokerError::McpSpawn {
        binary: bin.clone(),
        source: e,
    })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| BrokerError::McpProtocol("MCP child stdin not piped".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BrokerError::McpProtocol("MCP child stdout not piped".into()))?;

    let mut writer = stdin;
    let mut reader = BufReader::new(stdout);

    // --- 1. initialize ---
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "cori", "version": env!("CARGO_PKG_VERSION") },
            },
        }),
    )?;
    let _init_response = read_response(&mut reader, 1)?;

    // --- 2. initialized notification ---
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }),
    )?;

    // --- 3. tools/call ---
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args },
        }),
    )?;
    let response = read_response(&mut reader, 2)?;

    // --- 4. graceful shutdown — best effort ---
    let _ = send(
        &mut writer,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }),
    );
    drop(writer);
    let _ = child.kill();
    let _ = child.wait();

    if let Some(err) = response.get("error") {
        return Err(BrokerError::McpProtocol(format!(
            "MCP server returned error: {err}"
        )));
    }
    let result = response.get("result").cloned().unwrap_or(JsonValue::Null);
    Ok(result)
}

fn send(w: &mut impl Write, msg: &JsonValue) -> Result<()> {
    let line = serde_json::to_string(msg).expect("json serializable");
    w.write_all(line.as_bytes()).map_err(BrokerError::Io)?;
    w.write_all(b"\n").map_err(BrokerError::Io)?;
    w.flush().map_err(BrokerError::Io)?;
    Ok(())
}

/// Read line-delimited JSON-RPC messages until we see one whose `id`
/// matches `expect_id`. Notifications and out-of-order messages are
/// dropped.
fn read_response(r: &mut impl BufRead, expect_id: u64) -> Result<JsonValue> {
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = r.read_line(&mut buf).map_err(BrokerError::Io)?;
        if n == 0 {
            return Err(BrokerError::McpProtocol(
                "MCP server closed stdout before responding".into(),
            ));
        }
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: JsonValue = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // tolerate stray log lines
        };
        match v.get("id").and_then(|i| i.as_u64()) {
            Some(id) if id == expect_id => return Ok(v),
            _ => continue,
        }
    }
}

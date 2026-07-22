//! Integration tests for `cori mcp` — drive the real binary over stdio
//! with newline-delimited JSON-RPC, exactly as an MCP client would.
//!
//! These mirror the CLI behaviour contract: tools are a strict subset of
//! the verbs, `run` never executes without a human confirmation, and
//! `CORI_ASSUME_YES` is ignored at the MCP surface.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};

const RECV_TIMEOUT: Duration = Duration::from_secs(60);

struct McpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    incoming: mpsc::Receiver<JsonValue>,
    /// Owns the fake `~/.cori`; dropped (deleted) with the client.
    _home: tempfile::TempDir,
    home_path: std::path::PathBuf,
}

impl McpClient {
    fn spawn(extra_env: &[(&str, &str)]) -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let home_path = home.path().to_path_buf();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cori"));
        cmd.arg("mcp")
            .env("CORI_HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawning `cori mcp`");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<JsonValue>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });

        McpClient {
            child,
            stdin,
            incoming,
            _home: home,
            home_path,
        }
    }

    fn send(&mut self, msg: JsonValue) {
        writeln!(self.stdin, "{msg}").expect("writing to child stdin");
        self.stdin.flush().expect("flushing child stdin");
    }

    /// Wait for the response carrying `id`, skipping notifications and
    /// unrelated messages.
    fn recv_response(&self, id: u64) -> JsonValue {
        loop {
            let msg = self
                .incoming
                .recv_timeout(RECV_TIMEOUT)
                .expect("timed out waiting for MCP response");
            if msg.get("id").and_then(|i| i.as_u64()) == Some(id) && msg.get("method").is_none() {
                return msg;
            }
        }
    }

    /// Wait for a server→client request with the given method (elicitation).
    fn recv_server_request(&self, method: &str) -> JsonValue {
        loop {
            let msg = self
                .incoming
                .recv_timeout(RECV_TIMEOUT)
                .expect("timed out waiting for server request");
            if msg.get("method").and_then(|m| m.as_str()) == Some(method) {
                return msg;
            }
        }
    }

    fn initialize(&mut self, with_elicitation: bool) -> JsonValue {
        let caps = if with_elicitation {
            json!({ "elicitation": {} })
        } else {
            json!({})
        };
        self.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": caps,
                "clientInfo": { "name": "cori-mcp-tests", "version": "0" }
            }
        }));
        let resp = self.recv_response(1);
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        resp
    }

    fn call_tool(&mut self, id: u64, name: &str, args: JsonValue) -> JsonValue {
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }));
        self.recv_response(id)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn examples_dir(name: &str) -> String {
    // crates/cori-cli → ../../examples/<name>
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("examples");
    p.push(name);
    p.display().to_string()
}

// ---------------------------------------------------------------------------

#[test]
fn handshake_tools_resources_prompts() {
    let mut c = McpClient::spawn(&[]);
    let init = c.initialize(false);
    assert_eq!(
        init.pointer("/result/protocolVersion").unwrap(),
        "2025-06-18"
    );
    assert_eq!(init.pointer("/result/serverInfo/name").unwrap(), "cori");

    c.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = c.recv_response(2);
    let names: Vec<&str> = tools
        .pointer("/result/tools")
        .and_then(|t| t.as_array())
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["check", "run", "show", "runs_list", "runs_show", "status"],
        "tools must be exactly the CLI-verb subset"
    );
    // Locked exclusions — never expose these over MCP.
    for forbidden in ["login", "work", "config", "save_workflow"] {
        assert!(
            !names.contains(&forbidden),
            "`{forbidden}` must not be a tool"
        );
    }

    c.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }));
    let resources = c.recv_response(3);
    let uris: Vec<&str> = resources
        .pointer("/result/resources")
        .and_then(|r| r.as_array())
        .expect("resources array")
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris.len(), 5, "SKILL.md + four references");
    assert!(uris.contains(&"cori://skill/SKILL.md"));
    assert!(uris.contains(&"cori://skill/references/trace_interpretation.md"));

    c.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "resources/read",
        "params": { "uri": "cori://skill/SKILL.md" }
    }));
    let read = c.recv_response(4);
    let text = read
        .pointer("/result/contents/0/text")
        .and_then(|t| t.as_str())
        .expect("resource text");
    assert!(
        text.starts_with("---"),
        "SKILL.md frontmatter served verbatim"
    );

    c.send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "prompts/get",
        "params": { "name": "cori-save-workflow" }
    }));
    let prompt = c.recv_response(5);
    let ptext = prompt
        .pointer("/result/messages/0/content/text")
        .and_then(|t| t.as_str())
        .expect("prompt text");
    assert_eq!(ptext, text, "prompt serves the same embedded SKILL.md");
}

#[test]
fn run_refused_without_confirmation_channel_even_with_assume_yes() {
    // No elicitation capability AND the native-dialog fallback disabled:
    // `run` must fail closed, even with CORI_ASSUME_YES=1 in the environment.
    let mut c = McpClient::spawn(&[
        ("CORI_ASSUME_YES", "1"),
        ("CORI_MCP_DISABLE_NATIVE_CONFIRM", "1"),
    ]);
    c.initialize(false); // no elicitation capability
    let resp = c.call_tool(2, "run", json!({ "source": examples_dir("code_only") }));
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/status").unwrap(),
        "not_run"
    );
    let text = resp
        .pointer("/result/content/0/text")
        .and_then(|t| t.as_str())
        .unwrap();
    assert!(
        text.contains("no human confirmation"),
        "refusal must explain that no confirmation channel exists, got: {text}"
    );
    assert!(!c.home_path.join("runs").exists(), "nothing was executed");
}

#[test]
fn run_declined_via_elicitation_does_not_execute() {
    let mut c = McpClient::spawn(&[]);
    c.initialize(true);
    c.send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "run", "arguments": { "source": examples_dir("code_only") } }
    }));

    let elicit = c.recv_server_request("elicitation/create");
    let msg = elicit
        .pointer("/params/message")
        .and_then(|m| m.as_str())
        .expect("elicitation message");
    assert!(
        msg.contains("Run Cori workflow"),
        "per-run confirm must name the workflow, got: {msg}"
    );
    let elicit_id = elicit["id"].clone();
    c.send(json!({
        "jsonrpc": "2.0", "id": elicit_id,
        "result": { "action": "decline" }
    }));

    let resp = c.recv_response(2);
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/status").unwrap(),
        "not_run"
    );
    // Nothing was executed: the fake CORI_HOME has no runs directory.
    assert!(!c.home_path.join("runs").exists());
}

#[test]
fn runs_list_and_show_read_persisted_traces() {
    let mut c = McpClient::spawn(&[]);

    // Persist one fixture trace the way run_workflow does.
    let now = chrono::Utc::now();
    let trace = cori_protocol::RunTrace {
        run_id: "run-test-0001".into(),
        workflow_id: "fixture_wf".into(),
        workflow_content_hash: None,
        status: "succeeded".into(),
        trigger: "cli".into(),
        dry_run: false,
        requesting_identity: None,
        started_at: now,
        ended_at: now,
        duration_ms: 42,
        source: None,
        params: json!({}),
        activities: vec![],
        cost: cori_protocol::CostSummary::default(),
        error: None,
    };
    let dir = c.home_path.join("runs").join("fixture_wf-00000000");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("2026-07-21T00-00-00Z.json"),
        serde_json::to_vec(&trace).unwrap(),
    )
    .unwrap();

    c.initialize(false);

    let list = c.call_tool(2, "runs_list", json!({}));
    assert_eq!(list.pointer("/result/isError").unwrap(), false);
    let rows = list
        .pointer("/result/structuredContent")
        .and_then(|v| v.as_array())
        .expect("runs array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["run_id"], "run-test-0001");
    assert_eq!(rows[0]["workflow_id"], "fixture_wf");

    let shown = c.call_tool(3, "runs_show", json!({ "run_id": "run-test-0001" }));
    assert_eq!(shown.pointer("/result/isError").unwrap(), false);
    assert_eq!(
        shown.pointer("/result/structuredContent/status").unwrap(),
        "succeeded"
    );

    let missing = c.call_tool(4, "runs_show", json!({ "run_id": "nope" }));
    assert_eq!(missing.pointer("/result/isError").unwrap(), true);
}

#[test]
fn run_confirm_routes_through_console_approval_inbox() {
    // No elicitation capability, native dialog disabled, but a fresh
    // Console heartbeat exists → the confirm must go through the
    // approval inbox. The test plays the Console: it watches pending/
    // and declines the item.
    let mut c = McpClient::spawn(&[("CORI_MCP_DISABLE_NATIVE_CONFIRM", "1")]);

    // Fake a live Console.
    let state_dir = c.home_path.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("console.heartbeat"), "test").unwrap();

    c.initialize(false);
    c.send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "run", "arguments": { "source": examples_dir("code_only") } }
    }));

    // Wait for the pending item to appear, then decline it like the
    // Console would (decided/<nonce>.json + retire the pending file).
    let pending_dir = c.home_path.join("approvals").join("pending");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let pending_file = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "no approval item appeared in {}",
            pending_dir.display()
        );
        if let Ok(entries) = std::fs::read_dir(&pending_dir)
            && let Some(f) = entries
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        {
            break f;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let req: JsonValue =
        serde_json::from_slice(&std::fs::read(&pending_file).unwrap()).unwrap();
    assert_eq!(req["kind"], "run_confirm");
    assert_eq!(req["requested_by"], "mcp");
    assert!(req["message"].as_str().unwrap().contains("Run Cori workflow"));
    assert_eq!(req["payload"]["dry_run"], false);

    let nonce = req["nonce"].as_str().unwrap();
    let decided_dir = c.home_path.join("approvals").join("decided");
    std::fs::create_dir_all(&decided_dir).unwrap();
    std::fs::write(
        decided_dir.join(format!("{nonce}.json")),
        serde_json::to_vec(&json!({
            "nonce": nonce,
            "decision": "declined",
            "decided_at": chrono::Utc::now(),
            "via": "console",
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&pending_file).unwrap();

    let resp = c.recv_response(2);
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/status").unwrap(),
        "not_run"
    );
    assert!(!c.home_path.join("runs").exists(), "nothing was executed");
}

#[test]
fn show_inspects_a_local_workflow_offline() {
    let mut c = McpClient::spawn(&[]);
    c.initialize(false);
    let resp = c.call_tool(2, "show", json!({ "source": examples_dir("code_only") }));
    assert_eq!(resp.pointer("/result/isError").unwrap(), false);
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert!(sc["manifest"].is_object());
    assert!(
        sc["steps"].as_array().is_some_and(|s| !s.is_empty()),
        "code_only example must expose steps"
    );
    assert!(
        sc["consent_required"].is_null(),
        "local paths never need consent"
    );
}

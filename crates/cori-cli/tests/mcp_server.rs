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
        [
            // Read/execute: the CLI-verb subset.
            "check",
            "run",
            "show",
            "runs_list",
            "runs_show",
            "status",
            // Authoring: journalled, session-scoped editing.
            "workflow_create",
            "workflow_open",
            "workflow_write_file",
            "workflow_delete_file",
            "workflow_rename_step",
            "conventions",
            "capabilities",
            "request_input",
            "request_approval",
            "propose",
            "publish",
            "revert",
            "session_status",
            "session_stop",
            "session_rewind",
        ],
        "tools must be exactly the CLI-verb subset plus the authoring surface"
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
    let big_output = json!({ "rows": vec!["x".repeat(64); 200] });
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
        result: Some(cori_protocol::ResolvedResult {
            headline: "200 rows ready".into(),
            description: None,
            fields: vec![],
            sections: vec![],
            artifacts: vec![],
            issues: vec![],
        }),
        activities: vec![cori_protocol::ActivityTrace {
            activity_id: "01_bulk".into(),
            step_name: "bulk".into(),
            kind: cori_protocol::StepKind::Code,
            status: "ok".into(),
            started_at: now,
            ended_at: now,
            duration_ms: 1,
            attempts: 1,
            route: None,
            task_queue: None,
            worker_identity: None,
            input_summary: json!(null),
            output_summary: json!({ "rows": 200 }),
            output: big_output,
            cost_eur: None,
            tokens: None,
            error: None,
            notes: None,
        }],
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
    assert_eq!(rows[0]["result_headline"], "200 rows ready");

    // Default: bulky output elided, summary intact, fetch hint present.
    let shown = c.call_tool(3, "runs_show", json!({ "run_id": "run-test-0001" }));
    assert_eq!(shown.pointer("/result/isError").unwrap(), false);
    assert_eq!(
        shown.pointer("/result/structuredContent/status").unwrap(),
        "succeeded"
    );
    assert_eq!(
        shown
            .pointer("/result/structuredContent/result/headline")
            .unwrap(),
        "200 rows ready"
    );
    assert_eq!(
        shown
            .pointer("/result/structuredContent/activities/0/output/_elided")
            .unwrap(),
        true,
        "large outputs must be elided by default"
    );
    assert_eq!(
        shown
            .pointer("/result/structuredContent/activities/0/output_summary/rows")
            .unwrap(),
        200,
        "summaries survive the trim"
    );

    // Per-activity fetch returns the full output.
    let one = c.call_tool(
        4,
        "runs_show",
        json!({ "run_id": "run-test-0001", "activity": "bulk" }),
    );
    assert!(
        one.pointer("/result/structuredContent/output/rows/0")
            .is_some(),
        "activity fetch returns full output"
    );

    // full: true returns everything inline.
    let full = c.call_tool(
        5,
        "runs_show",
        json!({ "run_id": "run-test-0001", "full": true }),
    );
    assert!(
        full.pointer("/result/structuredContent/activities/0/output/rows/0")
            .is_some(),
        "full:true bypasses the trim"
    );

    let missing = c.call_tool(6, "runs_show", json!({ "run_id": "nope" }));
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
    let req: JsonValue = serde_json::from_slice(&std::fs::read(&pending_file).unwrap()).unwrap();
    assert_eq!(req["kind"], "run_confirm");
    assert_eq!(req["requested_by"], "mcp");
    assert!(
        req["message"]
            .as_str()
            .unwrap()
            .contains("Run Cori workflow")
    );
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

// ---------------------------------------------------------------------------
// Authoring surface
// ---------------------------------------------------------------------------

const VALID_CODE_STEP: &str = r#"import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({}).passthrough();
const Output = z.object({ n: z.number() });

export default step.code({
  description: "Count one thing",
  input: Input,
  output: Output,
  run: () => ({ n: 1 }),
});
"#;

/// Spawn a client whose authoring root is a scratch directory outside
/// the real user home (`CORI_AUTHORING_ROOTS` is the test seam).
fn spawn_authoring() -> (McpClient, tempfile::TempDir) {
    let work = tempfile::tempdir().expect("workdir");
    let c = McpClient::spawn(&[(
        "CORI_AUTHORING_ROOTS",
        work.path().to_str().expect("utf8 tmpdir"),
    )]);
    (c, work)
}

#[test]
fn authoring_create_write_rename_delete_rewind_stop_roundtrip() {
    let (mut c, work) = spawn_authoring();
    c.initialize(false);

    // Create: scaffold + session + conventions inline.
    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Count Products",
            "goal": "Count products per universe from the export."
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap().clone();
    let session_id = sc["session_id"].as_str().expect("session id").to_string();
    assert!(session_id.starts_with("ses_"));
    let wf_dir = std::path::PathBuf::from(sc["workflow_dir"].as_str().unwrap());
    assert_eq!(
        wf_dir,
        work.path().join("count_products").canonicalize().unwrap(),
        "workflow_dir is canonical from birth"
    );
    let written: Vec<&str> = sc["files_written"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(written.contains(&"manifest.md"));
    assert!(written.contains(&"deno.json"));
    assert!(written.contains(&"tests/assert.ts"));
    assert!(
        sc["conventions"]["house_rules"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "conventions returned inline"
    );
    assert!(wf_dir.join("steps").is_dir());

    // Write a valid step: sha comes back, no lint warnings.
    let resp = c.call_tool(
        3,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_count.ts",
            "content": VALID_CODE_STEP,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sha = resp
        .pointer("/result/structuredContent/sha256")
        .and_then(|s| s.as_str())
        .expect("sha256")
        .to_string();
    assert_eq!(
        resp.pointer("/result/structuredContent/warnings").unwrap(),
        &json!([]),
        "a valid folder lints clean"
    );

    // Optimistic concurrency: a stale sha refuses and returns current truth.
    let resp = c.call_tool(
        4,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_count.ts",
            "content": "// clobber",
            "expect_sha": "0000000000000000000000000000000000000000000000000000000000000000",
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "stale_file"
    );
    assert_eq!(
        resp.pointer("/result/structuredContent/current_sha")
            .unwrap(),
        &json!(sha),
    );
    assert_eq!(
        std::fs::read_to_string(wf_dir.join("steps/01_count.ts")).unwrap(),
        VALID_CODE_STEP,
        "nothing was written on stale"
    );

    // Rename: the orphan-file class of bug, killed. Old name gone, new
    // name present, nothing else claiming step 01.
    let resp = c.call_tool(
        5,
        "workflow_rename_step",
        json!({
            "session_id": session_id,
            "from_rel_path": "steps/01_count.ts",
            "to_rel_path": "steps/01_count_per_universe.ts",
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    assert!(!wf_dir.join("steps/01_count.ts").exists(), "no orphan left");
    assert!(wf_dir.join("steps/01_count_per_universe.ts").is_file());

    // Delete: the half that was missing from the write-only bridge.
    let resp = c.call_tool(
        6,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/02_scratch.ts",
            "content": VALID_CODE_STEP,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false);
    let resp = c.call_tool(
        7,
        "workflow_delete_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/02_scratch.ts",
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false);
    assert!(!wf_dir.join("steps/02_scratch.ts").exists());

    // Rewind to the session's very start: every journalled write undone.
    let resp = c.call_tool(
        8,
        "session_rewind",
        json!({
            "session_id": session_id, "to_seq": 1,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    assert!(
        resp.pointer("/result/structuredContent/files_restored")
            .and_then(|f| f.as_array())
            .is_some_and(|f| !f.is_empty())
    );
    assert!(!wf_dir.join("manifest.md").exists(), "scaffold undone");
    assert!(!wf_dir.join("steps/01_count_per_universe.ts").exists());

    // Stop: mutations refuse with the reason, status reports the state.
    let resp = c.call_tool(
        9,
        "session_stop",
        json!({
            "session_id": session_id, "reason": "review finished",
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false);
    let resp = c.call_tool(
        10,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_again.ts",
            "content": "// nope",
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "session_stopped"
    );
    let resp = c.call_tool(11, "session_status", json!({ "session_id": session_id }));
    assert_eq!(
        resp.pointer("/result/structuredContent/state").unwrap(),
        "stopped"
    );
}

#[test]
fn authoring_open_reports_tree_and_refuses_bad_roots() {
    let (mut c, work) = spawn_authoring();
    c.initialize(false);

    // Refused: outside every allowed root.
    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": "/System/Library",
            "name": "Nope",
            "goal": "should be refused"
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "allowed_roots"
    );

    // Refused: inside Cori's own state, even when a root covers it.
    let inside_cori = c.home_path.join("workflows");
    let resp = c.call_tool(
        3,
        "workflow_create",
        json!({
            "target_dir": inside_cori.to_str().unwrap(),
            "name": "Nope",
            "goal": "should be refused"
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true, "{resp}");

    // Create then reopen: the tree carries per-file truth.
    let resp = c.call_tool(
        4,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Reopen Me",
            "goal": "roundtrip"
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let wf_dir = resp
        .pointer("/result/structuredContent/workflow_dir")
        .and_then(|d| d.as_str())
        .unwrap()
        .to_string();

    let resp = c.call_tool(5, "workflow_open", json!({ "path": wf_dir }));
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert!(sc["session_id"].as_str().unwrap().starts_with("ses_"));
    assert_eq!(sc["live"], json!(false));
    assert_eq!(sc["version"], json!(1));
    let tree = sc["tree"].as_array().unwrap();
    let manifest_row = tree
        .iter()
        .find(|r| r["rel_path"] == "manifest.md")
        .expect("manifest in tree");
    assert_eq!(
        manifest_row["sha256"].as_str().unwrap().len(),
        64,
        "tree rows carry sha256"
    );
    assert!(
        sc["manifest"]["id"] == json!("reopen_me"),
        "manifest parsed: {sc}"
    );
}

#[test]
fn request_input_answered_via_elicitation() {
    let (mut c, work) = spawn_authoring();
    c.initialize(true); // client declares elicitation

    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Ask Me",
            "goal": "exercise request_input"
        }),
    );
    let session_id = resp
        .pointer("/result/structuredContent/session_id")
        .and_then(|s| s.as_str())
        .expect("session id")
        .to_string();

    // The call blocks on a server→client elicitation carrying the
    // question and the options as an enum schema.
    c.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "request_input", "arguments": {
            "session_id": session_id,
            "question": "Parameter or hardcode?",
            "options": ["parameter", "hardcode"],
            "default": "parameter"
        } }
    }));
    let elicit = c.recv_server_request("elicitation/create");
    assert_eq!(
        elicit.pointer("/params/message").unwrap(),
        "Parameter or hardcode?"
    );
    assert_eq!(
        elicit
            .pointer("/params/requestedSchema/properties/answer/enum/0")
            .unwrap(),
        "parameter"
    );

    // Answer like a client would.
    let elicit_id = elicit["id"].clone();
    c.send(json!({
        "jsonrpc": "2.0", "id": elicit_id,
        "result": { "action": "accept", "content": { "answer": "hardcode" } }
    }));
    let resp = c.recv_response(3);
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["answer"], "hardcode");
    assert_eq!(sc["by"], "elicitation");
    assert!(sc["ts"].is_string());

    // `secret` is refused, never collected.
    let resp = c.call_tool(
        4,
        "request_input",
        json!({
            "session_id": session_id,
            "question": "API key?",
            "kind": "secret"
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "not_supported"
    );
}

#[test]
fn request_approval_denied_via_inbox_carries_note_and_ledger_id() {
    let (mut c, work) = spawn_authoring();

    // Fake a live Console; no elicitation, native dialogs disabled via
    // the client env below is not needed since the inbox rung precedes
    // the dialog rung.
    let state_dir = c.home_path.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("console.heartbeat"), "test").unwrap();

    c.initialize(false);
    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Ship It",
            "goal": "exercise request_approval"
        }),
    );
    let session_id = resp
        .pointer("/result/structuredContent/session_id")
        .and_then(|s| s.as_str())
        .expect("session id")
        .to_string();

    c.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "request_approval", "arguments": {
            "session_id": session_id,
            "action": "publish",
            "summary": "Publish ship_it v1 (2 steps, no external effects).",
            "effects_diff": { "added": [], "removed": [], "unchanged": [] }
        } }
    }));

    // Play the Console: find the pending item, deny it with a note.
    let pending_dir = c.home_path.join("approvals").join("pending");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let pending_file = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "no approval item appeared"
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
    let req: JsonValue = serde_json::from_slice(&std::fs::read(&pending_file).unwrap()).unwrap();
    assert_eq!(req["kind"], "agent_approval");
    assert_eq!(req["payload"]["action"], "publish");
    assert_eq!(req["payload"]["effects_diff"]["added"], json!([]));

    let nonce = req["nonce"].as_str().unwrap();
    let decided_dir = c.home_path.join("approvals").join("decided");
    std::fs::create_dir_all(&decided_dir).unwrap();
    std::fs::write(
        decided_dir.join(format!("{nonce}.json")),
        serde_json::to_vec(&json!({
            "nonce": nonce,
            "decision": "declined",
            "decided_at": chrono::Utc::now().to_rfc3339(),
            "via": "console",
            "response": { "note": "add a test for step 02 first" }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&pending_file).unwrap();

    // A denial is information, not an error.
    let resp = c.recv_response(3);
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["granted"], false);
    assert_eq!(sc["by"], "console");
    assert_eq!(sc["scope"], "once");
    assert_eq!(sc["note"], "add a test for step 02 first");
    assert_eq!(sc["ledger_id"], json!(nonce));
}

#[test]
fn publish_gated_then_ships_and_revert_restores() {
    let (mut c, work) = spawn_authoring();
    c.initialize(true); // elicitation answers the approval

    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Release Train",
            "goal": "exercise publish and revert"
        }),
    );
    let session_id = resp
        .pointer("/result/structuredContent/session_id")
        .and_then(|s| s.as_str())
        .expect("session id")
        .to_string();
    let wf_dir = std::path::PathBuf::from(
        resp.pointer("/result/structuredContent/workflow_dir")
            .and_then(|d| d.as_str())
            .unwrap(),
    );

    // Not compiling (no steps yet) → check_failed, before any approval.
    let resp = c.call_tool(3, "publish", json!({ "session_id": session_id }));
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "check_failed"
    );

    // Make it compile, then publish without approval → approval_required.
    let resp = c.call_tool(
        4,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_count.ts",
            "content": VALID_CODE_STEP,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let resp = c.call_tool(5, "publish", json!({ "session_id": session_id }));
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "approval_required"
    );

    // Grant via elicitation, then publish ships v2 (manifest was v1).
    c.send(json!({
        "jsonrpc": "2.0", "id": 6, "method": "tools/call",
        "params": { "name": "request_approval", "arguments": {
            "session_id": session_id,
            "action": "publish",
            "summary": "Publish release_train v2 (1 code step, no external effects)."
        } }
    }));
    let elicit = c.recv_server_request("elicitation/create");
    let elicit_id = elicit["id"].clone();
    c.send(json!({
        "jsonrpc": "2.0", "id": elicit_id,
        "result": { "action": "accept", "content": { "confirm": true } }
    }));
    let resp = c.recv_response(6);
    assert_eq!(
        resp.pointer("/result/structuredContent/granted").unwrap(),
        true
    );

    let resp = c.call_tool(
        7,
        "publish",
        json!({
            "session_id": session_id, "notes": "first cut"
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["version"], 2);
    assert_eq!(sc["previous_version"], 1);
    assert_eq!(sc["notified"]["schedules"], json!([]));
    let manifest = std::fs::read_to_string(wf_dir.join("manifest.md")).unwrap();
    assert!(manifest.contains("version: 2"), "manifest bumped");
    let versions_root = c.home_path.join("versions");
    assert!(versions_root.is_dir(), "snapshots live in ~/.cori/versions");

    // Publishing is terminal: the session stopped with the version in
    // its reason, and every further call through it is refused.
    let resp = c.call_tool(8, "session_status", json!({ "session_id": session_id }));
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["state"], "stopped", "{resp}");
    assert!(
        sc["stop_reason"].as_str().unwrap().contains("published v2"),
        "{resp}"
    );
    let resp = c.call_tool(9, "publish", json!({ "session_id": session_id }));
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "session_stopped"
    );

    // Drift the folder out-of-band, then revert to v2 restores the
    // published state.
    std::fs::write(wf_dir.join("steps/02_extra.ts"), VALID_CODE_STEP).unwrap();
    let resp = c.call_tool(
        10,
        "revert",
        json!({
            "path": wf_dir.to_str().unwrap(), "to_version": 2
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    assert_eq!(
        resp.pointer("/result/structuredContent/version").unwrap(),
        2
    );
    assert!(!wf_dir.join("steps/02_extra.ts").exists(), "drift removed");
    assert!(wf_dir.join("steps/01_count.ts").is_file());
    let manifest = std::fs::read_to_string(wf_dir.join("manifest.md")).unwrap();
    assert!(manifest.contains("version: 2"));

    // Reverting to a version that was never published names the options.
    let resp = c.call_tool(
        11,
        "revert",
        json!({
            "path": wf_dir.to_str().unwrap(), "to_version": 9
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "not_found"
    );
}

#[test]
fn propose_freezes_review_card_and_console_accept_publishes() {
    let (mut c, work) = spawn_authoring();
    c.initialize(false);

    let resp = c.call_tool(
        2,
        "workflow_create",
        json!({
            "target_dir": work.path().to_str().unwrap(),
            "name": "Review Train",
            "goal": "exercise the proposal flow"
        }),
    );
    let session_id = resp
        .pointer("/result/structuredContent/session_id")
        .and_then(|s| s.as_str())
        .expect("session id")
        .to_string();
    let wf_dir = std::path::PathBuf::from(
        resp.pointer("/result/structuredContent/workflow_dir")
            .and_then(|d| d.as_str())
            .unwrap(),
    );

    // Not compiling (no steps yet) → check_failed, with the errors listed.
    let resp = c.call_tool(
        3,
        "propose",
        json!({ "session_id": session_id, "summary": "Too early." }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "check_failed"
    );

    // Make it compile, then propose freezes the per-step review card.
    let resp = c.call_tool(
        4,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_count.ts",
            "content": VALID_CODE_STEP,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let resp = c.call_tool(
        5,
        "propose",
        json!({
            "session_id": session_id,
            "summary": "Add a counting workflow with one pure code step."
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), false, "{resp}");
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["state"], "proposed");
    let steps = sc["proposal"]["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["activity_id"], "01_count");
    assert_eq!(steps[0]["kind"], "code");
    assert_eq!(steps[0]["access"], "none");
    assert_eq!(steps[0]["external"], false);
    assert_eq!(steps[0]["change"], "added");
    assert_eq!(sc["proposal"]["rollup"]["pure"], 1);

    // While proposed, every mutation is refused with the review pointer.
    let resp = c.call_tool(
        6,
        "workflow_write_file",
        json!({
            "session_id": session_id,
            "rel_path": "steps/01_count.ts",
            "content": VALID_CODE_STEP,
        }),
    );
    assert_eq!(resp.pointer("/result/isError").unwrap(), true);
    assert_eq!(
        resp.pointer("/result/structuredContent/error/code")
            .unwrap(),
        "session_proposed"
    );

    // The human accepts in the Console (same library call the Tauri
    // command makes), from a different process than the MCP server —
    // disk is the only truth. In-process env is safe here: no other
    // test in this binary reads CORI_HOME in-process, and every child
    // gets its CORI_HOME passed explicitly.
    // SAFETY: single writer; see above.
    unsafe { std::env::set_var("CORI_HOME", &c.home_path) };
    let out = cori_run::proposals::accept(&session_id, "console", None).expect("accept");
    unsafe { std::env::remove_var("CORI_HOME") };
    assert_eq!(out.version, 2);
    let manifest = std::fs::read_to_string(wf_dir.join("manifest.md")).unwrap();
    assert!(manifest.contains("version: 2"), "manifest bumped");

    let resp = c.call_tool(7, "session_status", json!({ "session_id": session_id }));
    let sc = resp.pointer("/result/structuredContent").unwrap();
    assert_eq!(sc["state"], "stopped");
    assert!(
        sc["stop_reason"].as_str().unwrap().contains("published v2"),
        "{sc}"
    );
}

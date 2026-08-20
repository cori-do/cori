//! MCP authoring tools — the write half of `cori mcp`.
//!
//! External agents hold the pen; the Console is the consent and audit
//! surface. Every mutation goes through a journalled session
//! (`cori_run::sessions`), so the Console can attribute, mirror, and
//! undo it. Spec: `docs/mcp-authoring-design.md`; the expansion of the
//! MCP surface beyond the read/execute subset was signed off 2026-08-16
//! (see AGENTS.md, "CLI surface").
//!
//! Design constraints inherited from the field notes:
//! - The bridge can delete and rename — a write-only bridge orphaned a
//!   renamed step file and produced a `duplicate step number` failure.
//! - Rules are returned as *data* (`conventions`) and enforced at write
//!   time (`warnings` on every write), not documented in prose.
//! - `capabilities` shares its readiness model with `check`/`run`.
//! - Nothing here simulates: a tool that could not do the thing says so
//!   with a structured `error.code` + `remedy`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value as JsonValue, json};
use sha2::Digest as _;

use cori_run::sessions;

/// Max bytes of file content echoed back in a `stale_file` result —
/// same rationale as `MAX_INLINE_OUTPUT` in `mcp.rs`.
const MAX_STALE_CONTENT: usize = 65_536;

/// Max files listed by `workflow_open`'s tree.
const MAX_TREE_FILES: usize = 500;

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> Vec<JsonValue> {
    vec![
        json!({
            "name": "workflow_create",
            "title": "Create a workflow folder",
            "description": "Create a new workflow folder (manifest.md, deno.json, \
                tests/assert.ts, empty steps/) and open a journalled authoring \
                session on it. Returns the authoring conventions inline — read \
                them before writing the first step. Refuses to write outside \
                the user's own directories and never writes into ~/.cori.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target_dir": { "type": "string", "description": "Absolute directory the workflow folder is created in" },
                    "name": { "type": "string", "description": "Workflow name; snake_cased for the folder and manifest id" },
                    "goal": { "type": "string", "description": "What the workflow does — becomes the manifest description and Goal prose" },
                    "base_on": { "type": "string", "description": "Optional path of an existing workflow folder to copy as the starting point" }
                },
                "required": ["target_dir", "name", "goal"]
            }
        }),
        json!({
            "name": "workflow_open",
            "title": "Open a workflow for editing",
            "description": "Start a journalled editing session on an existing local \
                workflow folder. Returns the manifest, the file tree with per-file \
                sha256 (edit from truth, not memory), and whether the workflow is \
                live (a schedule depends on it).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Local workflow folder path" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "workflow_write_file",
            "title": "Write one workflow file",
            "description": "Write a file (relative path) inside the session's workflow \
                folder. `expect_sha` enables optimistic concurrency: if the file \
                changed under you, nothing is written and the current content comes \
                back (`stale_file`). The response carries the same lint warnings \
                `check` would raise — fix them now, not three calls later.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "rel_path": { "type": "string", "description": "Path relative to the workflow folder, e.g. steps/01_fetch.ts" },
                    "content": { "type": "string" },
                    "expect_sha": { "type": "string", "description": "sha256 the caller believes the file currently has; omit for unconditional write" }
                },
                "required": ["session_id", "rel_path", "content"]
            }
        }),
        json!({
            "name": "workflow_delete_file",
            "title": "Delete one workflow file",
            "description": "Delete a file inside the session's workflow folder. \
                Required for clean renames — never leave an orphaned step file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "rel_path": { "type": "string" }
                },
                "required": ["session_id", "rel_path"]
            }
        }),
        json!({
            "name": "workflow_rename_step",
            "title": "Rename or renumber a step file",
            "description": "Atomic rename of a steps/NN_name.ts file, renumbering the \
                other steps to keep the sequence gapless. Use this instead of \
                write+delete: it makes orphaned step files structurally impossible.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "from_rel_path": { "type": "string", "description": "e.g. steps/01_download.ts" },
                    "to_rel_path": { "type": "string", "description": "e.g. steps/01_read_tab_values.ts" }
                },
                "required": ["session_id", "from_rel_path", "to_rel_path"]
            }
        }),
        json!({
            "name": "conventions",
            "title": "Authoring conventions as data",
            "description": "The canonical step templates per activity kind, the \
                manifest schema, the house rules `check` enforces, and the \
                zero-dependency test helper. Call before writing step files.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Optional workflow path (reserved for path-scoped rules)" }
            } }
        }),
        json!({
            "name": "capabilities",
            "title": "Capability readiness (three states)",
            "description": "Every capability with its state — `not_declared`, \
                `declared_unauthed`, or `ready` — and the literal remedy that \
                unblocks it. Same readiness source `check` and `run` use; never \
                invent a `cori login X` the machine doesn't know about.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        // The two blocking human-gate tools dispatch in `mcp.rs` (they
        // need the elicitation plumbing); they are defined here so the
        // authoring surface reads as one family.
        json!({
            "name": "request_input",
            "title": "Ask the human a question",
            "description": "Block until the human answers a short question the agent \
                genuinely can't infer (e.g. parameter or hardcode?). Renders as one \
                inline line in the Console, or an MCP elicitation. Times out after \
                10 minutes: with a `default` the default comes back (`timed_out: \
                true`), without one the timeout is an error. `secret` is not \
                supported — credentials never transit an MCP client.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "question": { "type": "string", "description": "One line, human voice" },
                    "options": { "type": "array", "items": { "type": "string" }, "description": "Render as a choice; omit for free text" },
                    "default": { "type": "string", "description": "Answer to proceed with on timeout" },
                    "kind": { "type": "string", "enum": ["choice", "text", "secret"] }
                },
                "required": ["session_id", "question"]
            }
        }),
        json!({
            "name": "request_approval",
            "title": "Ask permission for an irreversible action",
            "description": "Block until the human grants or denies an irreversible \
                action (publish, new external effect, new capability, schedule \
                change, live run). A denial returns granted: false with the human's \
                note — useful information, not an error; keep working, just don't \
                ship. Only irreversible things may request approval: reads, dry \
                runs, and code-only diffs must never call this.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["publish", "new_external_effect", "new_capability", "schedule_change", "run_live"] },
                    "summary": { "type": "string", "description": "One sentence, human voice" },
                    "effects_diff": { "type": "object", "description": "{added[], removed[], unchanged[]} of external effects" },
                    "evidence": { "type": "object", "description": "check / tests / replay results backing the request" },
                    "requested_scope": { "type": "string", "enum": ["once", "shape"], "description": "shape (standing consent) is not granted yet; scope always comes back once" }
                },
                "required": ["session_id", "action", "summary"]
            }
        }),
        json!({
            "name": "propose",
            "title": "Propose the session's work for human review",
            "description": "Submit everything this session did as a reviewable \
                proposal in the Console: one row per step (kind, effect target, \
                proven read/write access, external reach) joined with the file \
                changes, computed from the compiled DAG — never from the agent's \
                own claims. Gated on a green compile (`check_failed` otherwise). \
                The session enters `proposed`: every further mutation is refused \
                until the human accepts (publishes the next version) or rejects \
                (optionally discarding the session's changes). Call this when \
                authoring is done — do not leave the session open, and do not \
                pair it with request_approval(publish): acceptance is the \
                approval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "summary": { "type": "string", "description": "One sentence, human voice — what this change does and why" }
                },
                "required": ["session_id", "summary"]
            }
        }),
        json!({
            "name": "publish",
            "title": "Publish a workflow version",
            "description": "Snapshot the session's workflow folder as the next version. \
                Gated, non-negotiably: the folder must compile with every declared \
                capability resolvable (`check_failed` otherwise), and the session must \
                hold a granted request_approval(action: \"publish\") — one grant, one \
                publish (`approval_required` otherwise). Keeps the previous version \
                for `revert` and reports every schedule that will pick the new \
                version up. Cluster reachability is a run concern, not a publish gate. \
                Publishing ends the session — it is the terminal act of authoring, \
                like an accepted proposal. Open a new session with workflow_open to \
                edit further.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "notes": { "type": "string", "description": "Human release note, stored with the version" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "revert",
            "title": "Revert a workflow to a published version",
            "description": "Restore a workflow folder to a previously published \
                snapshot, through a journalled session so the restore itself is \
                attributable and undoable. See publish for how versions are kept.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workflow folder path" },
                    "to_version": { "type": "integer", "minimum": 1 }
                },
                "required": ["path", "to_version"]
            }
        }),
        json!({
            "name": "session_status",
            "title": "Authoring session status",
            "description": "State and journal tail of an authoring session.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "session_stop",
            "title": "Stop an authoring session",
            "description": "Terminate an authoring session; subsequent mutations are \
                refused with the reason. Normally pressed by the human in the \
                Console, but an agent may close its own session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "session_rewind",
            "title": "Rewind an authoring session",
            "description": "Undo the session's journalled file operations back to a \
                sequence number (see session_status). Exact, not git-approximate; \
                the undo itself is journalled.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "to_seq": { "type": "integer", "minimum": 0 }
                },
                "required": ["session_id", "to_seq"]
            }
        }),
    ]
}

/// Dispatch an authoring tool. `Ok(None)` = not an authoring tool.
pub fn dispatch(agent: &str, name: &str, args: &JsonValue) -> Result<Option<(JsonValue, bool)>> {
    let out = match name {
        "workflow_create" => tool_workflow_create(agent, args)?,
        "workflow_open" => tool_workflow_open(agent, args)?,
        "workflow_write_file" => tool_write_file(args)?,
        "workflow_delete_file" => tool_delete_file(args)?,
        "workflow_rename_step" => tool_rename_step(args)?,
        "conventions" => (conventions_json(), false),
        "capabilities" => (capabilities_json()?, false),
        "propose" => tool_propose(args)?,
        "publish" => tool_publish(args)?,
        "revert" => tool_revert(agent, args)?,
        "session_status" => tool_session_status(args)?,
        "session_stop" => tool_session_stop(args)?,
        "session_rewind" => tool_session_rewind(args)?,
        _ => return Ok(None),
    };
    Ok(Some(out))
}

// ---------------------------------------------------------------------------
// Structured errors
// ---------------------------------------------------------------------------

/// Errors an agent can act on: `code` to branch on, `message` for the
/// human, `remedy` when a human action would fix it.
pub(crate) fn err(code: &str, message: String, remedy: Option<&str>) -> (JsonValue, bool) {
    let mut e = json!({ "code": code, "message": message });
    if let Some(r) = remedy {
        e["remedy"] = json!(r);
    }
    (json!({ "error": e }), true)
}

fn arg_str(args: &JsonValue, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required argument `{key}`"))
}

fn arg_opt_str(args: &JsonValue, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// Load a session, mapping the two agent-actionable failure modes to
/// structured errors instead of a bare tool error. Also used by the
/// blocking `request_input` / `request_approval` tools in `mcp.rs`.
pub(crate) fn load_session(
    session_id: &str,
) -> std::result::Result<sessions::Session, (JsonValue, bool)> {
    let session = sessions::load(session_id).map_err(|e| {
        err(
            "unknown_session",
            format!("{e:#}"),
            Some("open the workflow again with workflow_open"),
        )
    })?;
    if session.state == sessions::SessionState::Stopped {
        return Err(err(
            "session_stopped",
            format!(
                "this session was stopped: {}",
                session
                    .stop_reason
                    .as_deref()
                    .unwrap_or("stopped by the user")
            ),
            Some("read the reason, then open a new session with workflow_open if appropriate"),
        ));
    }
    if session.state == sessions::SessionState::Proposed {
        return Err(err(
            "session_proposed",
            "this session's work is proposed and awaiting human review in the Console".into(),
            Some(
                "wait for the human to accept or reject; session_status shows the \
                 outcome (acceptance publishes, rejection carries the reason)",
            ),
        ));
    }
    Ok(session)
}

// ---------------------------------------------------------------------------
// Allowed roots
// ---------------------------------------------------------------------------

/// Where authoring may write: the user's home directory (plus any
/// `CORI_AUTHORING_ROOTS` entries — tests, unusual setups), and never
/// inside `~/.cori` — that is Cori's own state, not user files.
fn dir_allowed(path: &Path) -> std::result::Result<(), (JsonValue, bool)> {
    let canon = canonical_lexical(path);
    if let Ok(cori_home) = cori_run::paths::home() {
        let cori_home = canonical_lexical(&cori_home);
        if canon.starts_with(&cori_home) {
            return Err(err(
                "allowed_roots",
                format!("`{}` is inside Cori's own state directory", path.display()),
                Some("choose a folder in the user's workspace instead"),
            ));
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(extra) = std::env::var("CORI_AUTHORING_ROOTS") {
        roots.extend(std::env::split_paths(&extra).filter(|p| p.is_absolute()));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }
    if roots
        .iter()
        .any(|r| canon.starts_with(canonical_lexical(r)))
    {
        return Ok(());
    }
    Err(err(
        "allowed_roots",
        format!(
            "`{}` is outside the directories authoring may write to",
            path.display()
        ),
        Some("use a folder under the user's home directory"),
    ))
}

/// Canonicalize the deepest existing ancestor, then re-append the rest —
/// lets us compare paths that don't exist yet without symlink surprises.
fn canonical_lexical(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for part in tail.iter().rev() {
        out.push(part);
    }
    out
}

// ---------------------------------------------------------------------------
// Lint at write time
// ---------------------------------------------------------------------------

/// The same rules `check` enforces, surfaced on every write: compile
/// errors (as text) plus the advisory lints. Never fails the write —
/// mid-edit folders are legitimately broken.
fn lint_warnings(workflow_dir: &Path) -> Vec<String> {
    match cori_compiler::compile(workflow_dir) {
        Ok(compiled) => super::check::build_warnings(&compiled),
        Err(errors) => errors
            .iter()
            .map(|e| {
                let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
                format!("{}: {}{}", e.file, e.reason, line)
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// workflow_create
// ---------------------------------------------------------------------------

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut last_us = true; // suppress leading underscores
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    let out = out.trim_end_matches('_').to_string();
    // Manifest ids must start with a letter.
    out.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
        .chars()
        .take(64)
        .collect()
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

const DENO_JSON_TEMPLATE: &str = r#"{
  "imports": {
    "@cori-do/sdk": "npm:@cori-do/sdk@^0.2.4",
    "zod": "npm:zod@^4.4.3"
  },
  "tasks": {
    "test": "deno test --no-check --allow-read --allow-env --allow-net=registry.npmjs.org,esm.sh,jsr.io tests/"
  }
}
"#;

const ASSERT_TS_TEMPLATE: &str = r#"// tests/assert.ts — self-contained; no network needed to run tests.
export function assertEquals(actual: unknown, expected: unknown, msg?: string) {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a !== e) throw new Error(msg ?? `assertEquals failed:\n  actual:   ${a}\n  expected: ${e}`);
}
export function assert(cond: unknown, msg = "assertion failed") {
  if (!cond) throw new Error(msg);
}
"#;

fn tool_workflow_create(agent: &str, args: &JsonValue) -> Result<(JsonValue, bool)> {
    let target_dir = PathBuf::from(arg_str(args, "target_dir")?);
    let name = arg_str(args, "name")?;
    let goal = arg_str(args, "goal")?;
    let base_on = arg_opt_str(args, "base_on");

    if !target_dir.is_absolute() {
        return Ok(err(
            "bad_request",
            "target_dir must be an absolute path".into(),
            None,
        ));
    }
    let id = snake_case(&name);
    if id.is_empty() {
        return Ok(err(
            "invalid_name",
            format!("`{name}` does not yield a usable snake_case id"),
            Some("pick a name starting with a letter"),
        ));
    }
    let workflow_dir = target_dir.join(&id);
    if let Err(e) = dir_allowed(&workflow_dir) {
        return Ok(e);
    }
    if workflow_dir.exists() {
        return Ok(err(
            "dir_exists",
            format!("`{}` already exists", workflow_dir.display()),
            Some("open it with workflow_open, or pick another name"),
        ));
    }

    std::fs::create_dir_all(workflow_dir.join("steps"))
        .with_context(|| format!("creating `{}`", workflow_dir.display()))?;
    // Canonical from birth: schedules, versions, and the Console all
    // compare this path against canonicalized ones.
    let workflow_dir = workflow_dir.canonicalize().unwrap_or(workflow_dir);
    let session = sessions::create(agent, &workflow_dir, None)?;
    let sid = session.session_id.clone();

    let mut files_written: Vec<String> = Vec::new();
    let mut write = |rel: &str, content: &str| -> Result<()> {
        sessions::write_file(&sid, rel, content, None)?;
        files_written.push(rel.to_string());
        Ok(())
    };

    // base_on: copy an existing folder as the starting point.
    if let Some(base) = &base_on {
        let base = PathBuf::from(base);
        if !base.join("manifest.md").is_file() {
            let _ = std::fs::remove_dir_all(&workflow_dir);
            return Ok(err(
                "bad_request",
                format!(
                    "`{}` is not a workflow folder (no manifest.md)",
                    base.display()
                ),
                None,
            ));
        }
        for rel in list_files(&base, usize::MAX)? {
            let content = std::fs::read(base.join(&rel))?;
            write(&rel, &String::from_utf8_lossy(&content))?;
        }
    }

    // The scaffold (after base_on, so a copied manifest is replaced by
    // one carrying this workflow's identity).
    let description: String = goal
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    let manifest = format!(
        "---\nid: {id}\nname: {name}\ndescription: {description}\ncreated: {today}\nversion: 1\nparameters: []\ntools_required: []\nmcp_servers: []\ntags: []\n---\n\n# {raw_name}\n\n## Goal\n\n{goal}\n",
        name = yaml_quote(&name),
        description = yaml_quote(&description),
        today = chrono::Utc::now().format("%Y-%m-%d"),
        raw_name = name,
    );
    write("manifest.md", &manifest)?;
    if !workflow_dir.join("deno.json").is_file() {
        write("deno.json", DENO_JSON_TEMPLATE)?;
    }
    if !workflow_dir.join("tests/assert.ts").is_file() {
        write("tests/assert.ts", ASSERT_TS_TEMPLATE)?;
    }

    Ok((
        json!({
            "session_id": sid,
            "workflow_dir": workflow_dir.display().to_string(),
            "files_written": files_written,
            "conventions": conventions_json(),
        }),
        false,
    ))
}

// ---------------------------------------------------------------------------
// workflow_open
// ---------------------------------------------------------------------------

fn list_files(dir: &Path, cap: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue; // .git, editor droppings, our tmp files
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(dir) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
                if out.len() >= cap {
                    return Ok(out);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

fn tool_workflow_open(agent: &str, args: &JsonValue) -> Result<(JsonValue, bool)> {
    let path = PathBuf::from(arg_str(args, "path")?);
    if !path.is_dir() {
        return Ok(err(
            "not_found",
            format!("`{}` is not a directory on this machine", path.display()),
            Some("authoring edits local folders; clone or copy the workflow locally first"),
        ));
    }
    let path = path.canonicalize().unwrap_or(path);
    if let Err(e) = dir_allowed(&path) {
        return Ok(e);
    }
    if !path.join("manifest.md").is_file() {
        return Ok(err(
            "not_found",
            format!(
                "`{}` has no manifest.md — not a workflow folder",
                path.display()
            ),
            Some("create a new workflow with workflow_create"),
        ));
    }

    // Manifest: parsed when valid, carried as an error otherwise —
    // agents legitimately open broken workflows to fix them.
    let manifest_src = std::fs::read_to_string(path.join("manifest.md"))?;
    let (manifest_json, version) = match cori_manifest::parse_manifest(&manifest_src) {
        Ok(m) => {
            let v = m.version;
            (serde_json::to_value(&m)?, Some(v))
        }
        Err(errors) => (
            json!({ "invalid": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>() }),
            None,
        ),
    };

    // Tree with per-file truth (sha + line count).
    let files = list_files(&path, MAX_TREE_FILES)?;
    let truncated = files.len() >= MAX_TREE_FILES;
    let tree: Vec<JsonValue> = files
        .iter()
        .filter_map(|rel| {
            let bytes = std::fs::read(path.join(rel)).ok()?;
            Some(json!({
                "rel_path": rel,
                "sha256": hex::encode(sha2::Sha256::digest(&bytes)),
                "lines": String::from_utf8_lossy(&bytes).lines().count(),
            }))
        })
        .collect();

    // Steps summary + current lint state.
    let (steps, warnings) = match cori_compiler::compile(&path) {
        Ok(compiled) => (
            compiled
                .steps
                .iter()
                .map(|s| json!({ "activity_id": s.activity_id, "name": s.name }))
                .collect::<Vec<_>>(),
            super::check::build_warnings(&compiled),
        ),
        Err(errors) => (
            Vec::new(),
            errors
                .iter()
                .map(|e| format!("{}: {}", e.file, e.reason))
                .collect(),
        ),
    };

    // Live = a schedule fires this folder.
    let schedules: Vec<JsonValue> = cori_run::schedules::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            Path::new(&s.source)
                .canonicalize()
                .map(|p| p == path)
                .unwrap_or(false)
        })
        .map(|s| json!({ "id": s.id, "schedule": s.schedule, "enabled": s.enabled }))
        .collect();
    let live = !schedules.is_empty();

    let session = sessions::create(agent, &path, version)?;
    let mut result = json!({
        "session_id": session.session_id,
        "workflow_dir": path.display().to_string(),
        "manifest": manifest_json,
        "steps": steps,
        "tree": tree,
        "version": version,
        "live": live,
        "schedules": schedules,
        "warnings": warnings,
    });
    if truncated {
        result["tree_truncated"] = json!(true);
    }
    if live {
        result["live_note"] = json!(
            "a schedule depends on this folder and draft-version isolation is not \
             built yet — edits land on the live version, so keep `check` green and \
             prefer small, complete changes"
        );
    }
    Ok((result, false))
}

// ---------------------------------------------------------------------------
// Write / delete / rename
// ---------------------------------------------------------------------------

fn tool_write_file(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let rel_path = arg_str(args, "rel_path")?;
    let content = arg_str(args, "content")?;
    let expect_sha = arg_opt_str(args, "expect_sha");
    let session = match load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(e),
    };
    match sessions::write_file(&session_id, &rel_path, &content, expect_sha.as_deref())? {
        sessions::WriteResult::Written { sha256, lines } => Ok((
            json!({
                "sha256": sha256,
                "lines": lines,
                "warnings": lint_warnings(&session.workflow_dir),
            }),
            false,
        )),
        sessions::WriteResult::Stale {
            current_sha,
            current_content,
        } => {
            let mut e = json!({
                "error": {
                    "code": "stale_file",
                    "message": format!(
                        "`{rel_path}` changed since the expected sha — the human or \
                         another session may have taken the pen; nothing was written"
                    ),
                },
                "current_sha": current_sha,
            });
            if let Some(content) = current_content {
                let truncated = content.len() > MAX_STALE_CONTENT;
                let shown: String = content.chars().take(MAX_STALE_CONTENT).collect();
                e["current_content"] = json!(shown);
                if truncated {
                    e["current_content_truncated"] = json!(true);
                }
            }
            Ok((e, true))
        }
    }
}

fn tool_delete_file(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let rel_path = arg_str(args, "rel_path")?;
    let session = match load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(e),
    };
    let abs = match sessions::resolve_rel(&session, &rel_path) {
        Ok(p) => p,
        Err(e) => return Ok(err("bad_request", format!("{e:#}"), None)),
    };
    if !abs.is_file() {
        return Ok(err(
            "not_found",
            format!("no file `{rel_path}` in the workflow folder"),
            None,
        ));
    }
    sessions::delete_file(&session_id, &rel_path)?;
    Ok((
        json!({ "deleted": true, "warnings": lint_warnings(&session.workflow_dir) }),
        false,
    ))
}

/// `steps/NN_snake_case.ts` — must agree with the compiler's regex.
fn parse_step_rel(rel: &str) -> Option<(u32, String)> {
    let name = rel.strip_prefix("steps/")?;
    if name.contains('/') {
        return None;
    }
    let re = regex::Regex::new(r"^(\d+)_([a-z][a-z0-9_]*)\.ts$").expect("static regex");
    let caps = re.captures(name)?;
    Some((caps[1].parse().ok()?, caps[2].to_string()))
}

fn step_rel(number: u32, name: &str) -> String {
    format!("steps/{number:02}_{name}.ts")
}

fn tool_rename_step(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let from = arg_str(args, "from_rel_path")?;
    let to = arg_str(args, "to_rel_path")?;
    let session = match load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(e),
    };

    let Some((_, _)) = parse_step_rel(&from) else {
        return Ok(err(
            "invalid_step_path",
            format!("`{from}` is not a steps/NN_snake_case.ts path"),
            None,
        ));
    };
    let Some((to_num, to_name)) = parse_step_rel(&to) else {
        return Ok(err(
            "invalid_step_path",
            format!("`{to}` is not a steps/NN_snake_case.ts path"),
            None,
        ));
    };
    if !session.workflow_dir.join(&from).is_file() {
        return Ok(err("not_found", format!("no file `{from}`"), None));
    }
    if from == to {
        return Ok(err("bad_request", "from and to are identical".into(), None));
    }

    // Current steps, minus the renamed one.
    let mut others: Vec<(u32, String)> = list_files(&session.workflow_dir, usize::MAX)?
        .into_iter()
        .filter(|rel| rel != &from)
        .filter_map(|rel| parse_step_rel(&rel))
        .collect();
    others.sort();
    if others.iter().any(|(_, n)| *n == to_name) {
        return Ok(err(
            "to_exists",
            format!("a step named `{to_name}` already exists"),
            Some("pick a different name or delete the other step first"),
        ));
    }

    // The renamed step takes its target slot; everything renumbers to a
    // gapless 01..N sequence around it.
    let start = others.first().map(|(n, _)| (*n).min(1)).unwrap_or(1);
    let insert_at = others
        .iter()
        .position(|(n, _)| *n >= to_num)
        .unwrap_or(others.len());
    let mut ordered = others;
    ordered.insert(insert_at, (to_num, to_name.clone()));

    let mut moves: Vec<(String, String)> = Vec::new();
    let mut renumbered: Vec<JsonValue> = Vec::new();
    for (i, (old_num, name)) in ordered.iter().enumerate() {
        let new_num = start + i as u32;
        let old_rel = if *name == to_name {
            from.clone()
        } else {
            step_rel(*old_num, name)
        };
        let new_rel = step_rel(new_num, name);
        if old_rel != new_rel {
            moves.push((old_rel.clone(), new_rel.clone()));
            if *name != to_name {
                renumbered.push(json!({ "from": old_rel, "to": new_rel }));
            }
        }
    }
    sessions::apply_renames(&session_id, &moves)?;

    let final_to = step_rel(start + insert_at as u32, &to_name);
    Ok((
        json!({
            "renamed": { "from": from, "to": final_to },
            "renumbered": renumbered,
            "warnings": lint_warnings(&session.workflow_dir),
        }),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Sessions: status / stop / rewind
// ---------------------------------------------------------------------------

fn tool_session_status(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let session = match sessions::load(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(err("unknown_session", format!("{e:#}"), None)),
    };
    let events = sessions::events(&session_id)?;
    let tail: Vec<JsonValue> = events
        .iter()
        .rev()
        .take(20)
        .rev()
        .map(|e| serde_json::to_value(e).unwrap_or(JsonValue::Null))
        .collect();
    Ok((
        json!({
            "session_id": session.session_id,
            "agent": session.agent,
            "workflow_dir": session.workflow_dir.display().to_string(),
            "state": session.state,
            "stop_reason": session.stop_reason,
            "current_seq": session.next_seq.saturating_sub(1),
            "events_tail": tail,
        }),
        false,
    ))
}

fn tool_session_stop(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let reason = arg_opt_str(args, "reason");
    match sessions::stop(&session_id, reason.as_deref()) {
        Ok(_) => Ok((json!({ "stopped": true }), false)),
        Err(e) => Ok(err("unknown_session", format!("{e:#}"), None)),
    }
}

fn tool_session_rewind(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let to_seq = args
        .get("to_seq")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("missing required argument `to_seq`"))?;
    if let Err(e) = load_session(&session_id) {
        return Ok(e);
    }
    let (files_restored, current_seq) = sessions::rewind(&session_id, to_seq)?;
    Ok((
        json!({ "files_restored": files_restored, "current_seq": current_seq }),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Ship and undo: publish / revert
// ---------------------------------------------------------------------------

/// The schedules that fire this folder — they pick up whatever the
/// folder holds, so publish reports them and workflow_open flags `live`.
fn schedules_for(dir: &Path) -> Vec<String> {
    cori_run::schedules::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            Path::new(&s.source)
                .canonicalize()
                .map(|p| p == dir)
                .unwrap_or(false)
        })
        .map(|s| s.id)
        .collect()
}

fn tool_propose(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let summary = arg_str(args, "summary")?;
    let session = match load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(e),
    };

    // Gate — a proposal only exists for a folder that compiles; the
    // reviewer must never be asked to consent to a broken workflow.
    let compiled = match cori_compiler::compile(&session.workflow_dir) {
        Ok(c) => c,
        Err(errors) => {
            return Ok((
                json!({
                    "error": { "code": "check_failed", "message": "the workflow does not compile — fix the errors, then propose again" },
                    "errors": errors.iter().map(|e| {
                        let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
                        format!("{}: {}{}", e.file, e.reason, line)
                    }).collect::<Vec<_>>(),
                }),
                true,
            ));
        }
    };
    let warnings = super::check::build_warnings(&compiled);
    let proposal = cori_run::proposals::submit(&session_id, &summary, &compiled, warnings)?;
    Ok((
        json!({
            "state": "proposed",
            "proposal": serde_json::to_value(&proposal)?,
            "next": "a human reviews this in the Cori Console; mutations are \
                     refused until they accept (which publishes the next \
                     version) or reject (session_status carries the outcome)",
        }),
        false,
    ))
}

fn tool_publish(args: &JsonValue) -> Result<(JsonValue, bool)> {
    let session_id = arg_str(args, "session_id")?;
    let notes = arg_opt_str(args, "notes");
    let session = match load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return Ok(e),
    };
    let dir = session.workflow_dir.clone();

    // Gate 1 — green check, the parts publishing needs: the folder
    // compiles clean and every declared capability resolves on this
    // machine. Cluster reachability gates runs, not versioning.
    let compiled = match cori_compiler::compile(&dir) {
        Ok(c) => c,
        Err(errors) => {
            return Ok((
                json!({
                    "error": { "code": "check_failed", "message": "the workflow does not compile" },
                    "errors": errors.iter().map(|e| {
                        let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
                        format!("{}: {}{}", e.file, e.reason, line)
                    }).collect::<Vec<_>>(),
                }),
                true,
            ));
        }
    };
    let warnings = super::check::build_warnings(&compiled);
    let pf = cori_run::preflight(&dir.display().to_string(), false, false)?;
    if !pf.missing_caps.is_empty() {
        return Ok((
            json!({
                "error": { "code": "check_failed", "message": "declared capabilities are not ready on this machine" },
                "missing_capabilities": pf.missing_caps,
            }),
            true,
        ));
    }

    // Gate 2 — a granted publish approval, consumed on use.
    let Some(grant) = sessions::take_approval(&session_id, "publish")? else {
        return Ok(err(
            "approval_required",
            "publishing is irreversible and needs a granted approval for this session".into(),
            Some(
                "call request_approval with action \"publish\", the one-sentence summary, \
                 and the effects diff; publish again once granted",
            ),
        ));
    };

    // Version numbers come from the manifest (the human-visible truth).
    let manifest_rel = "manifest.md";
    let manifest_src = std::fs::read_to_string(dir.join(manifest_rel))?;
    let current_version = match cori_manifest::parse_manifest(&manifest_src) {
        Ok(m) => m.version,
        Err(errors) => {
            return Ok((
                json!({
                    "error": { "code": "check_failed", "message": "manifest.md does not parse" },
                    "errors": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
                }),
                true,
            ));
        }
    };
    let next_version = current_version + 1;
    let version_re = regex::Regex::new(r"(?m)^version:[ \t]*\d+[ \t]*$").expect("static regex");
    if version_re.find_iter(&manifest_src).count() != 1 {
        return Ok(err(
            "check_failed",
            "manifest.md must carry exactly one `version: N` line".into(),
            None,
        ));
    }

    // Keep the previous version: first publish of a pre-existing folder
    // snapshots its pre-bump state so revert always has a floor.
    if !cori_run::versions::exists(&dir, current_version) {
        cori_run::versions::snapshot(&dir, current_version)?;
    }

    // Bump the manifest through the session — journalled like any edit.
    let bumped = version_re
        .replace(&manifest_src, format!("version: {next_version}"))
        .into_owned();
    sessions::write_file(&session_id, manifest_rel, &bumped, None)?;
    let snapshot_dir = cori_run::versions::snapshot(&dir, next_version)?;

    // Version metadata lives beside the snapshot, never inside it.
    let meta = json!({
        "published_at": chrono::Utc::now().to_rfc3339(),
        "session_id": session_id,
        "agent": session.agent,
        "approved_by": grant.by,
        "ledger_id": grant.nonce,
        "notes": notes,
    });
    if let Some(parent) = snapshot_dir.parent() {
        let _ = std::fs::write(
            parent.join(format!("v{next_version}.meta.json")),
            serde_json::to_vec_pretty(&meta)?,
        );
    }

    // Publishing is the terminal act of authoring, exactly like an
    // accepted proposal: stop the session so the Console's canvas leaves
    // the writing phase. (Left open, a published workflow renders as an
    // unfinished draft forever.) Further edits start a new session.
    sessions::stop(&session_id, Some(&format!("published v{next_version}")))?;

    let mut result = json!({
        "version": next_version,
        "previous_version": current_version,
        "notified": {
            "schedules": schedules_for(&dir),
            "remote_consumers": [],
        },
        "warnings": warnings,
        "session": "stopped — published; open a new session to edit further",
    });
    if let Some(nonce) = grant.nonce {
        result["ledger_id"] = json!(nonce);
    }
    Ok((result, false))
}

fn tool_revert(agent: &str, args: &JsonValue) -> Result<(JsonValue, bool)> {
    let path = PathBuf::from(arg_str(args, "path")?);
    let to_version = args
        .get("to_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("missing required argument `to_version`"))?
        as u32;
    if !path.is_dir() {
        return Ok(err(
            "not_found",
            format!("`{}` is not a directory on this machine", path.display()),
            None,
        ));
    }
    let path = path.canonicalize().unwrap_or(path);
    if let Err(e) = dir_allowed(&path) {
        return Ok(e);
    }
    let files = match cori_run::versions::read_files(&path, to_version) {
        Ok(f) => f,
        Err(e) => {
            let known = cori_run::versions::list(&path);
            return Ok(err(
                "not_found",
                format!(
                    "{e:#}; published versions: [{}]",
                    known
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some("publish creates snapshots; only published versions can be reverted to"),
            ));
        }
    };

    // The restore runs through its own journalled session, so it is
    // attributable in the Console and itself undoable.
    let session = sessions::create(&format!("{agent} (revert)"), &path, None)?;
    let sid = session.session_id.clone();
    let snapshot_rels: std::collections::BTreeSet<&String> =
        files.iter().map(|(rel, _)| rel).collect();
    for rel in cori_run::versions::visible_files(&path)? {
        if !snapshot_rels.contains(&rel) {
            sessions::delete_file(&sid, &rel)?;
        }
    }
    let mut files_restored = Vec::new();
    for (rel, bytes) in &files {
        sessions::write_file(&sid, rel, &String::from_utf8_lossy(bytes), None)?;
        files_restored.push(rel.clone());
    }
    sessions::stop(&sid, Some(&format!("revert to v{to_version} complete")))?;

    Ok((
        json!({
            "version": to_version,
            "files_restored": files_restored,
            "ledger_id": sid,
        }),
        false,
    ))
}

// ---------------------------------------------------------------------------
// conventions
// ---------------------------------------------------------------------------

const TEMPLATE_CLI: &str = r#"import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({}).passthrough();
const Output = z.object({ /* what this step adds */ });

export default step.cli({
  description: "…",
  input: Input,
  output: Output,
  // Static argv — an array literal, never a shell string, never Deno.run.
  command: (input) => ["curl", "--silent", "--fail", "https://…"],
  parse: (stdout) => { return JSON.parse(stdout); },
});
"#;

const TEMPLATE_CODE: &str = r#"import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ /* fields consumed */ });
const Output = z.object({ /* fields produced */ });

export default step.code({
  description: "…",
  input: Input,
  output: Output,
  // Pure transform: no I/O, no fetch, no Deno.*, no imports beyond
  // @cori-do/sdk, zod, and relative in-folder files.
  run: (input) => { return { /* … */ }; },
});
"#;

const TEMPLATE_LLM: &str = r#"import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ /* fields consumed */ });
const Output = z.object({ /* fields produced */ });

export default step.llm({
  description: "…",
  input: Input,
  output: Output,
  // Declare a level, never a model name; the machine maps it.
  level: "medium",
  prompt: (input) => `…`,
});
"#;

const TEMPLATE_MCP_TOOL: &str = r#"import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ /* fields consumed */ });
const Output = z.object({ /* fields produced */ });

export default step.mcp_tool({
  description: "…",
  input: Input,
  output: Output,
  // The server must be declared in the manifest's mcp_servers and in
  // ~/.cori/mcp-servers.json on every machine that runs this step.
  server: "server_name",
  tool: "tool_name",
  args: (input) => ({ /* … */ }),
});
"#;

const TEMPLATE_BRANCH: &str = r#"import { step, goto } from "@cori-do/sdk";

// If / Else: splits the path based on whether a rule is met.
// A path is an inline step, or goto("step_name") to route forward to a
// later step (goto("end") finishes the run).
export default step.branch({
  description: "…",
  // Pure predicate over the accumulated input; evaluated in the sandbox.
  if: (input) => input.count > 10,
  // Nested steps are inline step.<kind>({...}) calls — cli, mcp_tool,
  // code, or llm. Builtins cannot nest builtins.
  then: step.code({ description: "…", run: (input) => ({ /* … */ }) }),
  // Optional; when the rule is false and no `else` exists, the step is
  // a no-op and the run continues. Routing example:
  else: goto("cleanup"),
});
"#;

const TEMPLATE_SWITCH: &str = r#"import { step, goto } from "@cori-do/sdk";

// Switch: sends the process down one of many paths based on a value.
// A case is an inline step, or goto("step_name") to route forward to a
// later step (goto("end") finishes the run).
export default step.switch({
  description: "…",
  // Pure; must return one of the case labels below.
  on: (input) => input.severity,
  cases: {
    // Labels match [A-Za-z0-9_-]+. One inline step or goto per case.
    high: step.code({ description: "…", run: (input) => ({ /* … */ }) }),
    low: goto("archive_ticket"),
  },
  // Optional; an unmatched label without a `default` fails the run.
  default: goto("end"),
});
"#;

const TEMPLATE_FOR_EACH: &str = r#"import { step } from "@cori-do/sdk";

// For Each: repeats a nested step once per item of a runtime list.
export default step.for_each({
  description: "…",
  // Pure; extracts the list from the accumulated input.
  over: (input) => input.rows,
  // Runs sequentially; the item input carries `item` and `item_index`.
  apply: step.llm({
    description: "…",
    prompt: ({ item }) => `… ${JSON.stringify(item)}`,
  }),
  // Optional (default 100, max 1000). More items than this fails the run.
  max_items: 100,
});
"#;

const TEMPLATE_LOOP: &str = r#"import { step } from "@cori-do/sdk";

// Loop: repeats a nested step until a goal is met.
export default step.loop({
  description: "…",
  // Body output merges into the working input before `until` runs; the
  // body input carries a 1-based `iteration` counter.
  body: step.cli({
    description: "…",
    command: (input) => ["…"],
  }),
  // Pure; `true` ends the loop.
  until: (input) => input.state === "done",
  // Optional (default 10, max 100). Exhausting it fails the run.
  max_iterations: 10,
});
"#;

const TEMPLATE_WAIT: &str = r#"import { step } from "@cori-do/sdk";

// Wait / Delay: pauses the workflow until a time or event occurs.
export default step.wait({
  description: "…",
  // At least one of timeout_ms / until / signal is required.
  for: {
    // Relative delay in milliseconds (max 30 days).
    timeout_ms: 60_000,
    // Or an absolute RFC 3339 time with explicit offset:
    // until: "2026-09-01T09:00:00Z",
    // Or an external event delivered via the `event_received` signal;
    // timeout_ms/until then bound the wait, which fails if no event
    // arrives in time:
    // signal: "approved",
  },
});
"#;

fn conventions_json() -> JsonValue {
    let rule = |id: &str, severity: &str, rule: &str, rationale: &str| json!({ "id": id, "rule": rule, "severity": severity, "rationale": rationale, "scope": "global" });
    json!({
        "activity_kinds": {
            "cli": {
                "template": TEMPLATE_CLI,
                "rules": [
                    "argv is a static array; no shell interpolation, no Deno.run",
                    "the binary must be listed in the manifest's tools_required",
                ],
            },
            "code": {
                "template": TEMPLATE_CODE,
                "rules": [
                    "pure transform: no I/O, no network, no node:* imports",
                    "imports limited to @cori-do/sdk, zod, and relative .ts files",
                ],
            },
            "llm": {
                "template": TEMPLATE_LLM,
                "rules": [
                    "declare level low|medium|high; model names do not compile",
                ],
            },
            "mcp_tool": {
                "template": TEMPLATE_MCP_TOOL,
                "rules": [
                    "the server must be declared in the manifest's mcp_servers",
                    "for Google Workspace prefer a `gws` cli step (see house rule gws_via_cli)",
                ],
            },
            "branch": {
                "template": TEMPLATE_BRANCH,
                "rules": [
                    "If / Else: `if` is a pure predicate; `then` runs when it holds, optional `else` otherwise (false + no else = no-op)",
                    "nested steps are inline step.cli/mcp_tool/code/llm calls — no helper variables, and builtins cannot nest builtins",
                    "a path may instead be goto(\"step_name\") — forward-only routing to a later step by its name (the part of NN_name.ts after the number), or goto(\"end\") to finish the run; steps skipped by a route get `not_taken` trace rows",
                    "nested capabilities still need declaring: cli binaries in tools_required, MCP servers in mcp_servers",
                ],
            },
            "switch": {
                "template": TEMPLATE_SWITCH,
                "rules": [
                    "Switch: `on` returns a case label ([A-Za-z0-9_-]+); an unmatched label without `default` fails the run",
                    "nested steps are inline step.cli/mcp_tool/code/llm calls — builtins cannot nest builtins",
                    "a case may instead be goto(\"step_name\") — forward-only routing to a later step by its name, or goto(\"end\") to finish the run; every step must stay reachable or the workflow does not compile",
                    "nested capabilities still need declaring in the manifest",
                ],
            },
            "for_each": {
                "template": TEMPLATE_FOR_EACH,
                "rules": [
                    "For Each: `over` extracts the list; `apply` runs sequentially per item with `item` and `item_index` in its input",
                    "max_items defaults to 100 (cap 1000); a longer list fails the run — output is { items: [...] }",
                    "nested step is an inline step.cli/mcp_tool/code/llm call; declare its capabilities in the manifest",
                ],
            },
            "loop": {
                "template": TEMPLATE_LOOP,
                "rules": [
                    "Loop: repeats `body` (its output merges into the working input) until the pure `until` returns true",
                    "max_iterations defaults to 10 (cap 100); exhausting it fails the run — output is the last body output plus `iterations`",
                    "nested step is an inline step.cli/mcp_tool/code/llm call; declare its capabilities in the manifest",
                ],
            },
            "wait": {
                "template": TEMPLATE_WAIT,
                "rules": [
                    "Wait / Delay: `for` needs at least one of timeout_ms (≤ 30 days), until (RFC 3339 with explicit offset), or signal ([A-Za-z0-9_.-]+)",
                    "with `signal`, timeout_ms/until bound the wait and it fails if no event arrives; the event is delivered via the run's `event_received` Temporal signal",
                    "wait steps have no nested steps and no retries/timeout_ms options; --dry-run does not pause",
                ],
            },
            "builtin": {
                "template": null,
                "rules": [
                    "control flow (branch, switch, for_each, loop, wait) is executable — see those kinds for templates",
                    "`map` and `parallel` are still deferred: the compiler accepts them but the runtime skips them",
                    "the outer builtin takes `description` only — retries/timeout_ms belong on its nested steps",
                ],
            },
        },
        "manifest_schema": {
            "frontmatter": {
                "id": "snake_case string, required",
                "name": "human name, required",
                "description": "one sentence, required",
                "created": "YYYY-MM-DD, required",
                "version": "integer, required",
                "parameters": "[{name, type: string|number|boolean|enum|path, required?, default?, values?}]",
                "tools_required": "cli binaries used by cli steps",
                "mcp_servers": "MCP servers used by mcp_tool steps",
                "tags": "free-form strings",
                "schedule": "optional POSIX cron; schedule_tz optional IANA tz",
                "result": "optional {headline, fields[], sections[], artifacts[]}",
            },
            "body": "markdown prose: # Name, ## Goal, ## Steps, ## Verification",
        },
        "house_rules": [
            rule("steps_filename", "error",
                "step files are steps/NN_snake_case.ts, numbered gaplessly from 01",
                "the compiler derives the DAG from the numbering; gaps and duplicates do not compile"),
            rule("frozen_imports", "error",
                "workflow modules (tests included) import only relative .ts files, @cori-do/sdk, and zod — no npm:/jsr:/https: specifiers, no dynamic import()",
                "the runtime resolves from a frozen import map; anything else fails at run time"),
            rule("no_io_in_code", "error",
                "code steps are pure transforms — no filesystem, network, or process access",
                "external effects belong to cli/mcp_tool steps, where the broker mediates them"),
            rule("cli_static_argv", "error",
                "cli steps build argv as a static array; never a shell string, never Deno.run",
                "static argv is auditable and injection-proof; the broker executes it directly"),
            rule("declared_capabilities", "error",
                "every cli binary appears in tools_required and every MCP server in mcp_servers — and nothing unused is declared",
                "check and the planner reason from declarations; drift in either direction breaks placement"),
            rule("gws_via_cli", "warn",
                "Google Workspace goes through a `gws` cli step, not an MCP server",
                "simpler auth, and it avoids requiring the server in ~/.cori/mcp-servers.json on every worker"),
            rule("no_secrets_in_folder", "error",
                "no credentials, .env files, or secret-looking filenames in the workflow folder",
                "the folder ships in the run bundle and into Temporal history; credentials stay broker-managed"),
            rule("zero_dep_tests", "warn",
                "tests assert with the local tests/assert.ts helper, not a registry package",
                "jsr.io is blocked in many authoring sandboxes; a test that dies on an import proves nothing"),
        ],
        "test_template": { "path": "tests/assert.ts", "content": ASSERT_TS_TEMPLATE },
    })
}

// ---------------------------------------------------------------------------
// capabilities
// ---------------------------------------------------------------------------

/// Machine-wide capability readiness in the spec's three states, from
/// the same discovery code `check` and `run` use.
fn capabilities_json() -> Result<JsonValue> {
    use cori_broker::capabilities::{self, CapabilityKind, CapabilityReport};
    use cori_broker::identity::{IdentitySource, OsUser};

    let mut rows: Vec<JsonValue> = Vec::new();

    // Registry binaries: install state is the declaration state.
    for r in capabilities::registry_status() {
        let state = if !r.installed {
            "not_declared"
        } else if r.requires_auth && r.authed != Some(true) {
            "declared_unauthed"
        } else {
            "ready"
        };
        let mut row = json!({
            "name": r.id,
            "kind": "cli",
            "state": state,
            "use_for": r.use_for,
        });
        if let Some(remedy) = r.remedy {
            row["remedy"] = json!(remedy);
        }
        rows.push(row);
    }

    // Declared MCP servers and LLM providers, with auth probes.
    let identity = OsUser.resolve().context("resolving OS user identity")?;
    let home = cori_run::paths::home()?;
    let caps = capabilities::discover_with_policy(
        &home,
        &[],
        &cori_run::resolve_llm_credentials(),
        &cori_run::resolve_llm_policy(&identity),
        capabilities::LlmProbe::Probe,
    );
    let report = CapabilityReport::from_capabilities_with(
        identity,
        &caps,
        Some(&cori_run::paths::credentials_dir()?),
    );
    for c in &report.capabilities {
        if matches!(c.kind, CapabilityKind::Cli) {
            continue; // registry rows above already cover binaries
        }
        let state = if c.authed {
            "ready"
        } else {
            "declared_unauthed"
        };
        let mut row = json!({
            "name": c.id,
            "kind": format!("{:?}", c.kind),
            "state": state,
        });
        if let Some(d) = &c.detail {
            row["detail"] = json!(d);
        }
        if !c.authed {
            row["remedy"] = json!(format!("cori login {}", c.id));
        }
        rows.push(row);
    }

    Ok(json!(rows))
}

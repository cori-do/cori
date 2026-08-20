//! Workflow compiler: parses manifest + steps and emits a
//! [`CompiledWorkflow`].
//!
//! Compiler responsibilities:
//!
//! 1. Parse `manifest.md` via [`cori_manifest`].
//! 2. Enumerate `steps/*.ts`, validate gapless numeric prefixes, and sort.
//! 3. For each step file, statically extract metadata (kind, description,
//!    declared `route`, plus kind-specific fields like CLI binary name, MCP
//!    `server` and `tool`, etc.).
//! 4. Build the linear DAG (`steps[i].depends_on = [steps[i-1].activity_id]`).
//! 5. Cross-validate: every CLI binary referenced is declared in
//!    `tools_required`; every MCP server is in `mcp_servers`; `code` steps
//!    must not import `node:*` modules.
//!
//! Static parsing is intentionally regex-based for now. The implementation
//! will eventually migrate to a full AST parser (swc/oxc) once builder
//! evaluation lands. Until then we accept the constraint that
//! step files use the canonical SDK call pattern.
//!
//! Note on `tsc --noEmit`: the long-term plan calls for a bundled TypeScript
//! compiler. That is wired in a later update (a vendored `tsc` ships with the
//! Cori binary). The compiler remains purely structural; `cori-run` closes the
//! executable-workflow gap by running the already-required Deno checker during
//! preflight, before any activity starts.

pub mod effects;
mod step_parser;

use std::io::Read;
use std::path::{Path, PathBuf};

use cori_manifest::{ManifestError, parse_manifest};
use cori_protocol::{
    CompiledStep, CompiledWorkflow, MAX_WORKFLOW_SOURCE_BYTES, MAX_WORKFLOW_SOURCE_FILE_BYTES,
    MAX_WORKFLOW_SOURCE_FILES, MAX_WORKFLOW_SOURCE_PATH_BYTES, MAX_WORKFLOW_SOURCE_PATH_DEPTH,
    Placement, StepKind, workflow_source_component_looks_sensitive,
};
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

pub use step_parser::ParsedStep;

/// Re-parse one CLI step at the activity boundary and return its statically
/// declared argv[0]. Activities use this immediately before evaluating the
/// command builder so a data-dependent runtime branch cannot switch binaries.
pub fn cli_binary_from_source(source: &str) -> Result<String, String> {
    let parsed = step_parser::parse(source).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| error.reason)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    if parsed.kind != StepKind::Cli {
        return Err("step is not a `cli` activity".to_string());
    }
    parsed
        .metadata
        .get("binary")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "could not statically determine CLI argv[0]".to_string())
}

/// Hash every executable input under a workflow root.
///
/// The digest is used both for the rebuildable compiler cache and at the
/// activity boundary. Keeping one implementation prevents a workflow import
/// (for example `types.ts` or `lib/helpers.ts`) from changing after preflight
/// while the trace still claims the old workflow identity.
///
/// Git metadata is excluded because it is not importable workflow source and
/// can change independently while a workflow folder is otherwise immutable.
pub fn workflow_content_hash(workflow_dir: &Path) -> anyhow::Result<String> {
    fn include_entry(entry: &DirEntry) -> bool {
        entry.depth() == 0 || entry.file_name() != ".git"
    }

    let entries = WalkDir::new(workflow_dir)
        // Workflow folders are executable input. Following a repository-owned
        // symlink here would let an untrusted remote workflow make Cori walk
        // and hash files outside the checkout before the consent gate.
        .follow_links(false)
        .into_iter()
        .filter_entry(include_entry);
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            anyhow::anyhow!(
                "walking workflow directory `{}`: {error}",
                workflow_dir.display()
            )
        })?;
        if entry.file_type().is_symlink() {
            return Err(anyhow::anyhow!(
                "workflow tree contains symlink `{}`; symlinks are not allowed in executable workflow input",
                entry.path().display()
            ));
        }
        if entry.file_type().is_file() {
            files.push(entry.into_path());
            if files.len() > MAX_WORKFLOW_SOURCE_FILES {
                anyhow::bail!(
                    "workflow tree contains more than {} files",
                    MAX_WORKFLOW_SOURCE_FILES
                );
            }
        } else if !entry.file_type().is_dir() {
            return Err(anyhow::anyhow!(
                "workflow tree contains unsupported special file `{}`",
                entry.path().display()
            ));
        }
    }

    files.sort_by(|left, right| {
        normalized_relative_path(workflow_dir, left)
            .cmp(&normalized_relative_path(workflow_dir, right))
    });

    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    for path in files {
        let relative = normalized_relative_path(workflow_dir, &path);
        if path
            .strip_prefix(workflow_dir)
            .unwrap_or(&path)
            .components()
            .any(|component| {
                workflow_source_component_looks_sensitive(&component.as_os_str().to_string_lossy())
            })
        {
            anyhow::bail!(
                "workflow source `{relative}` looks like a credential or secret file; keep secrets outside the workflow folder because Cori freezes source in its cache and may persist it in Temporal history"
            );
        }
        let depth = Path::new(&relative).components().count();
        if relative.len() > MAX_WORKFLOW_SOURCE_PATH_BYTES || depth > MAX_WORKFLOW_SOURCE_PATH_DEPTH
        {
            anyhow::bail!(
                "workflow path `{relative}` exceeds the portable source limit ({} bytes, depth {})",
                MAX_WORKFLOW_SOURCE_PATH_BYTES,
                MAX_WORKFLOW_SOURCE_PATH_DEPTH
            );
        }
        let metadata = std::fs::metadata(&path).map_err(|error| {
            anyhow::anyhow!("reading metadata for `{}`: {error}", path.display())
        })?;
        let expected_len = metadata.len();
        if expected_len > MAX_WORKFLOW_SOURCE_FILE_BYTES {
            anyhow::bail!(
                "workflow file `{relative}` is {expected_len} bytes; per-file limit is {}",
                MAX_WORKFLOW_SOURCE_FILE_BYTES
            );
        }
        total_bytes = total_bytes
            .checked_add(expected_len)
            .ok_or_else(|| anyhow::anyhow!("workflow source size overflow"))?;
        if total_bytes > MAX_WORKFLOW_SOURCE_BYTES {
            anyhow::bail!(
                "workflow tree is {total_bytes} bytes; total source limit is {}",
                MAX_WORKFLOW_SOURCE_BYTES
            );
        }

        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(expected_len.to_le_bytes());
        let mut file = std::fs::File::open(&path)
            .map_err(|error| anyhow::anyhow!("opening `{}`: {error}", path.display()))?;
        let mut observed_len = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| anyhow::anyhow!("reading `{}`: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            observed_len = observed_len
                .checked_add(read as u64)
                .ok_or_else(|| anyhow::anyhow!("workflow file size overflow"))?;
            if observed_len > MAX_WORKFLOW_SOURCE_FILE_BYTES {
                anyhow::bail!(
                    "workflow file `{relative}` grew beyond the per-file limit while it was hashed"
                );
            }
            hasher.update(&buffer[..read]);
        }
        if observed_len != expected_len {
            anyhow::bail!(
                "workflow file `{relative}` changed while it was hashed (expected {expected_len} bytes, read {observed_len})"
            );
        }
    }

    Ok(hex::encode(hasher.finalize()))
}

fn normalized_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A structured compile error. Always carries a file path so the CLI can
/// surface `file:line: reason` diagnostics.
#[derive(Debug, Clone, Serialize, Error)]
#[error("{file}: {reason}{}", line.map(|l| format!(" (line {l})")).unwrap_or_default())]
pub struct CompileError {
    pub file: String,
    pub line: Option<usize>,
    pub field: Option<String>,
    pub reason: String,
}

impl CompileError {
    fn new(file: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line: None,
            field: None,
            reason: reason.into(),
        }
    }
    fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
    fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

impl From<ManifestError> for CompileError {
    fn from(e: ManifestError) -> Self {
        CompileError {
            file: "manifest.md".into(),
            line: e.line,
            field: Some(e.field),
            reason: e.reason,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the [`Placement`] for a step from its kind + parsed
/// metadata. Pure: lives here so the protocol crate stays free of
/// step-kind heuristics.
///
/// - `Cli` → [`Placement::RequiresLocalFs`] (the binary runs on the
///   requesting user's machine).
/// - `Code` → `RequiresLocalFs` if the metadata declares a `reads_path`
///   or `writes_path` field (deferred; the static parser does not emit
///   these in v1, but the rule lives here so it lights up automatically
///   when the parser learns them); otherwise [`Placement::Anywhere`].
/// - `McpTool` → [`Placement::RequiresCapability`] keyed by the
///   declared `server` name. Falls back to `Anywhere` if absent (the
///   compiler will already have raised a hard error before this point).
/// - `Llm`, `Builtin` → [`Placement::Anywhere`].
pub fn compute_placement(
    kind: StepKind,
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Placement {
    match kind {
        StepKind::Cli => Placement::RequiresLocalFs,
        StepKind::Code => {
            let needs_fs =
                metadata.contains_key("reads_path") || metadata.contains_key("writes_path");
            if needs_fs {
                Placement::RequiresLocalFs
            } else {
                Placement::Anywhere
            }
        }
        StepKind::McpTool => match metadata.get("server").and_then(|v| v.as_str()) {
            Some(server) => Placement::RequiresCapability {
                id: server.to_string(),
            },
            None => Placement::Anywhere,
        },
        StepKind::Llm | StepKind::Builtin => Placement::Anywhere,
    }
}

/// Compile a workflow directory. Returns the compiled workflow on success or
/// a non-empty list of structured errors.
pub fn compile(workflow_dir: &Path) -> Result<CompiledWorkflow, Vec<CompileError>> {
    let mut errors: Vec<CompileError> = Vec::new();

    if !workflow_dir.is_dir() {
        return Err(vec![CompileError::new(
            workflow_dir.display().to_string(),
            "workflow path is not a directory",
        )]);
    }

    // 1. manifest.md
    let manifest_path = workflow_dir.join("manifest.md");
    if !manifest_path.is_file() {
        return Err(vec![CompileError::new(
            "manifest.md",
            "missing required file at workflow root",
        )]);
    }
    let manifest_src = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) => {
            return Err(vec![CompileError::new(
                "manifest.md",
                format!("could not read: {e}"),
            )]);
        }
    };
    let manifest = match parse_manifest(&manifest_src) {
        Ok(m) => m,
        Err(es) => return Err(es.into_iter().map(Into::into).collect()),
    };

    // 2. steps/
    let steps_dir = workflow_dir.join("steps");
    if !steps_dir.is_dir() {
        errors.push(CompileError::new(
            "steps/",
            "missing required `steps/` directory",
        ));
        return Err(errors);
    }

    let step_files = match enumerate_step_files(&steps_dir) {
        Ok(v) => v,
        Err(es) => {
            errors.extend(es);
            return Err(errors);
        }
    };

    if step_files.is_empty() {
        errors.push(CompileError::new(
            "steps/",
            "no step files found (expected `steps/NN_name.ts`)",
        ));
        return Err(errors);
    }

    // 3. Parse each step.
    let mut compiled_steps: Vec<CompiledStep> = Vec::with_capacity(step_files.len());
    for (idx, sf) in step_files.iter().enumerate() {
        let rel = format!("steps/{}", sf.filename);
        let src = match std::fs::read_to_string(&sf.path) {
            Ok(s) => s,
            Err(e) => {
                errors.push(CompileError::new(&rel, format!("could not read: {e}")));
                continue;
            }
        };
        match step_parser::parse(&src) {
            Ok(parsed) => {
                let activity_id = format!("{:02}_{}", sf.number, sf.name);
                // A prior file may have failed parsing and therefore not have
                // produced a compiled step. Keep collecting all diagnostics
                // without indexing the shorter successful-step vector.
                let depends_on = compiled_steps
                    .last()
                    .map(|step| vec![step.activity_id.clone()])
                    .unwrap_or_default();
                let placement = compute_placement(parsed.kind, &parsed.metadata);
                compiled_steps.push(CompiledStep {
                    activity_id,
                    index: idx as u32,
                    source_path: rel.clone(),
                    source_sha256: Some(source_sha256(src.as_bytes())),
                    kind: parsed.kind,
                    name: sf.name.clone(),
                    description: parsed.description,
                    route: parsed.route,
                    depends_on,
                    metadata: parsed.metadata,
                    placement,
                    task_queue: None,
                });
            }
            Err(es) => {
                for e in es {
                    let mut ce = CompileError::new(&rel, e.reason);
                    if let Some(l) = e.line {
                        ce = ce.with_line(l);
                    }
                    if let Some(f) = e.field {
                        ce = ce.with_field(f);
                    }
                    errors.push(ce);
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // 4. Routing: resolve `goto("name")` targets to activity ids and
    //    reject unknown / ambiguous / backward routes and unreachable
    //    steps. Runs before capability cross-validation so later checks
    //    see fully resolved metadata.
    let route_errors = resolve_goto_targets(&mut compiled_steps);
    if !route_errors.is_empty() {
        return Err(route_errors);
    }
    let reach_errors = validate_reachability(&compiled_steps);
    if !reach_errors.is_empty() {
        return Err(reach_errors);
    }

    // 5. Cross-validation.
    let mut required_cli: Vec<String> = Vec::new();
    let mut required_mcp: Vec<String> = Vec::new();
    let required_llm: Vec<String> = Vec::new();
    let mut requires_llm = false;
    for step in &compiled_steps {
        let rel = step.source_path.clone();
        match step.kind {
            StepKind::Cli => {
                if let Some(bin) = step.metadata.get("binary").and_then(|v| v.as_str()) {
                    if !manifest.tools_required.iter().any(|t| t == bin) {
                        errors.push(
                            CompileError::new(
                                &rel,
                                format!(
                                    "CLI binary `{bin}` not declared in manifest `tools_required`"
                                ),
                            )
                            .with_field("command"),
                        );
                    }
                    if !required_cli.contains(&bin.to_string()) {
                        required_cli.push(bin.to_string());
                    }
                }
            }
            StepKind::McpTool => {
                if let Some(server) = step.metadata.get("server").and_then(|v| v.as_str()) {
                    if !manifest.mcp_servers.iter().any(|s| s == server) {
                        errors.push(
                            CompileError::new(
                                &rel,
                                format!(
                                    "MCP server `{server}` not declared in manifest `mcp_servers`"
                                ),
                            )
                            .with_field("server"),
                        );
                    }
                    if !required_mcp.contains(&server.to_string()) {
                        required_mcp.push(server.to_string());
                    }
                }
            }
            StepKind::Code => {
                if let Some(violations) =
                    step.metadata.get("node_imports").and_then(|v| v.as_array())
                {
                    for v in violations {
                        if let Some(s) = v.as_str() {
                            errors.push(
                                CompileError::new(
                                    &rel,
                                    format!(
                                        "`code` step must not import `{s}` — use a `cli` or `mcp_tool` step for side effects"
                                    ),
                                )
                                .with_field("imports"),
                            );
                        }
                    }
                }
            }
            StepKind::Llm => {
                // Provider choice is machine configuration. Compiled LLM
                // steps carry only their portable low/medium/high level.
                requires_llm = true;
            }
            StepKind::Builtin => {
                // A builtin's nested steps carry the same capability
                // boundary as top-level steps. Validate each slot so a
                // branch case cannot smuggle an undeclared binary or
                // server past preflight and the planner.
                if let Some(violations) =
                    step.metadata.get("node_imports").and_then(|v| v.as_array())
                {
                    for v in violations {
                        if let Some(s) = v.as_str() {
                            errors.push(
                                CompileError::new(
                                    &rel,
                                    format!(
                                        "builtin step files must not import `{s}` — nested steps run in the same sandbox as `code` steps"
                                    ),
                                )
                                .with_field("imports"),
                            );
                        }
                    }
                }
                let nested = step
                    .metadata
                    .get("nested")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                for (slot, meta) in &nested {
                    let Some(meta) = meta.as_object() else {
                        continue;
                    };
                    match meta.get("kind").and_then(|v| v.as_str()) {
                        Some("cli") => {
                            if let Some(bin) = meta.get("binary").and_then(|v| v.as_str()) {
                                if !manifest.tools_required.iter().any(|t| t == bin) {
                                    errors.push(
                                        CompileError::new(
                                            &rel,
                                            format!(
                                                "CLI binary `{bin}` (in `{slot}`) not declared in manifest `tools_required`"
                                            ),
                                        )
                                        .with_field(slot),
                                    );
                                }
                                if !required_cli.contains(&bin.to_string()) {
                                    required_cli.push(bin.to_string());
                                }
                            }
                        }
                        Some("mcp_tool") => {
                            if let Some(server) = meta.get("server").and_then(|v| v.as_str()) {
                                if !manifest.mcp_servers.iter().any(|s| s == server) {
                                    errors.push(
                                        CompileError::new(
                                            &rel,
                                            format!(
                                                "MCP server `{server}` (in `{slot}`) not declared in manifest `mcp_servers`"
                                            ),
                                        )
                                        .with_field(slot),
                                    );
                                }
                                if !required_mcp.contains(&server.to_string()) {
                                    required_mcp.push(server.to_string());
                                }
                            }
                        }
                        Some("llm") => requires_llm = true,
                        _ => {}
                    }
                }
            }
        }
    }

    // Capability declarations are the execution boundary, not documentation.
    // Reject extras so an interpreter wrapper cannot declare the hidden child
    // process (for example `tools_required: [deno, gws]` with argv[0] `deno`)
    // and thereby make planner/auth checks appear to cover a binary the broker
    // never dispatches directly.
    for declared in &manifest.tools_required {
        if !required_cli.contains(declared) {
            errors.push(
                CompileError::new(
                    "manifest.md",
                    format!(
                        "CLI binary `{declared}` is declared in `tools_required` but no `cli` step invokes it directly as argv[0]"
                    ),
                )
                .with_field("tools_required"),
            );
        }
    }
    for declared in &manifest.mcp_servers {
        if !required_mcp.contains(declared) {
            errors.push(
                CompileError::new(
                    "manifest.md",
                    format!(
                        "MCP server `{declared}` is declared in `mcp_servers` but no `mcp_tool` step uses it"
                    ),
                )
                .with_field("mcp_servers"),
            );
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(CompiledWorkflow {
        manifest,
        steps: compiled_steps,
        required_cli_binaries: required_cli,
        required_mcp_servers: required_mcp,
        required_llm_providers: required_llm,
        requires_llm,
    })
}

/// Return the full SHA-256 digest used to freeze one compiled step source.
pub fn source_sha256(source: &[u8]) -> String {
    hex::encode(Sha256::digest(source))
}

// ---------------------------------------------------------------------------
// Draft view — best-effort parse for live authoring display
// ---------------------------------------------------------------------------

/// Best-effort view of a workflow folder mid-authoring. Never an error:
/// whatever parses is returned, whatever doesn't is skipped or stubbed.
#[derive(Debug)]
pub struct DraftWorkflow {
    /// `None` when `manifest.md` is missing or does not parse yet.
    pub manifest: Option<cori_manifest::Manifest>,
    /// Per-file parses in step order. No routing resolution, no
    /// reachability check, no manifest cross-validation — steps written
    /// so far render even while the folder as a whole cannot compile.
    pub steps: Vec<CompiledStep>,
}

/// Lenient, per-file parse of a workflow folder for the Console's live
/// authoring canvas. [`compile`] rejects every intermediate authoring
/// state by design (missing steps, declared-but-unused tools, forward
/// routes to steps not written yet); this function accepts them all so
/// the graph can grow as the agent writes. Draft output is display-only:
/// it must never feed the planner, the runner, or a proposal — those go
/// through [`compile`], whose cross-checks are the execution boundary.
pub fn draft(workflow_dir: &Path) -> DraftWorkflow {
    let manifest = std::fs::read_to_string(workflow_dir.join("manifest.md"))
        .ok()
        .and_then(|src| parse_manifest(&src).ok());

    let steps_dir = workflow_dir.join("steps");
    let re = step_filename_re();
    let mut files: Vec<StepFile> = Vec::new();
    if let Ok(read) = std::fs::read_dir(&steps_dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned)
            else {
                continue;
            };
            let Some(caps) = re.captures(&filename) else {
                continue; // draft tolerates stray files; compile flags them
            };
            let number: u32 = caps[1].parse().unwrap_or(u32::MAX);
            let name = caps[2].to_string();
            files.push(StepFile {
                path,
                filename,
                number,
                name,
            });
        }
    }
    // Duplicates and gaps render in file order; compile rejects them later.
    files.sort_by(|a, b| (a.number, &a.filename).cmp(&(b.number, &b.filename)));

    let mut steps: Vec<CompiledStep> = Vec::with_capacity(files.len());
    for (idx, sf) in files.iter().enumerate() {
        let rel = format!("steps/{}", sf.filename);
        let Ok(src) = std::fs::read_to_string(&sf.path) else {
            continue;
        };
        let (kind, description, route, metadata) = match step_parser::parse(&src) {
            Ok(parsed) => (
                parsed.kind,
                parsed.description,
                parsed.route,
                parsed.metadata,
            ),
            // A half-written file still earns a node: sniff the kind from
            // the SDK call so the canvas can at least color it right.
            Err(_) => {
                let (kind, metadata) = sniff_step_kind(&src);
                (kind, String::new(), None, metadata)
            }
        };
        let placement = compute_placement(kind, &metadata);
        let depends_on = steps
            .last()
            .map(|step: &CompiledStep| vec![step.activity_id.clone()])
            .unwrap_or_default();
        steps.push(CompiledStep {
            activity_id: format!("{:02}_{}", sf.number, sf.name),
            index: idx as u32,
            source_path: rel,
            source_sha256: Some(source_sha256(src.as_bytes())),
            kind,
            name: sf.name.clone(),
            description,
            route,
            depends_on,
            metadata,
            placement,
            task_queue: None,
        });
    }

    DraftWorkflow { manifest, steps }
}

/// Guess a step file's kind from its `step.<kind>(` call when the full
/// parse fails. Defaults to `code` — the most neutral node.
fn sniff_step_kind(source: &str) -> (StepKind, serde_json::Map<String, serde_json::Value>) {
    let re = Regex::new(
        r"step\s*\.\s*(cli|code|llm|mcp_tool|branch|switch|for_each|loop|wait|map|parallel)\s*\(",
    )
    .expect("static regex");
    let mut metadata = serde_json::Map::new();
    let kind = match re.captures(source).map(|c| c[1].to_string()).as_deref() {
        Some("cli") => StepKind::Cli,
        Some("llm") => StepKind::Llm,
        Some("mcp_tool") => StepKind::McpTool,
        Some(
            builtin @ ("branch" | "switch" | "for_each" | "loop" | "wait" | "map" | "parallel"),
        ) => {
            metadata.insert(
                "builtin".to_string(),
                serde_json::Value::String(builtin.to_string()),
            );
            StepKind::Builtin
        }
        _ => StepKind::Code,
    };
    (kind, metadata)
}

// ---------------------------------------------------------------------------
// Step routing (`goto`) — see docs/step-routing-design.md
// ---------------------------------------------------------------------------

/// The routing surface of one step: whether execution can fall through
/// to the next step, and the resolved jump targets (`None` = `end`).
fn step_routing(step: &CompiledStep) -> (bool, Vec<Option<String>>) {
    if step.kind != StepKind::Builtin {
        return (true, Vec::new());
    }
    let sub_kind = step.metadata.get("builtin").and_then(|v| v.as_str());
    if !matches!(sub_kind, Some("branch") | Some("switch")) {
        return (true, Vec::new());
    }
    let nested = step
        .metadata
        .get("nested")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut fall_through = false;
    let mut targets = Vec::new();
    for meta in nested.values() {
        match meta.as_object().and_then(|m| m.get("goto")) {
            Some(serde_json::Value::String(target)) if target == "end" => targets.push(None),
            Some(serde_json::Value::String(target)) => targets.push(Some(target.clone())),
            _ => fall_through = true,
        }
    }
    // A branch without an `else` has an implicit "false → continue"
    // path; a switch always takes one of its declared paths.
    if sub_kind == Some("branch") && !nested.contains_key("else") {
        fall_through = true;
    }
    (fall_through, targets)
}

/// Resolve every `goto("name")` route to its target's activity id and
/// enforce the routing rules: the target name exists, names exactly one
/// step, lies strictly after the routing step (backward routing is what
/// `loop` is for), and `end` is not shadowed by a step named `end`.
/// Resolved slots gain `"goto": "<activity_id>"` (or `"end"`) next to
/// their original `goto_name`.
fn resolve_goto_targets(steps: &mut [CompiledStep]) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let index: Vec<(String, String)> = steps
        .iter()
        .map(|s| (s.name.clone(), s.activity_id.clone()))
        .collect();

    for (i, step) in steps.iter_mut().enumerate() {
        let rel = step.source_path.clone();
        let Some(nested) = step
            .metadata
            .get_mut("nested")
            .and_then(|v| v.as_object_mut())
        else {
            continue;
        };
        for (slot, meta) in nested.iter_mut() {
            let Some(meta) = meta.as_object_mut() else {
                continue;
            };
            let Some(target_name) = meta
                .get("goto_name")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if target_name == "end" {
                if index.iter().any(|(name, _)| name == "end") {
                    errors.push(
                        CompileError::new(
                            &rel,
                            format!(
                                "`{slot}` routes to `end`, but a step named `end` exists — `end` is the reserved finish-the-run target; rename the step"
                            ),
                        )
                        .with_field(slot),
                    );
                    continue;
                }
                meta.insert("goto".into(), serde_json::Value::String("end".into()));
                continue;
            }
            let matches: Vec<usize> = index
                .iter()
                .enumerate()
                .filter(|(_, (name, _))| *name == target_name)
                .map(|(j, _)| j)
                .collect();
            match matches.as_slice() {
                [] => errors.push(
                    CompileError::new(
                        &rel,
                        format!(
                            "`{slot}` routes to unknown step `{target_name}` — targets are step names (the part of `NN_name.ts` after the number) or `end`"
                        ),
                    )
                    .with_field(slot),
                ),
                [j] => {
                    if *j <= i {
                        errors.push(
                            CompileError::new(
                                &rel,
                                format!(
                                    "`{slot}` routes backward to `{target_name}` — goto targets must come after this step (repeat work with a `loop` step instead)"
                                ),
                            )
                            .with_field(slot),
                        );
                    } else {
                        meta.insert(
                            "goto".into(),
                            serde_json::Value::String(index[*j].1.clone()),
                        );
                    }
                }
                _ => errors.push(
                    CompileError::new(
                        &rel,
                        format!(
                            "`{slot}` routes to `{target_name}`, which names {} steps — step names must be unique to be routing targets",
                            matches.len()
                        ),
                    )
                    .with_field(slot),
                ),
            }
        }
    }
    errors
}

/// Reject steps no route can reach. Walk from step 0: every step whose
/// routing allows fall-through has an edge to the next step, and every
/// resolved goto adds an edge to its target. Dead steps are dead code,
/// and Cori rejects dead declarations everywhere else.
fn validate_reachability(steps: &[CompiledStep]) -> Vec<CompileError> {
    let n = steps.len();
    if n == 0 {
        return Vec::new();
    }
    let id_to_index: std::collections::HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.activity_id.as_str(), i))
        .collect();
    let mut reachable = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        if i >= n || reachable[i] {
            continue;
        }
        reachable[i] = true;
        let (fall_through, targets) = step_routing(&steps[i]);
        if fall_through {
            stack.push(i + 1);
        }
        for target in targets.into_iter().flatten() {
            if let Some(&j) = id_to_index.get(target.as_str()) {
                stack.push(j);
            }
        }
    }
    steps
        .iter()
        .enumerate()
        .filter(|(i, _)| !reachable[*i])
        .map(|(_, step)| {
            CompileError::new(
                &step.source_path,
                "unreachable step — every route through the preceding control flow jumps past it; remove the step or add a path that reaches it",
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Step file enumeration
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct StepFile {
    path: PathBuf,
    filename: String,
    number: u32,
    name: String,
}

fn step_filename_re() -> Regex {
    Regex::new(r"^(\d+)_([a-z][a-z0-9_]*)\.ts$").expect("static regex")
}

fn enumerate_step_files(steps_dir: &Path) -> Result<Vec<StepFile>, Vec<CompileError>> {
    let re = step_filename_re();
    let mut files: Vec<StepFile> = Vec::new();
    let mut errors: Vec<CompileError> = Vec::new();

    let read = std::fs::read_dir(steps_dir).map_err(|e| {
        vec![CompileError::new(
            "steps/",
            format!("could not read directory: {e}"),
        )]
    })?;

    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        // Ignore non-TS files (READMEs, .DS_Store, etc.) silently.
        if !filename.ends_with(".ts") {
            continue;
        }
        match re.captures(&filename) {
            Some(caps) => {
                let number: u32 = caps[1].parse().unwrap_or(u32::MAX);
                let name = caps[2].to_string();
                files.push(StepFile {
                    path,
                    filename,
                    number,
                    name,
                });
            }
            None => {
                errors.push(CompileError::new(
                    format!("steps/{filename}"),
                    "step filename must match `NN_snake_case.ts` (e.g. `01_read_rows.ts`)",
                ));
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    files.sort_by_key(|f| f.number);

    // Gapless numbering check: must start at 1 (or 0) and increment by 1.
    if let Some(first) = files.first() {
        let start = first.number;
        if start != 1 && start != 0 {
            errors.push(CompileError::new(
                format!("steps/{}", first.filename),
                format!("step numbering must start at 1; first file is {start:02}"),
            ));
        }
        for window in files.windows(2) {
            let prev = &window[0];
            let cur = &window[1];
            if cur.number == prev.number {
                errors.push(CompileError::new(
                    format!("steps/{}", cur.filename),
                    format!(
                        "duplicate step number {:02}: `steps/{}` and `steps/{}` — if you \
                         renamed a step, the older file is probably orphaned; remove the \
                         stale one",
                        cur.number, prev.filename, cur.filename
                    ),
                ));
            } else if cur.number != prev.number + 1 {
                errors.push(CompileError::new(
                    format!("steps/{}", cur.filename),
                    format!(
                        "gap in step numbering: expected {:02} after {:02}, got {:02}",
                        prev.number + 1,
                        prev.number,
                        cur.number
                    ),
                ));
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(files)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_workflow(dir: &Path, manifest: &str, files: &[(&str, &str)]) {
        fs::create_dir_all(dir.join("steps")).unwrap();
        fs::write(dir.join("manifest.md"), manifest).unwrap();
        for (name, body) in files {
            fs::write(dir.join("steps").join(name), body).unwrap();
        }
    }

    const OK_MANIFEST: &str = "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\ntools_required: [echo]\n---\n# body\n";

    const LLM_MANIFEST: &str =
        "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\n---\n# body\n";

    fn compile_llm_step(args: &str) -> CompiledWorkflow {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[(
                "01_ask.ts",
                &format!(
                    "import {{ step }} from \"@cori-do/sdk\";\nexport default step.llm({{ description: \"ask\", {args}prompt: () => `hi` }});"
                ),
            )],
        );
        compile(tmp.path()).expect("llm workflow compiles")
    }

    #[test]
    fn level_declaration_needs_an_llm_but_no_specific_provider() {
        let c = compile_llm_step("level: \"low\", ");
        assert!(c.required_llm_providers.is_empty());
        assert!(c.requires_llm);
        assert_eq!(c.steps[0].metadata.get("level").unwrap(), "low");
    }

    #[test]
    fn absent_level_defaults_to_medium_and_still_needs_an_llm() {
        let c = compile_llm_step("");
        assert!(c.required_llm_providers.is_empty());
        assert!(
            c.requires_llm,
            "preflight must still check that some backend exists"
        );
        assert_eq!(c.steps[0].metadata.get("level").unwrap(), "medium");
    }

    #[test]
    fn legacy_model_is_a_compile_error() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[(
                "01_ask.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.llm({ description: \"ask\", model: \"gpt-4o-mini\", prompt: () => `hi` });",
            )],
        );
        let errors = compile(tmp.path()).expect_err("legacy model must fail");
        assert!(errors.iter().any(|e| e.to_string().contains("level")));
    }

    #[test]
    fn workflow_without_llm_steps_requires_no_llm() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_x.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"e\", command: () => [\"echo\", \"hi\"] });",
            )],
        );
        let c = compile(tmp.path()).expect("compiles");
        assert!(!c.requires_llm);
    }

    const CODE_STEP: &str = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", run: (x) => x });\n";

    fn goto_branch(then_target: &str, else_target: Option<&str>) -> String {
        let else_line = else_target
            .map(|t| format!("  else: goto(\"{t}\"),\n"))
            .unwrap_or_default();
        format!(
            "import {{ step, goto }} from \"@cori-do/sdk\";\nexport default step.branch({{\n  description: \"route\",\n  if: (input) => true,\n  then: goto(\"{then_target}\"),\n{else_line}}});\n"
        )
    }

    #[test]
    fn goto_resolves_to_the_named_step() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("finish", None)),
                ("02_middle.ts", CODE_STEP),
                ("03_finish.ts", CODE_STEP),
            ],
        );
        let compiled = compile(tmp.path()).expect("goto workflow compiles");
        let nested = compiled.steps[0].metadata.get("nested").unwrap();
        assert_eq!(nested["then"]["goto"], "03_finish");
        assert_eq!(nested["then"]["goto_name"], "finish");
    }

    #[test]
    fn goto_end_finishes_the_run_unless_shadowed() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("end", None)),
                ("02_x.ts", CODE_STEP),
            ],
        );
        let compiled = compile(tmp.path()).expect("goto end compiles");
        assert_eq!(
            compiled.steps[0].metadata.get("nested").unwrap()["then"]["goto"],
            "end"
        );

        let shadowed = tempdir();
        make_workflow(
            shadowed.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("end", None)),
                ("02_end.ts", CODE_STEP),
            ],
        );
        let errs = compile(shadowed.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("reserved")));
    }

    #[test]
    fn goto_rejects_unknown_backward_and_ambiguous_targets() {
        let unknown = tempdir();
        make_workflow(
            unknown.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("nowhere", None)),
                ("02_x.ts", CODE_STEP),
            ],
        );
        let errs = compile(unknown.path()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.reason.contains("unknown step `nowhere`"))
        );

        let backward = tempdir();
        make_workflow(
            backward.path(),
            LLM_MANIFEST,
            &[
                ("01_early.ts", CODE_STEP),
                ("02_route.ts", &goto_branch("early", None)),
                ("03_x.ts", CODE_STEP),
            ],
        );
        let errs = compile(backward.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("routes backward")));

        let ambiguous = tempdir();
        make_workflow(
            ambiguous.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("x", None)),
                ("02_x.ts", CODE_STEP),
                ("03_x.ts", CODE_STEP),
            ],
        );
        let errs = compile(ambiguous.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("names 2 steps")));
    }

    #[test]
    fn goto_rejects_unreachable_steps() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("finish", Some("finish"))),
                ("02_dead.ts", CODE_STEP),
                ("03_finish.ts", CODE_STEP),
            ],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| { e.file == "steps/02_dead.ts" && e.reason.contains("unreachable") })
        );

        // The implicit "false, no else" path keeps the next step live.
        let live = tempdir();
        make_workflow(
            live.path(),
            LLM_MANIFEST,
            &[
                ("01_route.ts", &goto_branch("finish", None)),
                ("02_alive.ts", CODE_STEP),
                ("03_finish.ts", CODE_STEP),
            ],
        );
        compile(live.path()).expect("fall-through path keeps steps reachable");
    }

    #[test]
    fn goto_is_rejected_in_loop_bodies() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[
                (
                    "01_loop.ts",
                    "import { step, goto } from \"@cori-do/sdk\";\nexport default step.loop({ description: \"l\", body: goto(\"finish\"), until: (x) => true });\n",
                ),
                ("02_finish.ts", CODE_STEP),
            ],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("cannot be a `goto")));
    }

    #[test]
    fn builtin_nested_capabilities_must_be_declared() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST, // declares no tools, no servers
            &[(
                "01_route.ts",
                r#"import { step } from "@cori-do/sdk";
export default step.switch({
  description: "route",
  on: ({ kind }) => kind,
  cases: {
    push: step.cli({ description: "push", command: () => ["gh", "pr", "create"] }),
    post: step.mcp_tool({ description: "post", server: "slack", tool: "post_message", args: () => ({}) }),
  },
});"#,
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.reason.contains("gh") && e.reason.contains("tools_required"))
        );
        assert!(
            errs.iter()
                .any(|e| e.reason.contains("slack") && e.reason.contains("mcp_servers"))
        );
    }

    #[test]
    fn builtin_nested_capabilities_satisfy_manifest_declarations() {
        let manifest = "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\ntools_required: [gh]\nmcp_servers: [slack]\n---\n# body\n";
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            manifest,
            &[(
                "01_route.ts",
                r#"import { step } from "@cori-do/sdk";
export default step.switch({
  description: "route",
  on: ({ kind }) => kind,
  cases: {
    push: step.cli({ description: "push", command: () => ["gh", "pr", "create"] }),
    post: step.mcp_tool({ description: "post", server: "slack", tool: "post_message", args: () => ({}) }),
  },
});"#,
            )],
        );
        let compiled = compile(tmp.path()).expect("nested capabilities count as usage");
        assert_eq!(compiled.required_cli_binaries, vec!["gh".to_string()]);
        assert_eq!(compiled.required_mcp_servers, vec!["slack".to_string()]);
        assert!(!compiled.requires_llm);
    }

    #[test]
    fn builtin_nested_llm_marks_workflow_as_requiring_llm() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[(
                "01_branch.ts",
                r#"import { step } from "@cori-do/sdk";
export default step.branch({
  description: "summarise when long",
  if: ({ length }) => length > 1000,
  then: step.llm({ description: "summarise", prompt: () => `s` }),
});"#,
            )],
        );
        let compiled = compile(tmp.path()).expect("compiles");
        assert!(compiled.requires_llm);
    }

    #[test]
    fn builtin_file_must_not_import_node_modules() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            LLM_MANIFEST,
            &[(
                "01_branch.ts",
                "import { step } from \"@cori-do/sdk\";\nimport fs from \"node:fs\";\nexport default step.branch({ description: \"x\", if: (i) => true, then: step.code({ description: \"n\", run: (x) => x }) });",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("node:fs")));
    }

    #[test]
    fn missing_manifest() {
        let tmp = tempdir();
        fs::create_dir(tmp.path().join("steps")).unwrap();
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.file == "manifest.md"));
    }

    #[test]
    fn missing_steps_dir() {
        let tmp = tempdir();
        fs::write(tmp.path().join("manifest.md"), OK_MANIFEST).unwrap();
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.file == "steps/"));
    }

    #[test]
    fn happy_path_two_steps() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[
                (
                    "01_one.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"one\", command: () => [\"echo\", \"hi\"] });\n",
                ),
                (
                    "02_two.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"two\", run: (x) => x });\n",
                ),
            ],
        );
        let w = compile(tmp.path()).unwrap();
        assert_eq!(w.steps.len(), 2);
        assert_eq!(w.steps[0].kind, StepKind::Cli);
        assert_eq!(w.steps[1].kind, StepKind::Code);
        assert_eq!(w.steps[0].activity_id, "01_one");
        assert_eq!(w.steps[1].depends_on, vec!["01_one".to_string()]);
        assert_eq!(w.required_cli_binaries, vec!["echo".to_string()]);
        assert_eq!(
            w.steps[0].source_sha256.as_deref(),
            Some(source_sha256(
                b"import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"one\", command: () => [\"echo\", \"hi\"] });\n"
            ).as_str())
        );
    }

    #[test]
    fn workflow_hash_includes_root_and_nested_imports() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"a\", command: () => [\"echo\"] });\n",
            )],
        );
        let initial = workflow_content_hash(tmp.path()).unwrap();
        assert_eq!(initial.len(), 64, "workflow identity uses full SHA-256");

        fs::write(
            tmp.path().join("types.ts"),
            "export type Row = { id: string };\n",
        )
        .unwrap();
        let with_root_import = workflow_content_hash(tmp.path()).unwrap();
        assert_ne!(with_root_import, initial);

        fs::create_dir_all(tmp.path().join("lib")).unwrap();
        fs::write(
            tmp.path().join("lib/helpers.ts"),
            "export const normalize = (s: string) => s.trim();\n",
        )
        .unwrap();
        let with_nested_import = workflow_content_hash(tmp.path()).unwrap();
        assert_ne!(with_nested_import, with_root_import);

        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".git/index"), b"mutable git metadata").unwrap();
        assert_eq!(
            workflow_content_hash(tmp.path()).unwrap(),
            with_nested_import,
            "git internals are not executable workflow inputs"
        );
    }

    #[test]
    fn workflow_hash_rejects_untrusted_tree_resource_exhaustion() {
        let too_large = tempdir();
        let oversized = too_large.path().join("oversized.bin");
        let file = fs::File::create(&oversized).expect("oversized file");
        file.set_len(MAX_WORKFLOW_SOURCE_FILE_BYTES + 1)
            .expect("sparse oversized file");
        let error = workflow_content_hash(too_large.path()).expect_err("large file must fail");
        assert!(error.to_string().contains("per-file limit"));

        let too_many = tempdir();
        for index in 0..=MAX_WORKFLOW_SOURCE_FILES {
            fs::write(too_many.path().join(format!("{index:04}.txt")), b"x")
                .expect("bounded fixture file");
        }
        let error = workflow_content_hash(too_many.path()).expect_err("file flood must fail");
        assert!(error.to_string().contains("more than"));
    }

    #[test]
    fn workflow_hash_rejects_secret_files_before_source_is_cached() {
        for relative in [
            ".env",
            ".env.production",
            "credentials.json",
            "keys/service-account.pem",
        ] {
            let workflow = tempdir();
            let path = workflow.path().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("secret parent");
            }
            fs::write(&path, "must not be persisted").expect("secret fixture");
            let error =
                workflow_content_hash(workflow.path()).expect_err("secret source must fail closed");
            assert!(
                error.to_string().contains("credential or secret"),
                "{relative}: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn workflow_hash_rejects_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"a\", command: () => [\"echo\"] });\n",
            )],
        );
        let outside = tempdir();
        fs::write(outside.path().join("secret.txt"), "must not be traversed").unwrap();
        symlink(outside.path(), tmp.path().join("linked-outside")).unwrap();

        let error = workflow_content_hash(tmp.path()).expect_err("symlink must be rejected");
        assert!(error.to_string().contains("symlink"));
        assert!(error.to_string().contains("linked-outside"));
    }

    #[test]
    fn gap_in_numbering_rejected() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[
                (
                    "01_a.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"a\", run: (x) => x });\n",
                ),
                (
                    "03_c.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"c\", run: (x) => x });\n",
                ),
            ],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("gap")));
    }

    #[test]
    fn bad_filename_rejected() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "BadName.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", run: (x) => x });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.file == "steps/BadName.ts"));
    }

    #[test]
    fn cli_binary_must_be_declared() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST, // only `echo` declared
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"a\", command: () => [\"kubectl\", \"get\", \"pods\"] });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.reason.contains("kubectl") && e.reason.contains("tools_required"))
        );
    }

    #[test]
    fn unused_cli_declaration_is_rejected() {
        let manifest = "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\ntools_required: [deno, gws]\n---\n# body\n";
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            manifest,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"a\", command: () => [\"deno\", \"eval\", \"new Deno.Command('gws')\"] });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.file == "manifest.md"
                && e.field.as_deref() == Some("tools_required")
                && e.reason.contains("gws")
                && e.reason.contains("directly as argv[0]")
        }));
    }

    #[test]
    fn mcp_server_must_be_declared() {
        let manifest = "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\nmcp_servers: [github]\n---\n# body\n";
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            manifest,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.mcp_tool({ description: \"a\", server: \"slack\", tool: \"post\", args: () => ({}) });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("slack")));
    }

    #[test]
    fn unused_mcp_declaration_is_rejected() {
        let manifest = "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\nmcp_servers: [slack]\n---\n# body\n";
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            manifest,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"a\", run: (x) => x });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.file == "manifest.md"
                && e.field.as_deref() == Some("mcp_servers")
                && e.reason.contains("slack")
        }));
    }

    #[test]
    fn code_must_not_import_node_modules() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nimport fs from \"node:fs\";\nexport default step.code({ description: \"a\", run: (x) => { fs.readFileSync; return x; } });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("node:fs")));
    }

    #[test]
    fn step_without_description_rejected() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.code({ run: (x) => x });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("description")));
    }

    #[test]
    fn parse_error_before_valid_step_returns_diagnostics_instead_of_panicking() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[
                (
                    "01_first.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"first\", run: (x) => x });\n",
                ),
                (
                    "02_invalid.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ run: (x) => x });\n",
                ),
                (
                    "03_later.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"later\", run: (x) => x });\n",
                ),
            ],
        );

        let errs = compile(tmp.path()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| { e.file == "steps/02_invalid.ts" && e.reason.contains("description") })
        );
    }

    #[test]
    fn step_without_default_export_rejected() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nconst x = step.code({ description: \"a\", run: (x) => x });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("default export")));
    }

    #[test]
    fn unknown_kind_rejected() {
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            OK_MANIFEST,
            &[(
                "01_a.ts",
                "import { step } from \"@cori-do/sdk\";\nexport default step.unknown({ description: \"a\" });\n",
            )],
        );
        let errs = compile(tmp.path()).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("unknown")));
    }

    // ----- Draft view (live authoring) -----

    #[test]
    fn draft_renders_states_compile_rejects() {
        // The exact mid-authoring shape from the field: the manifest
        // declares a tool no step uses yet, and a branch routes forward
        // to steps that are not written yet. `compile` rejects both;
        // `draft` must render every step written so far.
        let tmp = tempdir();
        make_workflow(
            tmp.path(),
            "---\nid: hi\nname: Hi\ndescription: greet\ncreated: 2026-05-25\nversion: 1\ntools_required: [echo, python3]\n---\n# body\n",
            &[
                (
                    "01_fetch.ts",
                    "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"fetch\", command: () => [\"echo\", \"hi\"] });\n",
                ),
                ("02_route.ts", &goto_branch("render_report", None)),
            ],
        );
        compile(tmp.path()).expect_err("intermediate state must not compile");

        let d = draft(tmp.path());
        assert_eq!(d.manifest.as_ref().map(|m| m.id.as_str()), Some("hi"));
        assert_eq!(d.steps.len(), 2);
        assert_eq!(d.steps[0].activity_id, "01_fetch");
        assert_eq!(d.steps[0].kind, StepKind::Cli);
        assert_eq!(d.steps[1].kind, StepKind::Builtin);
        assert_eq!(d.steps[1].metadata.get("builtin").unwrap(), "branch");
    }

    #[test]
    fn draft_survives_missing_manifest_and_broken_steps() {
        let tmp = tempdir();
        // No manifest.md at all; one valid step, one half-written file.
        fs::create_dir_all(tmp.path().join("steps")).unwrap();
        fs::write(tmp.path().join("steps/01_ok.ts"), CODE_STEP).unwrap();
        fs::write(
            tmp.path().join("steps/02_wip.ts"),
            "import { step } from \"@cori-do/sdk\";\nexport default step.llm({\n  description: \"unfinished",
        )
        .unwrap();

        let d = draft(tmp.path());
        assert!(d.manifest.is_none());
        assert_eq!(d.steps.len(), 2);
        assert_eq!(d.steps[0].kind, StepKind::Code);
        // The broken file still earns a node, kind sniffed from the call.
        assert_eq!(d.steps[1].activity_id, "02_wip");
        assert_eq!(d.steps[1].kind, StepKind::Llm);
    }

    #[test]
    fn draft_of_empty_folder_is_empty_not_an_error() {
        let tmp = tempdir();
        let d = draft(tmp.path());
        assert!(d.manifest.is_none());
        assert!(d.steps.is_empty());
    }

    // ----- Small tempdir helper (no external dep on `tempfile`). -----
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TmpDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let mut p = std::env::temp_dir();
        p.push(format!("cori-compiler-test-{pid}-{n}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}

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

mod step_parser;

use std::path::{Path, PathBuf};

use cori_manifest::{ManifestError, parse_manifest};
use cori_protocol::{CompiledStep, CompiledWorkflow, Placement, StepKind};
use regex::Regex;
use serde::Serialize;
use thiserror::Error;

pub use step_parser::ParsedStep;

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

/// Map an LLM model name to its provider id. Kept here (and not just in
/// the broker) so the compiler can validate `model: "…"` declarations
/// without taking a dep on the broker crate.
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        Some("openai")
    } else if model.starts_with("claude-") {
        Some("anthropic")
    } else if model.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
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

    // 4. Cross-validation.
    let mut required_cli: Vec<String> = Vec::new();
    let mut required_mcp: Vec<String> = Vec::new();
    let mut required_llm: Vec<String> = Vec::new();
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
                if let Some(model) = step.metadata.get("model").and_then(|v| v.as_str()) {
                    let provider = provider_for_model(model);
                    if let Some(p) = provider {
                        if !required_llm.iter().any(|x| x == p) {
                            required_llm.push(p.to_string());
                        }
                    } else {
                        errors.push(
                            CompileError::new(
                                &rel,
                                format!(
                                    "model `{model}` does not match any known LLM provider (expected prefix: gpt-/o1-/o3-/o4- for OpenAI, claude- for Anthropic, gemini- for Gemini)"
                                ),
                            )
                            .with_field("model"),
                        );
                    }
                }
            }
            _ => {}
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
    })
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
                    format!("duplicate step number {:02}", cur.number),
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

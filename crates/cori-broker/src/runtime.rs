//! Resolves the Deno binary and runner script paths.
//!
//! The runner script and its `deno.json` import map are installed by
//! `cori init --local` into `~/.cori/runtime/`. The broker takes the
//! runtime root as a parameter so tests can point it at a temporary
//! directory.
//!
//! The Deno binary lookup falls back through three candidates, in order:
//!
//! 1. The `CORI_DENO` environment variable.
//! 2. `<runtime>/deno` (where `cori init` would download the pinned binary
//!    in a future update).
//! 3. `deno` on `PATH`.
//!
//! If none resolve, dispatch fails with [`BrokerError::RuntimeUnavailable`]
//! and a message pointing the user at `deno.land`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value as JsonValue;
use thiserror::Error;
use walkdir::WalkDir;

use crate::BrokerError;
use crate::process::hide_console_window;

/// Resolved paths needed to spawn the runner.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub deno_bin: PathBuf,
    pub runner_script: PathBuf,
    pub config_path: PathBuf,
    pub lock_path: PathBuf,
}

impl Runtime {
    /// Resolve from a runtime root, returning [`BrokerError::RuntimeUnavailable`]
    /// if any required file (or the Deno binary) is missing.
    pub fn resolve(runtime_root: &Path) -> crate::Result<Self> {
        let runner_script = runtime_root.join("runner.ts");
        let config_path = runtime_root.join("deno.json");
        let lock_path = runtime_root.join("deno.lock");

        if !runner_script.is_file() {
            return Err(BrokerError::RuntimeUnavailable(format!(
                "runner script missing at `{}` — reinstall Cori or restart the current Cori command",
                runner_script.display()
            )));
        }
        if !config_path.is_file() {
            return Err(BrokerError::RuntimeUnavailable(format!(
                "Deno config missing at `{}` — reinstall Cori or restart the current Cori command",
                config_path.display()
            )));
        }
        if !lock_path.is_file() {
            return Err(BrokerError::RuntimeUnavailable(format!(
                "Deno lockfile missing at `{}` — reinstall Cori or restart the current Cori command",
                lock_path.display()
            )));
        }

        let deno_bin = locate_deno(runtime_root)?;
        Ok(Self {
            deno_bin,
            runner_script,
            config_path,
            lock_path,
        })
    }

    /// Resolve the exact locked npm dependency set before a worker starts
    /// polling. Activities subsequently run in cached-only mode, so workflow
    /// code can never make registry state part of an execution.
    pub fn cache_locked_dependencies(&self) -> crate::Result<()> {
        let runtime_root = self.config_path.parent().ok_or_else(|| {
            BrokerError::RuntimeUnavailable(
                "Deno configuration path has no runtime parent".to_string(),
            )
        })?;
        let mut command = Command::new(&self.deno_bin);
        command
            .arg("cache")
            .arg("--quiet")
            .arg("--config")
            .arg(&self.config_path)
            .arg("--lock")
            .arg(&self.lock_path)
            .arg("--frozen")
            .arg(self.schema_path())
            .arg(runtime_root.join("sdk/index.ts"));
        hide_console_window(&mut command);
        let output = command.output().map_err(BrokerError::Spawn)?;
        if !output.status.success() {
            let diagnostic = if output.stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };
            return Err(BrokerError::RuntimeUnavailable(format!(
                "could not cache the locked Deno runtime dependencies (exit {}): {}",
                output.status.code().unwrap_or(-1),
                if diagnostic.is_empty() {
                    "Deno returned no diagnostic output"
                } else {
                    &diagnostic
                }
            )));
        }
        Ok(())
    }

    fn schema_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("schema.ts")
    }

    /// Parse and type-check every workflow step without evaluating module code.
    ///
    /// Deno already provides the TypeScript runtime used by activities, so this
    /// closes the gap between the compiler's structural metadata extraction and
    /// the first activity import without introducing a second TS toolchain.
    pub fn validate_step_modules(
        &self,
        step_files: &[PathBuf],
        workflow_root: &Path,
    ) -> std::result::Result<(), StepValidationError> {
        if step_files.is_empty() {
            return Ok(());
        }

        let workflow_root = workflow_root
            .canonicalize()
            .map_err(StepValidationError::Root)?;
        let runtime_root = self
            .config_path
            .parent()
            .ok_or_else(|| {
                StepValidationError::Graph(
                    "Deno configuration path has no runtime parent".to_string(),
                )
            })?
            .canonicalize()
            .map_err(StepValidationError::Root)?;
        let runtime_sdk = runtime_root.join("sdk/index.ts");
        validate_workflow_imports(&workflow_root)?;

        let mut command = Command::new(&self.deno_bin);
        command.arg("check").arg("--quiet");
        apply_module_resolution_flags(&mut command);
        command
            .arg("--no-remote")
            .arg("--lock")
            .arg(&self.lock_path)
            .arg("--frozen")
            .arg("--config")
            .arg(&self.config_path)
            .args(step_files);
        hide_console_window(&mut command);
        let output = command.output().map_err(StepValidationError::Spawn)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let diagnostic = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            return Err(StepValidationError::Failed {
                exit_code: output.status.code().unwrap_or(-1),
                diagnostic: if diagnostic.is_empty() {
                    "Deno returned no diagnostic output".to_string()
                } else {
                    diagnostic.to_string()
                },
            });
        }

        for step_file in step_files {
            self.validate_module_graph(step_file, &workflow_root, &runtime_sdk)?;
        }
        Ok(())
    }

    fn validate_module_graph(
        &self,
        step_file: &Path,
        workflow_root: &Path,
        runtime_sdk: &Path,
    ) -> std::result::Result<(), StepValidationError> {
        let mut command = Command::new(&self.deno_bin);
        command.arg("info").arg("--json").arg("--quiet");
        apply_module_resolution_flags(&mut command);
        command
            // Remote URL modules are mutable executable input. npm packages
            // remain supported through the runtime's pinned import mapping.
            .arg("--no-remote")
            .arg("--lock")
            .arg(&self.lock_path)
            .arg("--frozen")
            .arg("--config")
            .arg(&self.config_path)
            .arg(step_file);
        hide_console_window(&mut command);
        let output = command.output().map_err(StepValidationError::Spawn)?;
        if !output.status.success() {
            let diagnostic = if output.stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };
            return Err(StepValidationError::Graph(format!(
                "Deno could not resolve the module graph for `{}` (exit {}): {}",
                step_file.display(),
                output.status.code().unwrap_or(-1),
                if diagnostic.is_empty() {
                    "no diagnostic output"
                } else {
                    &diagnostic
                }
            )));
        }

        let graph: JsonValue = serde_json::from_slice(&output.stdout).map_err(|error| {
            StepValidationError::Graph(format!(
                "Deno returned invalid module-graph JSON for `{}`: {error}",
                step_file.display()
            ))
        })?;
        let modules = graph
            .get("modules")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                StepValidationError::Graph(format!(
                    "Deno module graph for `{}` has no `modules` array",
                    step_file.display()
                ))
            })?;
        for module in modules {
            let Some(specifier) = module.get("specifier").and_then(JsonValue::as_str) else {
                continue;
            };
            if !specifier.starts_with("file:") {
                if !matches!(specifier, "npm:/zod@4.4.3" | "npm:zod@4.4.3") {
                    return Err(StepValidationError::Graph(format!(
                        "module `{specifier}` is not an allowed runtime-owned dependency; workflow modules may import only local frozen files, `@cori-do/sdk`, and the pinned `zod` alias"
                    )));
                }
                continue;
            }
            let module_url = url::Url::parse(specifier).map_err(|error| {
                StepValidationError::Graph(format!(
                    "invalid file module URL `{specifier}` in `{}`: {error}",
                    step_file.display()
                ))
            })?;
            let module_path = module_url.to_file_path().map_err(|()| {
                StepValidationError::Graph(format!(
                    "could not convert file module URL `{specifier}` to a local path"
                ))
            })?;
            let canonical = module_path.canonicalize().map_err(|error| {
                StepValidationError::Graph(format!(
                    "could not resolve file module `{}`: {error}",
                    module_path.display()
                ))
            })?;
            if !canonical.starts_with(workflow_root) && canonical != runtime_sdk {
                return Err(StepValidationError::GraphEscape {
                    step: step_file.to_path_buf(),
                    module: canonical,
                });
            }
            if canonical.starts_with(workflow_root) {
                let source = std::fs::read_to_string(&canonical).map_err(|error| {
                    StepValidationError::Graph(format!(
                        "reading workflow module `{}` while checking dynamic imports: {error}",
                        canonical.display()
                    ))
                })?;
                if contains_dynamic_import(&source) {
                    return Err(StepValidationError::Graph(format!(
                        "workflow module `{}` uses dynamic `import(...)`; computed module loading is not allowed because it cannot be frozen into the verified source graph",
                        canonical.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

static STATIC_IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ms)\b(?:import\s+(?:type\s+)?(?:[^;"']*?\s+from\s*)?|export\s+(?:type\s+)?[^;"']*?\s+from\s*)["']([^"']+)["']"#,
    )
    .expect("static import regex")
});

fn validate_workflow_imports(workflow_root: &Path) -> Result<(), StepValidationError> {
    for entry in WalkDir::new(workflow_root).follow_links(false) {
        let entry = entry.map_err(|error| {
            StepValidationError::Graph(format!(
                "walking workflow source `{}`: {error}",
                workflow_root.display()
            ))
        })?;
        if !entry.file_type().is_file()
            || !matches!(
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("ts" | "tsx" | "js" | "jsx" | "mjs")
            )
        {
            continue;
        }
        let source = std::fs::read_to_string(entry.path()).map_err(|error| {
            StepValidationError::Graph(format!(
                "reading workflow module `{}`: {error}",
                entry.path().display()
            ))
        })?;
        if contains_dynamic_import(&source) {
            return Err(StepValidationError::Graph(format!(
                "workflow module `{}` uses dynamic `import(...)`; computed module loading is not allowed because it cannot be frozen into the verified source graph",
                entry.path().display()
            )));
        }
        for captures in STATIC_IMPORT_RE.captures_iter(&source) {
            let Some(specifier) = captures.get(1).map(|capture| capture.as_str()) else {
                continue;
            };
            if specifier == "@cori-do/sdk"
                || specifier == "zod"
                || specifier.starts_with("./")
                || specifier.starts_with("../")
            {
                continue;
            }
            return Err(StepValidationError::Graph(format!(
                "workflow module `{}` imports unsupported specifier `{specifier}`; use only relative frozen modules, `@cori-do/sdk`, or the pinned `zod` alias",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn contains_dynamic_import(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut search_from = 0_usize;
    while let Some(relative) = source[search_from..].find("import") {
        let start = search_from + relative;
        let end = start + "import".len();
        let identifier = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$');
        if start > 0 && identifier(bytes[start - 1]) || end < bytes.len() && identifier(bytes[end])
        {
            search_from = end;
            continue;
        }

        let mut cursor = end;
        loop {
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor..cursor + 2) == Some(b"//") {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
                continue;
            }
            if bytes.get(cursor..cursor + 2) == Some(b"/*") {
                let Some(close) = source[cursor + 2..].find("*/") else {
                    return false;
                };
                cursor += 2 + close + 2;
                continue;
            }
            break;
        }
        if bytes.get(cursor) == Some(&b'(') {
            return true;
        }
        search_from = end;
    }
    false
}

/// Apply the module-resolution flags shared by preflight and activity imports.
///
/// Cori's shipped workflows intentionally use extensionless local imports such
/// as `../types`; keeping this helper shared prevents the checker and runner
/// from accepting different module graphs.
pub(crate) fn apply_module_resolution_flags(command: &mut Command) {
    command.arg("--sloppy-imports");
}

/// Failure from the non-executing TypeScript validation gate.
#[derive(Debug, Error)]
pub enum StepValidationError {
    #[error("failed to start Deno workflow validation: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("workflow TypeScript validation failed (Deno exit {exit_code}):\n{diagnostic}")]
    Failed { exit_code: i32, diagnostic: String },

    #[error("could not resolve a workflow/runtime root: {0}")]
    Root(#[source] std::io::Error),

    #[error("workflow TypeScript module graph validation failed: {0}")]
    Graph(String),

    #[error(
        "workflow step `{step}` imports local module `{module}` outside its frozen workflow snapshot"
    )]
    GraphEscape { step: PathBuf, module: PathBuf },
}

fn locate_deno(runtime_root: &Path) -> crate::Result<PathBuf> {
    if let Ok(env) = std::env::var("CORI_DENO")
        && !env.is_empty()
    {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Ok(p);
        }
        return Err(BrokerError::RuntimeUnavailable(format!(
            "$CORI_DENO is set to `{}` but no file exists there",
            p.display()
        )));
    }

    let bundled = runtime_root.join(if cfg!(windows) { "deno.exe" } else { "deno" });
    if bundled.is_file() {
        return Ok(bundled);
    }

    if let Some(found) = which("deno") {
        return Ok(found);
    }

    Err(BrokerError::RuntimeUnavailable(
        "no `deno` binary found on PATH and none installed at the runtime root".to_string(),
    ))
}

/// Minimal cross-platform PATH lookup — avoids adding a `which` dependency
/// just for this. Returns `None` if nothing matches.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        for suffix in exe_suffixes {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn validation_surfaces_the_deno_diagnostic() {
        let temp = tempdir().expect("temporary runtime");
        let deno = temp.path().join("deno");
        fs::write(
            &deno,
            "#!/bin/sh\nprintf '%s\\n' 'error: SyntaxError: Expression expected at steps/02_bad.ts:13:9' >&2\nexit 1\n",
        )
        .expect("fake deno");
        fs::set_permissions(&deno, fs::Permissions::from_mode(0o755))
            .expect("fake deno permissions");

        let runtime = Runtime {
            deno_bin: deno,
            runner_script: temp.path().join("runner.ts"),
            config_path: temp.path().join("deno.json"),
            lock_path: temp.path().join("deno.lock"),
        };
        let error = runtime
            .validate_step_modules(&[temp.path().join("steps/02_bad.ts")], temp.path())
            .expect_err("validation should fail");

        assert!(matches!(
            error,
            StepValidationError::Failed { diagnostic, .. }
                if diagnostic.contains("steps/02_bad.ts:13:9")
        ));
    }

    #[test]
    fn validation_enables_the_runtime_sloppy_import_resolution() {
        let temp = tempdir().expect("temporary runtime");
        let deno = temp.path().join("deno");
        fs::write(
            &deno,
            "#!/bin/sh\ncase \" $* \" in *\" --sloppy-imports \"*) ;; *) exit 9;; esac\ncase \"$1\" in check) exit 0;; info) printf '%s\\n' '{\"modules\":[]}' ;; *) exit 8;; esac\n",
        )
        .expect("fake deno");
        fs::set_permissions(&deno, fs::Permissions::from_mode(0o755))
            .expect("fake deno permissions");

        let runtime = Runtime {
            deno_bin: deno,
            runner_script: temp.path().join("runner.ts"),
            config_path: temp.path().join("deno.json"),
            lock_path: temp.path().join("deno.lock"),
        };
        runtime
            .validate_step_modules(
                &[temp.path().join("steps/01_extensionless_import.ts")],
                temp.path(),
            )
            .expect("validation should match activity module resolution");
    }

    #[test]
    fn no_step_files_need_no_subprocess() {
        let runtime = Runtime {
            deno_bin: PathBuf::from("does-not-exist"),
            runner_script: PathBuf::from("runner.ts"),
            config_path: PathBuf::from("deno.json"),
            lock_path: PathBuf::from("deno.lock"),
        };

        runtime
            .validate_step_modules(&[], Path::new("does-not-exist"))
            .expect("empty validation should be a no-op");
    }

    #[test]
    fn validation_rejects_file_imports_outside_workflow() {
        let temp = tempdir().expect("temporary root");
        let runtime_root = temp.path().join("runtime");
        let workflow_root = temp.path().join("workflow");
        let outside = temp.path().join("outside.ts");
        fs::create_dir_all(workflow_root.join("steps")).expect("workflow steps");
        fs::create_dir_all(&runtime_root).expect("runtime root");
        fs::write(&outside, "export const secret = true;\n").expect("outside module");

        let deno = runtime_root.join("deno");
        let canonical_outside = outside.canonicalize().expect("canonical outside module");
        let outside_url = url::Url::from_file_path(&canonical_outside).expect("outside file URL");
        fs::write(
            &deno,
            format!(
                "#!/bin/sh\ncase \"$1\" in check) exit 0;; info) printf '%s\\n' '{{\"modules\":[{{\"specifier\":\"{outside_url}\"}}]}}' ;; *) exit 8;; esac\n"
            ),
        )
        .expect("fake deno");
        fs::set_permissions(&deno, fs::Permissions::from_mode(0o755))
            .expect("fake deno permissions");
        let config_path = runtime_root.join("deno.json");
        fs::write(&config_path, "{}").expect("runtime config");
        let runtime = Runtime {
            deno_bin: deno,
            runner_script: runtime_root.join("runner.ts"),
            config_path,
            lock_path: runtime_root.join("deno.lock"),
        };

        let error = runtime
            .validate_step_modules(&[workflow_root.join("steps/01_escape.ts")], &workflow_root)
            .expect_err("external file import must fail");
        assert!(matches!(
            error,
            StepValidationError::GraphEscape { module, .. } if module == canonical_outside
        ));
    }

    #[test]
    fn validation_rejects_relative_imports_into_runtime_internals() {
        let temp = tempdir().expect("temporary root");
        let runtime_root = temp.path().join("runtime");
        let workflow_root = temp.path().join("workflow");
        let runtime_schema = runtime_root.join("schema.ts");
        fs::create_dir_all(workflow_root.join("steps")).expect("workflow steps");
        fs::create_dir_all(runtime_root.join("sdk")).expect("runtime root");
        fs::write(&runtime_schema, "export const internal = true;\n").expect("runtime module");
        fs::write(runtime_root.join("sdk/index.ts"), "export {};\n").expect("runtime sdk");

        let deno = runtime_root.join("deno");
        let canonical_schema = runtime_schema
            .canonicalize()
            .expect("canonical runtime module");
        let schema_url =
            url::Url::from_file_path(&canonical_schema).expect("runtime module file URL");
        fs::write(
            &deno,
            format!(
                "#!/bin/sh\ncase \"$1\" in check) exit 0;; info) printf '%s\\n' '{{\"modules\":[{{\"specifier\":\"{schema_url}\"}}]}}' ;; *) exit 8;; esac\n"
            ),
        )
        .expect("fake deno");
        fs::set_permissions(&deno, fs::Permissions::from_mode(0o755))
            .expect("fake deno permissions");
        let config_path = runtime_root.join("deno.json");
        fs::write(&config_path, "{}").expect("runtime config");
        let runtime = Runtime {
            deno_bin: deno,
            runner_script: runtime_root.join("runner.ts"),
            config_path,
            lock_path: runtime_root.join("deno.lock"),
        };

        let error = runtime
            .validate_step_modules(
                &[workflow_root.join("steps/01_runtime_escape.ts")],
                &workflow_root,
            )
            .expect_err("runtime-internal file import must fail");
        assert!(matches!(
            error,
            StepValidationError::GraphEscape { module, .. } if module == canonical_schema
        ));
    }

    #[test]
    fn dynamic_import_detection_covers_spacing_and_comments() {
        assert!(contains_dynamic_import("const value = import(name);"));
        assert!(contains_dynamic_import(
            "const value = import /* deliberately hidden */ (name);"
        ));
        assert!(contains_dynamic_import(
            "const value = `example ${import(name)}`;"
        ));
        assert!(!contains_dynamic_import(
            "import { step } from \"@cori-do/sdk\";"
        ));
        assert!(!contains_dynamic_import("const important = true;"));
    }
}

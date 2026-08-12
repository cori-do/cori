//! Dispatch a `cli` step.
//!
//! The flow is:
//!
//! 1. Invoke the runner in `cli_command` mode to materialise the argv +
//!    optional env additions from the user's `command(input)` builder.
//! 2. For the built-in `cori-sap` capability, resolve the owner/target-bound
//!    credential and execute the exact typed read through the linked
//!    `cori_sap` library. No PATH binary or credential-bearing child exists.
//! 3. For every other CLI, resolve the binary on the worker's PATH and spawn
//!    it with `std::process::Command`, capturing stdout, stderr, and exit code.
//! 4. Invoke the runner in `cli_parse` mode to translate stdout into the
//!    typed output (or `JSON.parse(stdout)` when no `parse` is declared).

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::capabilities::Capabilities;
use crate::cli_auth::{self, AuthState};
use crate::dispatch::{self, RunnerMode};
use crate::process::hide_console_window;
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result};

#[derive(Debug, Deserialize)]
struct CommandSpec {
    command: Vec<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
}

/// Run one `cli` step.
pub fn run(
    runtime: &Runtime,
    capabilities: &Capabilities,
    step_file_path: &Path,
    input: &JsonValue,
    user_id: &str,
    credentials_dir: &Path,
    expected_binary: Option<&str>,
) -> Result<ActivityOutcome> {
    let started = Instant::now();

    // 1. Resolve argv via the runner.
    let cmd_call =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::CliCommand, input)?;
    let spec: CommandSpec =
        serde_json::from_value(cmd_call.output.clone()).map_err(|e| BrokerError::BadEnvelope {
            envelope: cmd_call.output.to_string(),
            source: e,
        })?;
    let argv = spec.command;
    let binary = argv
        .first()
        .cloned()
        .ok_or_else(|| BrokerError::StepFailed {
            message: "cli step produced an empty command".to_string(),
            stack: None,
        })?;
    validate_binary_boundary(expected_binary, &binary)?;
    let workflow_policy = cli_auth::workflow_policy_for_binary(&binary);
    validate_step_env_boundary(&binary, spec.env.as_ref(), workflow_policy)?;

    // 2. Per-CLI auth check (Phase 5). For known CLIs that carry their
    //     own login state (e.g. `gws`), refuse to spawn when the CLI is
    //     not authenticated so the user sees a clean `NeedsReauth`
    //     instead of an opaque 401 from the CLI itself.
    let sap_credential = if binary == "cori-sap" {
        Some(
            cli_auth::sap::access_token_for_owner(user_id, credentials_dir)
                .map_err(|error| sap_credential_error(error, user_id))?,
        )
    } else {
        if let AuthState::NeedsReauth { hint } = cli_auth::check_known(&binary) {
            return Err(BrokerError::NeedsReauth {
                server_id: binary.clone(),
                owner_kind: "user",
                owner_id: user_id.to_string(),
                auth_kind: "cli",
                hint,
            });
        }
        None
    };

    // 3. Execute the data plane. SAP branches before binary resolution and
    //    consumes the credential in-process; generic CLIs retain the existing
    //    subprocess path.
    let execution = execute_data_plane(
        capabilities,
        &binary,
        &argv,
        spec.env.as_ref(),
        workflow_policy,
        sap_credential,
        user_id,
    )?;
    let stdout_str = execution.stdout;
    let stderr_str = execution.stderr;
    let exit_code = execution.exit_code;

    // 4. Parse stdout via the runner.
    let parse_payload = json!({
        "input": input,
        "parseCtx": {
            "stdout": stdout_str,
            "stderr": stderr_str,
            "exitCode": exit_code,
        }
    });
    let parse_call = dispatch::invoke(
        runtime,
        step_file_path,
        RunnerMode::CliParse,
        &parse_payload,
    )?;

    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: parse_call.output,
        duration: started.elapsed(),
        stderr: combine_stderr(&cmd_call.stderr, &stderr_str, &parse_call.stderr),
        cost_eur: None,
        usage: None,
        notes: Vec::new(),
    })
}

fn sap_credential_error(error: cli_auth::sap::SapCredentialError, user_id: &str) -> BrokerError {
    use cli_auth::sap::SapCredentialError;

    match error {
        SapCredentialError::MissingToken
        | SapCredentialError::InvalidToken
        | SapCredentialError::TargetChanged => BrokerError::NeedsReauth {
            server_id: "cori-sap".to_string(),
            owner_kind: "user",
            owner_id: user_id.to_string(),
            auth_kind: "cli",
            hint: "run: cori login cori-sap".to_string(),
        },
        SapCredentialError::Store(_) => BrokerError::CapabilityDenied {
            kind: "SAP credential",
            name: "cori-sap".to_string(),
            hint: "SAP credential store is unavailable; unlock the OS keychain and try again"
                .to_string(),
        },
        error => BrokerError::CapabilityDenied {
            kind: "SAP credential",
            name: "cori-sap".to_string(),
            hint: error.to_string(),
        },
    }
}

#[derive(Debug)]
struct DataPlaneOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn map_sap_adapter_error(error: cori_sap::AdapterError, user_id: &str) -> BrokerError {
    let code = error.code();
    if matches!(
        code,
        "authentication_required" | "authentication_failed" | "invalid_bearer_token"
    ) {
        return BrokerError::NeedsReauth {
            server_id: "cori-sap".to_string(),
            owner_kind: "user",
            owner_id: user_id.to_string(),
            auth_kind: "cli",
            hint: "run: cori login cori-sap".to_string(),
        };
    }
    // SAP workflow reads never opt into Temporal retries in v1. Even errors
    // that the standalone diagnostic binary labels retryable become a stable,
    // non-retryable StepFailed here; the operator may start a fresh run.
    BrokerError::StepFailed {
        message: format!("cori-sap workflow request failed without automatic retry: `{code}`"),
        stack: None,
    }
}

fn execute_data_plane(
    capabilities: &Capabilities,
    binary: &str,
    argv: &[String],
    step_env: Option<&HashMap<String, String>>,
    workflow_policy: cli_auth::WorkflowPolicy,
    sap_credential: Option<cli_auth::sap::SapCredential>,
    user_id: &str,
) -> Result<DataPlaneOutput> {
    if binary == "cori-sap" {
        let credential = sap_credential.ok_or_else(|| BrokerError::StepFailed {
            message: "SAP credential resolution was not completed".to_string(),
            stack: None,
        })?;
        let cli_auth::sap::SapCredential {
            access_token,
            profile,
        } = credential;
        let stdout = cori_sap::execute_workflow_argv(profile, access_token, argv)
            .map_err(|error| map_sap_adapter_error(error, user_id))?;
        return Ok(DataPlaneOutput {
            stdout,
            stderr: String::new(),
            exit_code: 0,
        });
    }

    // Fast path: the startup snapshot. Fall back to a live PATH probe — the
    // compiler already enforced declaration in `tools_required`, so this only
    // catches a binary installed after a long-running worker started.
    let resolved_bin = match capabilities.cli_binaries.get(binary) {
        Some(path) => path.clone(),
        None => crate::install::resolve_binary(binary).ok_or_else(|| {
            BrokerError::CapabilityDenied {
                kind: "CLI",
                name: binary.to_string(),
                hint: format!(
                    "binary `{binary}` declared in `tools_required` but not found on this worker's PATH"
                ),
            }
        })?,
    };

    // Adapter-provided headless env first (never overrides the parent
    // environment), then step-declared env (always wins).
    let mut cmd = Command::new(&resolved_bin);
    cmd.args(&argv[1..]);
    crate::process::scrub_sap_env(&mut cmd);
    if let Some(adapter) = cli_auth::for_binary(binary) {
        cli_auth::apply_spawn_env(&mut cmd, adapter);
    }
    apply_workflow_env(&mut cmd, step_env, workflow_policy);
    // Workflow-declared env is applied after adapter defaults, so scrub at
    // the final spawn boundary as well. No generic child may receive an
    // ambient or explicitly restored standalone SAP token.
    crate::process::scrub_sap_env(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut cmd);

    let proc_output = cmd.output().map_err(|source| BrokerError::CliSpawn {
        binary: binary.to_string(),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&proc_output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&proc_output.stderr).into_owned();
    let exit_code = proc_output.status.code().unwrap_or(-1);
    if !proc_output.status.success() {
        return Err(BrokerError::CliExitNonZero {
            binary: binary.to_string(),
            exit_code,
            stderr: truncate(&stderr, 4096),
        });
    }
    Ok(DataPlaneOutput {
        stdout,
        stderr,
        exit_code,
    })
}

/// Enforce the compiler's static argv[0] at the last point before process
/// resolution. `None` remains available to callers that have not performed
/// static step parsing; Cori activities always pass `Some`.
pub(crate) fn validate_binary_boundary(expected: Option<&str>, actual: &str) -> Result<()> {
    if actual == "cori-sap" && expected != Some("cori-sap") {
        return Err(BrokerError::CapabilityDenied {
            kind: "CLI",
            name: actual.to_string(),
            hint: "linked SAP dispatch requires compiler-frozen `cori-sap` metadata; start a new run from current workflow source"
                .to_string(),
        });
    }
    if let Some(expected) = expected
        && expected != actual
    {
        return Err(BrokerError::CapabilityDenied {
            kind: "CLI",
            name: actual.to_string(),
            hint: format!(
                "step was compiled for `{expected}` but its command builder produced `{actual}`; argv[0] must remain the directly declared executable"
            ),
        });
    }
    Ok(())
}

fn validate_step_env_boundary(
    binary: &str,
    step_env: Option<&HashMap<String, String>>,
    policy: cli_auth::WorkflowPolicy,
) -> Result<()> {
    if !policy.allow_step_env && step_env.is_some() {
        return Err(BrokerError::CapabilityDenied {
            kind: "CLI environment",
            name: binary.to_string(),
            hint: format!(
                "`{binary}` does not allow workflow-declared environment variables; configure credentials and endpoints with the CLI outside the workflow"
            ),
        });
    }
    Ok(())
}

fn apply_workflow_env(
    cmd: &mut Command,
    step_env: Option<&HashMap<String, String>>,
    policy: cli_auth::WorkflowPolicy,
) {
    if let Some(env) = step_env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    // Adapter-enforced variables are applied last so step code cannot
    // override them. The built-in SAP path never reaches process execution.
    for (k, v) in policy.forced_env {
        cmd.env(k, v);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("\n…(truncated)");
        t
    }
}

fn combine_stderr(runner_a: &str, child: &str, runner_b: &str) -> String {
    let mut out = String::new();
    if !runner_a.trim().is_empty() {
        out.push_str("[runner: cli_command]\n");
        out.push_str(runner_a);
        if !runner_a.ends_with('\n') {
            out.push('\n');
        }
    }
    if !child.trim().is_empty() {
        out.push_str("[cli stderr]\n");
        out.push_str(child);
        if !child.ends_with('\n') {
            out.push('\n');
        }
    }
    if !runner_b.trim().is_empty() {
        out.push_str("[runner: cli_parse]\n");
        out.push_str(runner_b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_compiled_binary() {
        validate_binary_boundary(Some("gws"), "gws").expect("matching binary");
    }

    #[test]
    fn rejects_a_runtime_binary_switch() {
        let error = validate_binary_boundary(Some("gws"), "deno")
            .expect_err("runtime argv must not cross the compiled boundary");
        assert!(matches!(
            error,
            BrokerError::CapabilityDenied { name, hint, .. }
                if name == "deno" && hint.contains("compiled for `gws`")
        ));
    }

    #[test]
    fn cori_sap_rejects_even_an_empty_declared_env() {
        let env = HashMap::new();
        let error = validate_step_env_boundary(
            "cori-sap",
            Some(&env),
            cli_auth::workflow_policy_for_binary("cori-sap"),
        )
        .expect_err("cori-sap workflow env must be closed");
        assert!(matches!(
            error,
            BrokerError::CapabilityDenied {
                kind: "CLI environment",
                ..
            }
        ));
    }

    #[test]
    fn generic_workflow_env_is_applied() {
        let mut declared = HashMap::new();
        declared.insert("UNRELATED".to_string(), "kept".to_string());

        let mut cmd = Command::new("generic-cli");
        apply_workflow_env(
            &mut cmd,
            Some(&declared),
            cli_auth::workflow_policy_for_binary("generic-cli"),
        );

        let env: HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("UNRELATED"))
                .and_then(|value| *value),
            Some(std::ffi::OsStr::new("kept"))
        );
    }

    #[test]
    fn generic_children_cannot_receive_reserved_sap_env() {
        let mut cmd = Command::new("env");
        cmd.env(cli_auth::sap::ACCESS_TOKEN_ENV, "ambient-or-declared-token");
        crate::process::scrub_sap_env(&mut cmd);

        let env: HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new(cli_auth::sap::ACCESS_TOKEN_ENV)),
            Some(&None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn generic_cli_step_cannot_restore_the_reserved_sap_token() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temporary executable directory");
        let executable = temp.path().join("generic-cli");
        std::fs::write(
            &executable,
            concat!(
                "#!/bin/sh\n",
                "if [ -n \"${SAP_ACCESS_TOKEN+x}\" ]; then ",
                "printf present; else printf absent; fi\n"
            ),
        )
        .expect("generic executable");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("generic executable permissions");

        let mut capabilities = Capabilities::default();
        capabilities
            .cli_binaries
            .insert("generic-cli".to_string(), executable);
        let declared_env = HashMap::from([(
            cli_auth::sap::ACCESS_TOKEN_ENV.to_string(),
            "workflow-restored-token".to_string(),
        )]);
        let argv = ["generic-cli".to_string()];
        let output = execute_data_plane(
            &capabilities,
            "generic-cli",
            &argv,
            Some(&declared_env),
            cli_auth::workflow_policy_for_binary("generic-cli"),
            None,
            "alice",
        )
        .expect("generic CLI execution");
        assert_eq!(output.stdout, "absent");
    }

    #[cfg(unix)]
    #[test]
    fn linked_sap_dispatch_never_executes_a_malicious_discovered_binary() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temporary Cori home");
        std::fs::write(
            temp.path().join("sap.toml"),
            r#"
default_profile = "production"
[profiles.production]
base_url = "https://tenant.example.com"
sap_client = "100"
"#,
        )
        .expect("SAP config");
        let profile = cori_sap::load_default_machine_profile_from_cori_home(temp.path())
            .expect("SAP profile");
        let credential = cli_auth::sap::SapCredential {
            access_token: "owner-token".to_string(),
            profile,
        };

        let marker = temp.path().join("malicious-binary-ran");
        let malicious = temp.path().join("cori-sap");
        std::fs::write(
            &malicious,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .expect("malicious binary");
        std::fs::set_permissions(&malicious, std::fs::Permissions::from_mode(0o755))
            .expect("malicious binary permissions");
        let mut capabilities = Capabilities::default();
        capabilities
            .cli_binaries
            .insert("cori-sap".to_string(), malicious);

        let argv = ["cori-sap", "config", "set"].map(str::to_string);
        let error = execute_data_plane(
            &capabilities,
            "cori-sap",
            &argv,
            None,
            cli_auth::workflow_policy_for_binary("cori-sap"),
            Some(credential),
            "alice",
        )
        .expect_err("unknown SAP commands fail in the linked parser");
        assert!(matches!(error, BrokerError::StepFailed { .. }));
        assert!(!marker.exists(), "PATH-backed cori-sap must never run");
    }

    #[test]
    fn sap_auth_failures_become_needs_reauth_without_leaking_sap_context() {
        let error = map_sap_adapter_error(
            cori_sap::AdapterError::HttpStatus {
                status: 401,
                sap_code: Some("SENSITIVE/BUSINESS-CODE".to_string()),
            },
            "alice",
        );
        assert!(matches!(
            error,
            BrokerError::NeedsReauth { ref owner_id, .. } if owner_id == "alice"
        ));
        assert!(!error.to_string().contains("SENSITIVE"));
    }

    #[test]
    fn sap_keychain_errors_are_not_reflected() {
        let error = sap_credential_error(
            cli_auth::sap::SapCredentialError::Store(cori_secrets::SecretError::Keychain(
                "SENSITIVE KEYCHAIN DETAIL".to_string(),
            )),
            "alice",
        );
        assert!(matches!(error, BrokerError::CapabilityDenied { .. }));
        assert!(!error.to_string().contains("SENSITIVE"));
        assert!(error.to_string().contains("unlock the OS keychain"));
    }

    #[test]
    fn sap_transient_failures_are_explicitly_non_retryable_in_workflows() {
        for transient in [
            cori_sap::AdapterError::Transport,
            cori_sap::AdapterError::HttpStatus {
                status: 503,
                sap_code: Some("SENSITIVE/BUSINESS-CODE".to_string()),
            },
        ] {
            let error = map_sap_adapter_error(transient, "alice");
            assert!(matches!(error, BrokerError::StepFailed { .. }));
            assert!(error.to_string().contains("without automatic retry"));
            assert!(!error.to_string().contains("SENSITIVE"));
        }

        let permanent = map_sap_adapter_error(
            cori_sap::AdapterError::InvalidInput {
                field: "id",
                reason: "must be bounded",
            },
            "alice",
        );
        assert!(matches!(permanent, BrokerError::StepFailed { .. }));
    }

    #[test]
    fn permits_legacy_inputs_without_an_expected_binary() {
        validate_binary_boundary(None, "gws").expect("legacy payload compatibility");
    }

    #[test]
    fn linked_sap_rejects_legacy_inputs_without_frozen_metadata() {
        let error = validate_binary_boundary(None, "cori-sap")
            .expect_err("linked SAP requires compiler-frozen metadata");
        assert!(matches!(
            error,
            BrokerError::CapabilityDenied { hint, .. }
                if hint.contains("compiler-frozen `cori-sap` metadata")
        ));
    }
}

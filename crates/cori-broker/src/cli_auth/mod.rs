//! Per-CLI authentication adapters.
//!
//! Many CLIs Cori dispatches (`gws`, `gh`, `aws`, …) carry their own
//! login state. When the broker is about to spawn one of these, the
//! adapter probes whether the CLI is currently authed — if not, the
//! step fails fast with a clean `NeedsReauth` instead of executing and
//! emitting a confusing 401.
//!
//! Adapters are intentionally tiny: each one knows how to run a
//! "whoami"-style probe and how to suggest a re-auth command. v1 ships
//! `gws`; unknown CLIs are passed through without an auth check (the
//! whitelist in [`crate::capabilities`] still gates them).

pub mod gws;

use std::path::PathBuf;
use std::sync::OnceLock;

/// Result of an adapter probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    Ok,
    /// Adapter is confident the CLI is not authenticated. `hint` is a
    /// one-line command the user can run to fix it.
    NeedsReauth {
        hint: String,
    },
    /// The probe itself failed (e.g. binary not found, timed out). We
    /// treat this as `Ok` to avoid false positives — the actual spawn
    /// will surface any real problem with its own error.
    Unknown,
}

/// An OAuth client Cori owns and provisions into a vendor CLI so the
/// user never creates one themselves. For desktop ("installed app")
/// clients the secret is not a confidential value per the vendors' own
/// docs — but we still keep it out of argv and never print it.
#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
    /// GCP project id owning the client. Some vendor CLIs (gws) refuse
    /// a client config without it. Optional: adapters derive a
    /// fallback (e.g. the project number embedded in the client id).
    pub project_id: Option<String>,
}

/// A fully-materialised managed-login plan for one CLI: where to write
/// the provisioned OAuth client, what to write, and what to run to
/// finish the interactive sign-in.
#[derive(Debug, Clone)]
pub struct ManagedLogin {
    /// Where the vendor CLI expects its OAuth client config
    /// (e.g. `~/.config/gws/client_secret.json`).
    pub client_config_path: PathBuf,
    /// Rendered contents for that file.
    pub client_config: String,
    /// Overwrite an existing client config. Default policy is
    /// hands-off (a user-managed file always wins); adapters set this
    /// only when the existing file is identifiably a broken previous
    /// Cori provisioning that the vendor CLI rejects.
    pub overwrite_existing: bool,
    /// argv (binary first) that runs the vendor's own browser sign-in.
    pub login_argv: Vec<String>,
}

pub trait CliAuthAdapter: Send + Sync {
    /// The binary this adapter probes (`"gws"`, `"gh"`, …).
    fn binary(&self) -> &'static str;
    /// Probe the CLI's current auth state.
    fn check(&self) -> AuthState;
    /// Human-readable name of the underlying account / service.
    fn display_name(&self) -> &'static str {
        self.binary()
    }
    /// One-line `cori login`-style hint shown by `cori login <bin>`.
    fn login_hint(&self) -> String {
        format!("see `{} --help`", self.binary())
    }
    /// Cori-owned OAuth client baked in at release-build time (via
    /// `option_env!`). `None` in dev builds without the env vars.
    fn baked_client(&self) -> Option<OAuthClient> {
        None
    }
    /// Managed login: given an [`OAuthClient`] Cori owns, produce the
    /// provisioning + sign-in plan. `None` means this CLI only supports
    /// the manual flow (the default).
    fn managed_login(&self, _client: &OAuthClient, _services: &[String]) -> Option<ManagedLogin> {
        None
    }
    /// Environment Cori applies to every process it spawns for this CLI
    /// (auth probes, managed login, workflow steps) — vendor-specific
    /// headless knobs. Each variable is only applied when not already
    /// set in the parent environment: an explicit user setting wins.
    fn spawn_env(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// Apply [`CliAuthAdapter::spawn_env`] to a command, honouring the
/// parent environment (a user-set variable is never overridden).
pub fn apply_spawn_env(cmd: &mut std::process::Command, adapter: &dyn CliAuthAdapter) {
    for (k, v) in adapter.spawn_env() {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
}

/// Resolve the OAuth client Cori should provision for `capability`, in
/// precedence order: runtime env (`CORI_<ID>_OAUTH_CLIENT_ID/SECRET`) →
/// the caller-supplied config value → the adapter's baked-in release
/// client.
pub fn resolve_client(capability: &str, from_config: Option<OAuthClient>) -> Option<OAuthClient> {
    let prefix = format!(
        "CORI_{}_OAUTH_CLIENT",
        capability.to_ascii_uppercase().replace('-', "_")
    );
    if let (Ok(id), Ok(secret)) = (
        std::env::var(format!("{prefix}_ID")),
        std::env::var(format!("{prefix}_SECRET")),
    ) && !id.is_empty()
    {
        return Some(OAuthClient {
            client_id: id,
            client_secret: secret,
            project_id: std::env::var(format!("{prefix}_PROJECT_ID"))
                .ok()
                .filter(|p| !p.is_empty()),
        });
    }
    if from_config.is_some() {
        return from_config;
    }
    for_binary(capability).and_then(|a| a.baked_client())
}

/// Outcome of [`run_managed_login`], for callers that render UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedLoginOutcome {
    /// Sign-in completed and the post-login probe reports authed.
    SignedIn,
    /// The vendor login command exited non-zero (user closed the
    /// browser, denied consent, …). Stderr tail included when captured.
    LoginFailed { detail: String },
}

/// How long the headless path waits for the user to finish the browser
/// consent before killing the vendor's login process. Mirrors the PKCE
/// flow's timeout.
const BROWSER_LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Execute a [`ManagedLogin`] plan: provision the OAuth client file
/// (never overwriting one the user already has), run the vendor's
/// sign-in command, and re-probe.
///
/// `interactive` controls stdio: `true` inherits the terminal (CLI
/// path). `false` is the desktop path: output is captured, and because
/// vendor CLIs without a TTY tend to *print* the authorization URL
/// instead of opening a browser (gws does), stdout is scanned and the
/// first `https://` URL is opened with the system browser. The child is
/// killed after [`BROWSER_LOGIN_TIMEOUT`] so a UI caller can never hang
/// forever on an abandoned consent screen.
pub fn run_managed_login(
    adapter: &dyn CliAuthAdapter,
    plan: &ManagedLogin,
    interactive: bool,
) -> std::io::Result<ManagedLoginOutcome> {
    use std::process::{Command, Stdio};

    if !plan.client_config_path.exists() || plan.overwrite_existing {
        if let Some(parent) = plan.client_config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&plan.client_config_path, &plan.client_config)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &plan.client_config_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
    }

    let binary = plan
        .login_argv
        .first()
        .map(String::as_str)
        .unwrap_or_else(|| adapter.binary());
    let resolved =
        crate::install::resolve_binary(binary).unwrap_or_else(|| std::path::PathBuf::from(binary));

    let mut cmd = Command::new(resolved);
    cmd.args(&plan.login_argv[1..]);
    apply_spawn_env(&mut cmd, adapter);
    let detail = if interactive {
        let status = cmd.status()?;
        if status.success() {
            String::new()
        } else {
            format!("`{}` exited with {status}", plan.login_argv.join(" "))
        }
    } else {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;

        // Watch BOTH streams for the authorization URL — gws prints it
        // to stderr ("Open this URL in your browser…"), other CLIs use
        // stdout. First URL seen on either stream wins.
        let opened = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stdout_handle = child
            .stdout
            .take()
            .map(|s| spawn_url_watching_reader(s, opened.clone()));
        let stderr_handle = child
            .stderr
            .take()
            .map(|s| spawn_url_watching_reader(s, opened));

        let deadline = std::time::Instant::now() + BROWSER_LOGIN_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        let stdout = stdout_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stderr = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();

        match status {
            None => format!(
                "`{}` timed out after {}s waiting for the browser sign-in to finish",
                plan.login_argv.join(" "),
                BROWSER_LOGIN_TIMEOUT.as_secs()
            ),
            Some(s) if s.success() => String::new(),
            Some(s) => {
                let tail: String = format!("{stderr}\n{stdout}")
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .rev()
                    .take(5)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("`{}` exited with {s}: {tail}", plan.login_argv.join(" "))
            }
        }
    };

    // The login attempt (whatever its outcome) invalidates any cached
    // probe so callers see fresh state immediately.
    invalidate_check(adapter.binary());

    if detail.is_empty() && matches!(adapter.check(), AuthState::Ok) {
        Ok(ManagedLoginOutcome::SignedIn)
    } else if detail.is_empty() {
        Ok(ManagedLoginOutcome::LoginFailed {
            detail: "sign-in command succeeded but the auth probe still reports signed out"
                .to_string(),
        })
    } else {
        Ok(ManagedLoginOutcome::LoginFailed { detail })
    }
}

/// First `https://` token on a line — the authorization URL a vendor
/// CLI prints when it can't (or won't) open the browser itself.
fn extract_https_url(line: &str) -> Option<&str> {
    line.split_whitespace().find(|t| t.starts_with("https://"))
}

/// Drain one child stream line by line, opening the first `https://`
/// URL seen (across all readers sharing `opened`) in the system
/// browser. Returns the collected output for error reporting.
fn spawn_url_watching_reader<R: std::io::Read + Send + 'static>(
    stream: R,
    opened: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<String> {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::Ordering;

    std::thread::spawn(move || {
        let mut collected = String::new();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if let Some(url) = extract_https_url(&line)
                && opened
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                && webbrowser::open(url).is_err()
            {
                // Open failed — let another sighting retry.
                opened.store(false, Ordering::SeqCst);
            }
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    })
}

static ADAPTERS: OnceLock<Vec<Box<dyn CliAuthAdapter>>> = OnceLock::new();

fn registry() -> &'static [Box<dyn CliAuthAdapter>] {
    ADAPTERS.get_or_init(|| {
        let v: Vec<Box<dyn CliAuthAdapter>> = vec![Box::new(gws::GwsAdapter)];
        v
    })
}

/// Look up an adapter by CLI binary name.
pub fn for_binary(name: &str) -> Option<&'static dyn CliAuthAdapter> {
    registry()
        .iter()
        .find(|a| a.binary() == name)
        .map(|a| a.as_ref())
}

/// TTL for [`check_known`]'s probe cache. Auth state moves slowly;
/// probes spawn a vendor process whose credential access can trigger an
/// OS keychain prompt (macOS ACLs re-ask per process for ad-hoc-signed
/// binaries) — so UI polling must not multiply spawns.
const CHECK_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

type CheckCache = std::collections::HashMap<String, (std::time::Instant, AuthState)>;
static CHECK_CACHE: OnceLock<std::sync::Mutex<CheckCache>> = OnceLock::new();

/// Convenience: probe a binary; returns `AuthState::Ok` when no
/// adapter is registered for that name (unknown CLI is best-effort).
/// Results are cached for [`CHECK_CACHE_TTL`]; sign-in flows call
/// [`invalidate_check`] so the UI flips immediately after a login.
pub fn check_known(name: &str) -> AuthState {
    let Some(adapter) = for_binary(name) else {
        return AuthState::Ok;
    };
    let cache = CHECK_CACHE.get_or_init(Default::default);
    if let Ok(guard) = cache.lock()
        && let Some((at, state)) = guard.get(name)
        && at.elapsed() < CHECK_CACHE_TTL
    {
        return state.clone();
    }
    let state = adapter.check();
    if let Ok(mut guard) = cache.lock() {
        guard.insert(name.to_string(), (std::time::Instant::now(), state.clone()));
    }
    state
}

/// Drop the cached probe result for one binary (after a login attempt).
pub fn invalidate_check(name: &str) {
    if let Some(cache) = CHECK_CACHE.get()
        && let Ok(mut guard) = cache.lock()
    {
        guard.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_https_url_finds_indented_auth_url() {
        // Exactly what `gws auth login` prints without a TTY.
        let line = "  https://accounts.google.com/o/oauth2/auth?scope=x&redirect_uri=http://localhost:60396";
        assert_eq!(
            extract_https_url(line),
            Some(
                "https://accounts.google.com/o/oauth2/auth?scope=x&redirect_uri=http://localhost:60396"
            )
        );
    }

    #[test]
    fn extract_https_url_ignores_prose_and_http() {
        assert_eq!(extract_https_url("Open this URL in your browser:"), None);
        assert_eq!(
            extract_https_url("listening on http://localhost:8080"),
            None
        );
        assert_eq!(extract_https_url(""), None);
    }
}

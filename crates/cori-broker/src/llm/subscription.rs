//! Subscription-backed LLM backends: the vendor agent CLIs already
//! installed and signed in on the user's own machine.
//!
//! A user with Claude Pro/Max, ChatGPT Plus/Pro, Cursor, or a Google AI
//! plan has already paid for model access. When Cori runs on that same
//! machine it can spend that subscription instead of a metered API key,
//! by shelling out to the vendor's own documented non-interactive mode:
//!
//! | Backend | Binary | One-shot invocation |
//! |---|---|---|
//! | Claude Code | `claude` | `claude -p --output-format json` |
//! | Codex | `codex` | `codex exec --json --sandbox read-only -` |
//! | Cursor | `cursor-agent` | `cursor-agent -p --output-format json` |
//! | Gemini | `gemini` | `gemini --output-format json` |
//!
//! Three properties matter and are enforced here, not left to the
//! vendor's defaults:
//!
//! 1. **These are agents, not completion endpoints.** Every one of them
//!    can read files and run shell commands. An `llm` step in Cori is a
//!    pure text transform, and the capability model (`tools_required`)
//!    knows nothing about tools an agent CLI might reach for on its own.
//!    So each child runs with its vendor's most restrictive documented
//!    flag *and* with its working directory set to an empty scratch dir
//!    ([`scratch_dir`]) — never the workflow folder, never the user's
//!    cwd. A step that wants filesystem or shell access must declare a
//!    `cli` step for it.
//! 2. **The subscription must actually be what pays.** Every one of
//!    these CLIs silently switches to metered API billing when it finds
//!    a vendor API key in the environment. Cori strips those variables
//!    from the child ([`SUPPRESSED_API_KEY_VARS`]) so "use my
//!    subscription" cannot quietly bill an API key instead.
//! 3. **The prompt never enters argv.** Prompts carry workflow data and
//!    argv is world-readable via `ps`. Every backend takes its prompt on
//!    stdin.
//!
//! Sign-in state is probed cheaply — a vendor credential file, or a
//! `status` subcommand where the vendor offers one. Probes are
//! best-effort by design: a false "ready" surfaces as a normal step
//! error from the spawn itself, which is a better failure than blocking
//! a run on a heuristic. Results are cached ([`CHECK_CACHE_TTL`])
//! because the Console polls this for its settings UI.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value as JsonValue;

use super::catalog::ModelTier;
use super::providers::{LlmProvider, LlmRequest, LlmResponse};
use crate::{BrokerError, Result, TokenUsage};

/// Vendor API-key variables stripped from every subscription child, so
/// the subscription is what pays. Each of these makes at least one of
/// the agent CLIs fall back to metered API billing.
pub const SUPPRESSED_API_KEY_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "CURSOR_API_KEY",
];

/// How long a child may run before Cori kills it. Generous: these CLIs
/// start a Node/Rust runtime and may retry internally.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// TTL for the sign-in probe cache. `cursor-agent status` spawns a
/// process; the Console's settings tab must not multiply spawns.
const CHECK_CACHE_TTL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Backend registry
// ---------------------------------------------------------------------------

/// One subscription backend Cori knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendSpec {
    /// Stable id used in config and the trace (`claude`, `codex`, …).
    pub id: &'static str,
    /// Executable name resolved on PATH / `~/.cori/bin`.
    pub binary: &'static str,
    pub display_name: &'static str,
    /// Which subscription pays for it — shown in the Console.
    pub subscription_name: &'static str,
    /// The command that signs the user in, for remediation hints.
    pub login_hint: &'static str,
    /// Default tier → model mapping. `None` means "don't pass a model
    /// flag; use whatever the CLI is configured to use". Overridable
    /// per-backend in `~/.cori/config.toml`.
    pub fast: Option<&'static str>,
    pub balanced: Option<&'static str>,
    pub deep: Option<&'static str>,
    /// Model names worth offering in the Console's per-tier pickers.
    /// Suggestions only — any string is accepted, because vendors ship
    /// models faster than Cori ships releases.
    pub model_suggestions: &'static [&'static str],
}

impl BackendSpec {
    /// Default model for a tier, before config overrides.
    pub fn default_model_for(&self, tier: ModelTier) -> Option<&'static str> {
        match tier {
            ModelTier::Fast => self.fast,
            ModelTier::Balanced => self.balanced,
            ModelTier::Deep => self.deep,
        }
    }

    /// Whether this backend natively serves an API provider's models.
    /// Used so `model: "claude-3-5-sonnet"` prefers the Claude
    /// subscription over the Cursor one when both are signed in.
    pub fn serves_api_provider(&self, provider: &str) -> bool {
        matches!(
            (self.id, provider),
            ("claude", "anthropic") | ("codex", "openai") | ("gemini-cli", "gemini")
        )
    }
}

/// Every subscription backend, in built-in preference order.
///
/// This order is only the default: the user's `[llm].priority` list
/// ranks these against the API providers however they like. Claude Code
/// and Codex lead because their non-interactive modes are the most
/// established; Cursor and Gemini follow.
///
/// **Model names are the maintenance point of this file.** Vendors
/// rename models often. Users can override any cell via
/// `cori config set llm.models.<id>.<tier> <model>` without waiting for
/// a Cori release (see [`super::policy::LlmConfig`]).
pub const BACKENDS: &[BackendSpec] = &[
    BackendSpec {
        id: "claude",
        binary: "claude",
        display_name: "Claude Code",
        subscription_name: "Claude Pro or Max",
        login_hint: "run `claude` once in a terminal and sign in",
        // Claude Code resolves these aliases to current models itself,
        // which is exactly the indirection we want here.
        fast: Some("haiku"),
        balanced: Some("sonnet"),
        deep: Some("opus"),
        model_suggestions: &["haiku", "sonnet", "opus"],
    },
    BackendSpec {
        id: "codex",
        binary: "codex",
        display_name: "Codex CLI",
        subscription_name: "ChatGPT Plus, Pro, or Business",
        login_hint: "run `codex login` and choose \"Sign in with ChatGPT\"",
        fast: Some("gpt-5-mini"),
        balanced: Some("gpt-5"),
        deep: Some("gpt-5"),
        model_suggestions: &["gpt-5-mini", "gpt-5", "gpt-5-codex", "o3"],
    },
    BackendSpec {
        id: "cursor",
        binary: "cursor-agent",
        display_name: "Cursor CLI",
        subscription_name: "Cursor Pro or Business",
        login_hint: "run `cursor-agent login`",
        fast: Some("sonnet-4"),
        balanced: Some("gpt-5"),
        deep: Some("sonnet-4-thinking"),
        model_suggestions: &["sonnet-4", "sonnet-4-thinking", "gpt-5", "opus-4.1"],
    },
    BackendSpec {
        id: "gemini-cli",
        binary: "gemini",
        display_name: "Gemini CLI",
        subscription_name: "Google AI Pro/Ultra or Gemini Code Assist",
        login_hint: "run `gemini` once and sign in with your Google account",
        fast: Some("gemini-2.5-flash"),
        balanced: Some("gemini-2.5-pro"),
        deep: Some("gemini-2.5-pro"),
        model_suggestions: &["gemini-2.5-flash", "gemini-2.5-pro"],
    },
];

pub fn spec_for(id: &str) -> Option<&'static BackendSpec> {
    BACKENDS.iter().find(|b| b.id == id)
}

// ---------------------------------------------------------------------------
// Sign-in probing
// ---------------------------------------------------------------------------

/// Whether a backend can be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendState {
    /// Installed and signed in with a subscription.
    Ready,
    /// Binary is not installed.
    NotInstalled,
    /// Installed, but not signed in with a subscription. `hint` is the
    /// one command that fixes it.
    SignedOut { hint: String },
}

impl BackendState {
    pub fn is_ready(&self) -> bool {
        matches!(self, BackendState::Ready)
    }
}

type CheckCache = HashMap<&'static str, (Instant, BackendState)>;
static CHECK_CACHE: OnceLock<Mutex<CheckCache>> = OnceLock::new();

/// Probe one backend, memoised for [`CHECK_CACHE_TTL`].
pub fn check(spec: &BackendSpec) -> BackendState {
    let cache = CHECK_CACHE.get_or_init(Default::default);
    if let Ok(guard) = cache.lock()
        && let Some((at, state)) = guard.get(spec.id)
        && at.elapsed() < CHECK_CACHE_TTL
    {
        return state.clone();
    }
    let state = probe(spec);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(spec.id, (Instant::now(), state.clone()));
    }
    state
}

/// Drop every cached probe (after a sign-in, or when the user asks the
/// Console to re-check).
pub fn invalidate_checks() {
    if let Some(cache) = CHECK_CACHE.get()
        && let Ok(mut guard) = cache.lock()
    {
        guard.clear();
    }
}

fn probe(spec: &BackendSpec) -> BackendState {
    if crate::install::resolve_binary(spec.binary).is_none() {
        return BackendState::NotInstalled;
    }
    let signed_in = match spec.id {
        "claude" => claude_signed_in(),
        "codex" => codex_signed_in(),
        "cursor" => cursor_signed_in(),
        "gemini-cli" => gemini_signed_in(),
        _ => true,
    };
    if signed_in {
        BackendState::Ready
    } else {
        BackendState::SignedOut {
            hint: spec.login_hint.to_string(),
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Claude Code stores its OAuth tokens in the OS keychain on macOS and
/// in `~/.claude/.credentials.json` elsewhere, so neither path alone is
/// conclusive. `~/.claude.json` gains an `oauthAccount` entry on
/// sign-in on every platform, which covers the keychain case.
fn claude_signed_in() -> bool {
    let Some(home) = home_dir() else {
        return false;
    };
    if home.join(".claude/.credentials.json").is_file() {
        return true;
    }
    std::fs::read_to_string(home.join(".claude.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<JsonValue>(&s).ok())
        .map(|v| v.get("oauthAccount").is_some())
        .unwrap_or(false)
}

/// Codex records how it authenticated in `~/.codex/auth.json`:
/// `auth_mode: "chatgpt"` is the subscription, `"apikey"` is metered
/// billing. Only the former counts as a subscription backend — if the
/// user authed Codex with an API key, Cori's own API path should serve
/// that request instead, with proper cost accounting.
fn codex_signed_in() -> bool {
    let path = match std::env::var_os("CODEX_HOME") {
        Some(dir) => PathBuf::from(dir).join("auth.json"),
        None => match home_dir() {
            Some(home) => home.join(".codex/auth.json"),
            None => return false,
        },
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<JsonValue>(&raw) else {
        return false;
    };
    if v.get("auth_mode").and_then(JsonValue::as_str) == Some("chatgpt") {
        return true;
    }
    // Older Codex builds wrote no `auth_mode`; OAuth tokens present with
    // no API key is the same state.
    v.get("tokens").and_then(JsonValue::as_object).is_some()
        && v.get("OPENAI_API_KEY")
            .map(JsonValue::is_null)
            .unwrap_or(true)
}

/// Ceiling for the one sign-in probe that has to spawn a process.
/// `cursor-agent status` reaches the network, so it must never be able
/// to stall a run: past this we report "not signed in" and move on.
const CURSOR_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cursor keeps no readable credential file, but ships `cursor-agent
/// status`. That command exits 0 whether or not the user is signed in
/// (same trap as `gws auth status`), so the stdout text is the truth.
///
/// This is the only probe that costs a process spawn, which is why
/// callers opt into probing via
/// [`crate::capabilities::LlmProbe`] rather than paying for it on every
/// run.
fn cursor_signed_in() -> bool {
    let Some(bin) = crate::install::resolve_binary("cursor-agent") else {
        return false;
    };
    let mut cmd = Command::new(bin);
    cmd.arg("status")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_console_window(&mut cmd);
    let Ok(child) = cmd.spawn() else {
        return false;
    };
    let Some(out) = output_before(child, CURSOR_PROBE_TIMEOUT) else {
        return false;
    };
    let text = strip_ansi(&format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
    .to_ascii_lowercase();
    if text.contains("not logged in") || text.contains("not authenticated") {
        return false;
    }
    text.contains("logged in") || text.contains("authenticated")
}

/// Collect a child's output, killing it past `timeout`. `None` on
/// timeout or I/O failure — probe callers treat both as "can't tell".
fn output_before(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::Output> {
    use std::io::Read;

    let drain = |pipe: Option<_>| -> Option<std::thread::JoinHandle<Vec<u8>>> {
        pipe.map(|mut s: Box<dyn std::io::Read + Send>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            })
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    Some(std::process::Output {
        status: status?,
        stdout: stdout.and_then(|h| h.join().ok()).unwrap_or_default(),
        stderr: stderr.and_then(|h| h.join().ok()).unwrap_or_default(),
    })
}

fn gemini_signed_in() -> bool {
    home_dir()
        .map(|h| h.join(".gemini/oauth_creds.json").is_file())
        .unwrap_or(false)
}

/// Strip ANSI escape sequences — the agent CLIs draw spinners and
/// cursor moves even when their output is piped.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ params intermediates final. The final byte is
            // the first in @..~ *after* the introducer — which is itself
            // in that range, so it has to be consumed first.
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … terminated by BEL or ESC \.
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\x07' {
                        break;
                    }
                    if next == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-character sequences (ESC M, ESC 7, …).
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The provider
// ---------------------------------------------------------------------------

/// An [`LlmProvider`] that spends a local subscription by driving a
/// vendor agent CLI in its one-shot, non-interactive mode.
pub struct SubscriptionProvider {
    spec: &'static BackendSpec,
    /// Model to pass to the CLI, or `None` to use its configured default.
    model: Option<String>,
}

impl SubscriptionProvider {
    pub fn new(spec: &'static BackendSpec, model: Option<String>) -> Self {
        Self { spec, model }
    }

    /// argv after the binary. The prompt is never included — it goes on
    /// stdin (see module docs).
    fn argv(&self) -> Vec<String> {
        let model = self.model.as_deref();
        let mut args: Vec<String> = match self.spec.id {
            // `-p` is print mode; JSON gives us a parseable envelope
            // instead of the TTY rendering.
            "claude" => vec!["-p".into(), "--output-format".into(), "json".into()],
            // `exec` is Codex's non-interactive mode. `--sandbox
            // read-only` is its documented no-writes, no-network setting;
            // `-` reads the prompt from stdin. `--skip-git-repo-check`
            // keeps it from refusing to start in the scratch dir.
            "codex" => vec![
                "exec".into(),
                "--json".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--skip-git-repo-check".into(),
            ],
            // No `--force`: without it cursor-agent denies shell commands
            // rather than running them.
            "cursor" => vec!["-p".into(), "--output-format".into(), "json".into()],
            "gemini-cli" => vec!["--output-format".into(), "json".into()],
            _ => Vec::new(),
        };
        if let Some(model) = model {
            match self.spec.id {
                "claude" | "cursor" => {
                    args.push("--model".into());
                    args.push(model.to_string());
                }
                "codex" | "gemini-cli" => {
                    args.push("-m".into());
                    args.push(model.to_string());
                }
                _ => {}
            }
        }
        // Codex takes its prompt positionally; `-` means stdin.
        if self.spec.id == "codex" {
            args.push("-".into());
        }
        args
    }
}

impl LlmProvider for SubscriptionProvider {
    fn name(&self) -> &'static str {
        self.spec.id
    }

    fn complete(&self, req: &LlmRequest<'_>) -> Result<LlmResponse> {
        let bin = crate::install::resolve_binary(self.spec.binary).ok_or_else(|| {
            BrokerError::LlmProviderError {
                provider: self.spec.id,
                status: 0,
                body: format!(
                    "`{}` is no longer on PATH — {}",
                    self.spec.binary, self.spec.login_hint
                ),
            }
        })?;

        // The full instruction, since these CLIs have no separate system
        // channel in one-shot mode.
        let prompt = compose_prompt(req);
        let scratch = scratch_dir(self.spec.id)?;

        let mut cmd = Command::new(bin);
        cmd.args(self.argv())
            .current_dir(&scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in SUPPRESSED_API_KEY_VARS {
            cmd.env_remove(var);
        }
        // Agent CLIs colour their output when they think a terminal is
        // attached, and some check these rather than isatty.
        cmd.env("NO_COLOR", "1");
        cmd.env("TERM", "dumb");
        crate::process::hide_console_window(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| BrokerError::LlmProviderError {
            provider: self.spec.id,
            status: 0,
            body: format!("spawning `{}`: {e}", self.spec.binary),
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            // A closed pipe means the child died early; its stderr is the
            // real diagnostic, so don't fail on the write itself.
            let _ = stdin.write_all(prompt.as_bytes());
            drop(stdin);
        }

        let out = wait_with_timeout(child, self.spec, CALL_TIMEOUT)?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();

        if !out.status.success() {
            return Err(BrokerError::LlmProviderError {
                provider: self.spec.id,
                status: out.status.code().unwrap_or(-1) as u16,
                body: subscription_failure_body(self.spec, &stdout, &stderr),
            });
        }

        let (text, usage) = parse_output(self.spec, &stdout);
        if text.trim().is_empty() {
            return Err(BrokerError::LlmProviderError {
                provider: self.spec.id,
                status: 0,
                body: format!(
                    "{} returned no assistant text.\nstdout: {}\nstderr: {}",
                    self.spec.display_name,
                    truncate(&stdout, 2048),
                    truncate(&stderr, 2048)
                ),
            });
        }
        Ok(LlmResponse { text, usage })
    }
}

/// Fold the system instruction and schema into one prompt: one-shot mode
/// has no separate system channel.
fn compose_prompt(req: &LlmRequest<'_>) -> String {
    let mut out = String::new();
    out.push_str(super::providers::system_message(req.strict_retry));
    if let Some(schema) = req.output_schema {
        out.push_str(
            "\n\nRespond with a single JSON object matching this JSON Schema exactly. \
             Return ONLY the JSON — no markdown fences, no prose, no commentary:\n",
        );
        out.push_str(&serde_json::to_string(schema).unwrap_or_default());
    }
    out.push_str("\n\n---\n\n");
    out.push_str(req.prompt);
    out
}

/// A private, empty working directory for the child. Agent CLIs read the
/// directory they start in; pointing them at an empty one keeps an `llm`
/// step from picking up workflow files or the user's cwd as context.
fn scratch_dir(id: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("cori-llm-{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| BrokerError::LlmProviderError {
        provider: "subscription",
        status: 0,
        body: format!("creating scratch dir `{}`: {e}", dir.display()),
    })?;
    Ok(dir)
}

/// `Child::wait_with_output` has no timeout. Poll, then kill.
fn wait_with_timeout(
    mut child: std::process::Child,
    spec: &BackendSpec,
    timeout: Duration,
) -> Result<std::process::Output> {
    use std::io::Read;

    // Drain both pipes on threads so a chatty child can't fill a pipe
    // buffer and deadlock against our polling loop.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_handle = stdout_pipe.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = stderr_pipe.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                return Err(BrokerError::LlmProviderError {
                    provider: spec.id,
                    status: 0,
                    body: format!("waiting on `{}`: {e}", spec.binary),
                });
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let stdout = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    let Some(status) = status else {
        return Err(BrokerError::LlmProviderError {
            provider: spec.id,
            status: 0,
            body: format!(
                "{} did not answer within {}s",
                spec.display_name,
                timeout.as_secs()
            ),
        });
    };
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Turn a non-zero exit into something the user can act on. A signed-out
/// CLI is the overwhelmingly common cause, and its own message says so —
/// surface that plus the fix.
fn subscription_failure_body(spec: &BackendSpec, stdout: &str, stderr: &str) -> String {
    let combined = strip_ansi(&format!("{stderr}\n{stdout}"));
    let lower = combined.to_ascii_lowercase();
    let auth_ish = [
        "not logged in",
        "unauthorized",
        "authenticate",
        "sign in",
        "401",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    let tail: String = combined
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");

    if auth_ish {
        format!(
            "{} is not signed in — {}.\n{}",
            spec.display_name,
            spec.login_hint,
            truncate(&tail, 2048)
        )
    } else {
        format!("{} failed:\n{}", spec.display_name, truncate(&tail, 4096))
    }
}

// ---------------------------------------------------------------------------
// Output parsing
// ---------------------------------------------------------------------------

/// Extract assistant text and token usage from a backend's stdout.
///
/// Deliberately forgiving. These CLIs are on their own release cadence
/// and have each changed their JSON envelope at least once; the schema
/// is not a stable contract. So: try the documented shape, then a set of
/// widely-used key names, then fall back to the raw stdout. Returning
/// slightly-wrapped text that the schema retry can fix beats hard-failing
/// a run because a vendor renamed a field.
fn parse_output(spec: &BackendSpec, stdout: &str) -> (String, TokenUsage) {
    let cleaned = strip_ansi(stdout);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return (String::new(), TokenUsage::default());
    }

    // Codex emits JSONL events rather than one object.
    if spec.id == "codex" {
        if let Some(found) = parse_jsonl_events(trimmed) {
            return found;
        }
        return (trimmed.to_string(), TokenUsage::default());
    }

    if let Ok(v) = serde_json::from_str::<JsonValue>(trimmed) {
        let text = extract_text(&v).unwrap_or_default();
        let usage = extract_usage(&v);
        if !text.trim().is_empty() {
            return (text, usage);
        }
        // Valid JSON with no recognisable text field: hand back the raw
        // body so a schema-shaped response still has a chance to parse.
        return (trimmed.to_string(), usage);
    }

    // Not JSON at all (older builds, or `--output-format` unsupported).
    (trimmed.to_string(), TokenUsage::default())
}

/// Codex's `--json` stream: one JSON object per line. The last agent
/// message wins; token counts accumulate from usage events.
fn parse_jsonl_events(stdout: &str) -> Option<(String, TokenUsage)> {
    let mut text: Option<String> = None;
    let mut usage = TokenUsage::default();
    let mut saw_json = false;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<JsonValue>(line) else {
            continue;
        };
        saw_json = true;

        // Current shape: { "type": "item.completed",
        //                  "item": { "type": "agent_message", "text": … } }
        if let Some(item) = v.get("item")
            && item.get("type").and_then(JsonValue::as_str) == Some("agent_message")
            && let Some(t) = item.get("text").and_then(JsonValue::as_str)
        {
            text = Some(t.to_string());
        }
        // Older shape: { "msg": { "type": "agent_message", "message": … } }
        if let Some(msg) = v.get("msg")
            && msg.get("type").and_then(JsonValue::as_str) == Some("agent_message")
            && let Some(t) = msg
                .get("message")
                .or_else(|| msg.get("text"))
                .and_then(JsonValue::as_str)
        {
            text = Some(t.to_string());
        }
        let event_usage = extract_usage(&v);
        if event_usage.input_tokens > 0 || event_usage.output_tokens > 0 {
            // Usage events report cumulative totals, so take the last.
            usage = event_usage;
        }
    }

    if !saw_json {
        return None;
    }
    text.map(|t| (t, usage))
}

/// Pull assistant text out of a one-object envelope, trying the field
/// names these CLIs actually use.
fn extract_text(v: &JsonValue) -> Option<String> {
    // Claude Code and Cursor: { "type": "result", "result": "…" }
    // Gemini: { "response": "…" }
    for key in ["result", "response", "text", "content", "message", "output"] {
        if let Some(s) = v.get(key).and_then(JsonValue::as_str)
            && !s.trim().is_empty()
        {
            return Some(s.to_string());
        }
    }
    // Some builds nest the payload one level down.
    for key in ["data", "result", "response"] {
        if let Some(inner) = v.get(key).filter(|i| i.is_object())
            && let Some(found) = extract_text(inner)
        {
            return Some(found);
        }
    }
    None
}

/// Token usage, across the several spellings in use.
fn extract_usage(v: &JsonValue) -> TokenUsage {
    let usage = ["usage", "tokenUsage", "token_usage", "stats", "tokens"]
        .iter()
        .find_map(|k| v.get(*k))
        .unwrap_or(v);

    let read = |keys: &[&str]| -> u64 {
        for key in keys {
            if let Some(n) = usage.get(*key).and_then(JsonValue::as_u64) {
                return n;
            }
        }
        // One level down (Gemini nests under `stats.tokens`).
        for (_, nested) in usage.as_object().into_iter().flatten() {
            for key in keys {
                if let Some(n) = nested.get(*key).and_then(JsonValue::as_u64) {
                    return n;
                }
            }
        }
        0
    };

    TokenUsage {
        input_tokens: read(&[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokenCount",
        ]),
        output_tokens: read(&[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "candidatesTokenCount",
        ]),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(truncated)", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(id: &str) -> &'static BackendSpec {
        spec_for(id).expect("registered backend")
    }

    #[test]
    fn prompt_never_enters_argv() {
        for backend in BACKENDS {
            let provider = SubscriptionProvider::new(backend, Some("some-model".into()));
            let argv = provider.argv();
            assert!(
                !argv.iter().any(|a| a.contains("PROMPT")),
                "{} put content in argv",
                backend.id
            );
            // Codex's trailing `-` is the stdin marker, not a prompt.
            assert!(argv.iter().all(|a| a.len() < 64));
        }
    }

    #[test]
    fn codex_reads_stdin_and_is_sandboxed() {
        let argv = SubscriptionProvider::new(spec("codex"), None).argv();
        assert_eq!(argv.first().map(String::as_str), Some("exec"));
        assert!(argv.iter().any(|a| a == "read-only"));
        assert_eq!(argv.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn cursor_never_gets_force() {
        let argv = SubscriptionProvider::new(spec("cursor"), Some("gpt-5".into())).argv();
        assert!(!argv.iter().any(|a| a == "-f" || a == "--force"));
        assert!(argv.iter().any(|a| a == "--model"));
    }

    #[test]
    fn omitting_a_model_omits_the_flag() {
        let argv = SubscriptionProvider::new(spec("claude"), None).argv();
        assert!(!argv.iter().any(|a| a == "--model"));
    }

    #[test]
    fn every_vendor_api_key_var_is_suppressed() {
        // The whole point of the subscription path: none of these may
        // reach the child, or a metered key silently pays instead.
        for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"] {
            assert!(SUPPRESSED_API_KEY_VARS.contains(&var));
        }
    }

    #[test]
    fn parses_claude_json_envelope() {
        let out = json!({
            "type": "result",
            "result": "{\"ok\":true}",
            "usage": { "input_tokens": 120, "output_tokens": 8 }
        })
        .to_string();
        let (text, usage) = parse_output(spec("claude"), &out);
        assert_eq!(text, "{\"ok\":true}");
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 8);
    }

    #[test]
    fn parses_gemini_response_key() {
        let out = json!({ "response": "hello", "stats": { "promptTokenCount": 3 } }).to_string();
        let (text, usage) = parse_output(spec("gemini-cli"), &out);
        assert_eq!(text, "hello");
        assert_eq!(usage.input_tokens, 3);
    }

    #[test]
    fn parses_codex_jsonl_taking_the_last_agent_message() {
        let out = [
            r#"{"type":"item.started","item":{"type":"reasoning"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"first"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"final"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":40,"output_tokens":5}}"#,
        ]
        .join("\n");
        let (text, usage) = parse_output(spec("codex"), &out);
        assert_eq!(text, "final");
        assert_eq!(usage.input_tokens, 40);
    }

    #[test]
    fn parses_legacy_codex_msg_shape() {
        let out = r#"{"msg":{"type":"agent_message","message":"legacy"}}"#;
        let (text, _) = parse_output(spec("codex"), out);
        assert_eq!(text, "legacy");
    }

    #[test]
    fn non_json_output_falls_back_to_raw_text() {
        let (text, _) = parse_output(spec("claude"), "just plain text\n");
        assert_eq!(text, "just plain text");
    }

    #[test]
    fn strips_ansi_spinner_noise() {
        // Exactly what `cursor-agent status` writes to a pipe.
        assert_eq!(strip_ansi("\x1b[2K\x1b[1ALogged in\x1b[0m"), "Logged in");
        assert_eq!(
            strip_ansi("\x1b[2K\x1b[1A\x1b[GNot logged in"),
            "Not logged in"
        );
        assert_eq!(strip_ansi("\x1b]0;title\x07ready"), "ready");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn exact_model_ownership_maps_to_the_right_subscription() {
        assert!(spec("claude").serves_api_provider("anthropic"));
        assert!(!spec("claude").serves_api_provider("openai"));
        assert!(spec("codex").serves_api_provider("openai"));
        assert!(spec("gemini-cli").serves_api_provider("gemini"));
        // Cursor is a multi-vendor router; it claims no provider natively.
        assert!(!spec("cursor").serves_api_provider("openai"));
    }

    #[test]
    fn signed_out_failures_surface_the_fix() {
        let body = subscription_failure_body(spec("codex"), "", "Error: not logged in");
        assert!(body.contains("codex login"));
    }

    #[test]
    fn every_backend_covers_every_tier() {
        for backend in BACKENDS {
            for tier in [ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep] {
                assert!(
                    backend.default_model_for(tier).is_some(),
                    "{} has no model for {tier}",
                    backend.id
                );
            }
        }
    }
}

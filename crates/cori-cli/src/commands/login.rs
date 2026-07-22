//! `cori login <capability>` — sign in to an MCP server, CLI, or LLM
//! provider.
//!
//! Dispatch logic (in order):
//!
//! 1. If `<capability>` matches a known [`cori_broker::cli_auth`]
//!    adapter (currently: `gws`), run the **managed login**: install
//!    the binary if missing (built-in [`cori_broker::install`]
//!    registry), provision the Cori-owned OAuth client into the
//!    vendor's config if the adapter supports it, then delegate to the
//!    vendor's own `<cli> auth login` (which opens the browser and owns
//!    token refresh). Without a provisioned client we fall back to
//!    printing the manual hint — Cori never fakes the vendor's flow.
//! 2. If `<capability>` is an LLM provider (`openai`, `anthropic`,
//!    `gemini`), prompt for an API key and store it in
//!    `~/.cori/config.toml` (env vars still take precedence at run
//!    time). The migration plan explicitly leaves LLM credentials
//!    out of the OAuth path; we surface a uniform prompt for UX.
//! 3. Otherwise, treat `<capability>` as an MCP server id. Look up its
//!    `oauth` block in `~/.cori/mcp-servers.json` and run the
//!    [`pkce`][cori_broker::oauth::pkce] flow. The resulting [`Token`]
//!    is stored in the OS keychain (or the encrypted-file fallback)
//!    keyed by `(server_id, Owner::User(<os user>))`.
//!
//! Idempotent: if a still-valid token already exists for the
//! requesting user, the command is a no-op and prints `(already signed
//! in)`.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use cori_broker::capabilities::discover_mcp_for_login;
use cori_broker::cli_auth;
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::install;
use cori_broker::llm::LlmCredentials;
use cori_broker::oauth::{self, McpOAuthConfig, Owner, TokenKey, default_store, pkce};
use cori_protocol::WorkerIdentity;

use cori_run::config::Config;
use cori_run::paths;

const PKCE_TIMEOUT: Duration = Duration::from_secs(300);

pub fn login(capability: &str) -> Result<()> {
    // 1. Known-CLI adapter? Run the managed login flow.
    if let Some(adapter) = cli_auth::for_binary(capability) {
        return login_managed_cli(capability, adapter);
    }

    // 2. LLM provider? Prompt for an API key and write config.
    match capability {
        "openai" => return login_llm_provider("openai"),
        "anthropic" => return login_llm_provider("anthropic"),
        "gemini" => return login_llm_provider("gemini"),
        _ => {}
    }

    // 3. MCP server with OAuth metadata.
    let home = paths::home()?;
    let servers = discover_mcp_for_login(&home);
    let server_cfg = servers.get(capability).ok_or_else(|| {
        anyhow::anyhow!(
            "no capability `{capability}` is known. \
             For MCP servers, add an entry to ~/.cori/mcp-servers.json. \
             For CLIs, install the binary; for LLM providers, pass `openai`/`anthropic`/`gemini`."
        )
    })?;
    let oauth_cfg = server_cfg.oauth.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "MCP server `{capability}` has no `oauth` block in ~/.cori/mcp-servers.json — \
             add {{ authorization_endpoint, token_endpoint, client_id, scopes }} to enable `cori login`."
        )
    })?;

    login_mcp_oauth(capability, oauth_cfg)
}

/// Managed CLI login: install if missing → provision the Cori-owned
/// OAuth client → delegate to the vendor's own sign-in → re-probe.
fn login_managed_cli(capability: &str, adapter: &dyn cli_auth::CliAuthAdapter) -> Result<()> {
    // Already signed in? Done.
    if matches!(adapter.check(), cli_auth::AuthState::Ok) {
        println!("✓ {} is already signed in.", adapter.display_name());
        notify_open_workflows(capability);
        return Ok(());
    }

    // Binary missing? Install from the built-in registry.
    if install::resolve_binary(adapter.binary()).is_none() {
        if let Some(spec) = install::spec_for(capability) {
            println!(
                "`{}` is not installed — installing {}…",
                adapter.binary(),
                spec.display_name
            );
            let path = install::install(capability)
                .with_context(|| format!("installing `{capability}`"))?;
            println!("✓ Installed {} to {}", spec.display_name, path.display());
        } else {
            bail!(
                "`{}` is not installed and Cori has no install recipe for it — install it manually, then re-run `cori login {capability}`",
                adapter.binary()
            );
        }
    }

    // Managed provisioning: resolve the OAuth client Cori owns.
    let cfg = Config::load()?;
    let from_config = match (
        config_string(&cfg, &format!("capability.{capability}.oauth_client_id")),
        config_string(
            &cfg,
            &format!("capability.{capability}.oauth_client_secret"),
        ),
    ) {
        (Some(client_id), Some(client_secret)) => Some(cli_auth::OAuthClient {
            client_id,
            client_secret,
        }),
        _ => None,
    };
    let services: Vec<String> = config_string(&cfg, &format!("capability.{capability}.services"))
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();

    let plan = cli_auth::resolve_client(capability, from_config)
        .and_then(|client| adapter.managed_login(&client, &services));

    let Some(plan) = plan else {
        // No Cori-owned client available (dev build, unconfigured) or
        // the adapter has no managed flow — fall back to the manual
        // path with an honest explanation.
        println!("No Cori-provisioned OAuth client is available for `{capability}` in this build.");
        println!(
            "Either set one (`cori config set capability.{capability}.oauth_client_id …` and \
             `…oauth_client_secret …`, then re-run `cori login {capability}`), or follow the \
             vendor's own setup: `{} auth setup` then `{} auth login`.",
            adapter.binary(),
            adapter.binary()
        );
        bail!("CLI auth required");
    };

    if plan.client_config_path.exists() {
        println!(
            "Using the existing OAuth client config at {} (Cori never overwrites it).",
            plan.client_config_path.display()
        );
    } else {
        println!(
            "Provisioning the Cori OAuth client to {} — no GCP project needed.",
            plan.client_config_path.display()
        );
    }
    println!(
        "Opening your browser to sign in to {}…",
        adapter.display_name()
    );

    match cli_auth::run_managed_login(adapter, &plan, true)? {
        cli_auth::ManagedLoginOutcome::SignedIn => {
            println!("✓ Signed in to {}.", adapter.display_name());
            notify_open_workflows(capability);
            Ok(())
        }
        cli_auth::ManagedLoginOutcome::LoginFailed { detail } => {
            bail!(
                "{} sign-in did not complete: {detail}",
                adapter.display_name()
            );
        }
    }
}

fn config_string(cfg: &Config, key: &str) -> Option<String> {
    cfg.get(key).and_then(|v| match v {
        toml::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

fn login_mcp_oauth(server_id: &str, oauth_cfg: &McpOAuthConfig) -> Result<()> {
    let identity = OsUser
        .resolve()
        .context("resolving OS user for OAuth token ownership")?;
    let owner = match identity {
        WorkerIdentity::Person { user_id } => Owner::User(user_id),
        WorkerIdentity::Service { pool } => Owner::Service(pool),
    };

    let credentials_dir = paths::credentials_dir()?;
    let store = default_store(credentials_dir);
    let key = TokenKey::new(server_id.to_string(), owner.clone());

    // Idempotency: if a non-expiring token is already present, we're done.
    if let Ok(Some(existing)) = store.get(&key)
        && !existing.is_expiring(oauth::REFRESH_MARGIN_SECS)
    {
        println!("✓ `{server_id}` is already signed in (token still valid).");
        return Ok(());
    }

    let req = pkce::PkceRequest {
        authorization_endpoint: oauth_cfg.authorization_endpoint.clone(),
        token_endpoint: oauth_cfg.token_endpoint.clone(),
        client_id: oauth_cfg.client_id.clone(),
        scopes: oauth_cfg.scopes.clone(),
        display_name: server_id.to_string(),
        timeout: PKCE_TIMEOUT,
    };

    let token = pkce::run(&req).with_context(|| format!("PKCE sign-in to `{server_id}` failed"))?;

    store
        .put(&key, &token)
        .with_context(|| format!("storing token for `{server_id}`"))?;

    println!("✓ Signed in. Token stored for `{server_id}`.");
    notify_open_workflows(server_id);
    Ok(())
}

fn login_llm_provider(provider: &'static str) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};

    let existing = LlmCredentials::from_env();
    if existing.key_for(provider).is_some() {
        println!("✓ {provider} is already configured via environment variable (overrides config).");
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        bail!(
            "`cori login {provider}` is interactive — set the API key with `cori config set llm.{provider}.api_key <key>` instead"
        );
    }

    eprint!("Paste your {provider} API key (input hidden in your shell history): ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading API key from stdin")?;
    let key = line.trim().to_string();
    if key.is_empty() {
        bail!("no API key entered");
    }

    crate::commands::config::set(&format!("llm.{provider}.api_key"), &key)?;
    println!("✓ Stored {provider} API key in ~/.cori/config.toml.");
    println!("  (To override per shell, export the matching env var instead.)");
    notify_open_workflows(provider);
    Ok(())
}

/// Phase 6: after a successful sign-in, signal every open workflow on
/// the requesting user's task queue with `reauth_completed`. Workflows
/// suspended in a `NeedsReauth` wait for this `server_id` will resume.
///
/// Best-effort: every failure is logged at debug level and swallowed
/// so that login itself stays successful. Skipped entirely when the
/// Temporal endpoint is unreachable (no daemon running == no workflows
/// to notify).
fn notify_open_workflows(server_id: &str) {
    use cori_protocol::task_queue_for;
    use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};
    use cori_worker::workflow::{CoriWorkflow, ReauthSignalArgs};
    use temporalio_client::{WorkflowListOptions, WorkflowSignalOptions};

    let endpoint = match cori_run::temporal_endpoint::resolve() {
        Ok(e) => e,
        Err(_) => return,
    };
    if preflight_check(&endpoint.target, std::time::Duration::from_millis(300)).is_err() {
        return;
    }

    let identity = match OsUser.resolve() {
        Ok(id) => id,
        Err(_) => return,
    };
    let task_queue = task_queue_for(&identity);

    // Spin up a small tokio runtime; we only need it long enough to
    // enumerate + signal a handful of workflows. Use one worker thread
    // to keep this lightweight.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::debug!(error = %e, "notify_open_workflows: tokio runtime build failed");
            return;
        }
    };

    rt.block_on(async move {
        let runtime = match CoriTemporalRuntime::connect(
            endpoint.target.clone(),
            DEFAULT_NAMESPACE,
            task_queue.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "notify_open_workflows: connect failed");
                return;
            }
        };

        let query = format!(
            "TaskQueue = '{}' AND ExecutionStatus = 'Running'",
            task_queue
        );
        let opts = WorkflowListOptions::builder().limit(100_usize).build();

        use futures::StreamExt;
        let mut stream = runtime.client.list_workflows(query, opts);
        let mut notified: u32 = 0;
        while let Some(item) = stream.next().await {
            let info = match item {
                Ok(info) => info,
                Err(e) => {
                    tracing::debug!(error = %e, "notify_open_workflows: list page failed");
                    break;
                }
            };
            let handle = runtime
                .client
                .get_workflow_handle::<CoriWorkflow>(info.id().to_string());
            let args = ReauthSignalArgs {
                server_id: server_id.to_string(),
            };
            match handle
                .signal(
                    CoriWorkflow::reauth_completed,
                    args,
                    WorkflowSignalOptions::default(),
                )
                .await
            {
                Ok(()) => notified = notified.saturating_add(1),
                Err(e) => tracing::debug!(
                    workflow_id = %info.id(),
                    error = %e,
                    "notify_open_workflows: signal failed (workflow may not be in NeedsReauth wait)"
                ),
            }
        }
        if notified > 0 {
            println!(
                "  (notified {notified} open workflow{plural} of the sign-in)",
                plural = if notified == 1 { "" } else { "s" }
            );
        }
    });
}

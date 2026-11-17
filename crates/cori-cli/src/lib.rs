// SPDX-License-Identifier: Apache-2.0
//! The `cori` command line as a library: `run` is the whole binary, given its modules. `src/main.rs` passes none; a
//! binary built on this crate passes its own, which its `serve`, `up` and `work` load (`cori_server::module`) after the
//! core's own (`core_modules`). The commands are the same in every binary: those of a module call its routes on the server.
mod cli;
mod client;
mod cmd;
mod commands;
mod embedded_pg;
mod init;
mod session;
use clap::{CommandFactory, FromArgMatches};
use cli::*;
use std::sync::Arc;

pub use cori_server::module::{Module, Modules};

/// What a `cori` binary is built with: a name (`cori --version`) and the modules it adds to the core's own
/// (`core_modules`, which every binary carries).
pub struct Distribution { pub name: &'static str, pub modules: Vec<Arc<dyn Module>> }

impl Distribution {
    /// The core alone: its own modules only.
    pub fn core() -> Self { Self { name: "cori", modules: vec![] } }
}

/// The core's own modules, which `run` loads in every binary ahead of the distribution's. Each does nothing until its
/// settings name the service it uses: Vault, each app's credentials under a key of its own (`CORI_VAULT_ADDR`).
pub fn core_modules() -> Vec<Arc<dyn Module>> { vec![Arc::new(cori_vault::Vault)] }

/// The modules a binary loads: the core's, then the distribution's, which may not bring one of the core's again.
fn loaded(core: Vec<Arc<dyn Module>>, added: Vec<Arc<dyn Module>>) -> anyhow::Result<Modules> {
    if let Some(m) = added.iter().find(|m| core.iter().any(|c| c.id() == m.id())) {
        anyhow::bail!("module `{}` is the core's own: every binary carries it, so leave it out of the distribution's", m.id());
    }
    Ok(Modules(core.into_iter().chain(added).collect()))
}

/// Logs to stderr; when CORI_OTLP_ENDPOINT is set, spans are also exported over OTLP/gRPC (service name `cori`). The
/// processes of a deployment (`serve`, `work`, `sandbox`) log at `info` unless RUST_LOG says otherwise, the other commands
/// errors only; `CORI_LOG_FORMAT=json` writes one JSON object per line, for a log pipeline.
fn init_tracing() {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let deployed = matches!(std::env::args().nth(1).as_deref(), Some("serve" | "work" | "sandbox"));
    let filter = if std::env::var_os("RUST_LOG").is_none() && deployed { tracing_subscriber::EnvFilter::new("info,sqlx=warn") } else { tracing_subscriber::EnvFilter::from_default_env() };
    let json = std::env::var("CORI_LOG_FORMAT").map(|v| v.trim().eq_ignore_ascii_case("json")).unwrap_or(false);
    let fmt = if json { tracing_subscriber::fmt::layer().json().with_current_span(false).with_writer(std::io::stderr).boxed() } else { tracing_subscriber::fmt::layer().with_writer(std::io::stderr).boxed() };
    if let Ok(endpoint) = std::env::var("CORI_OTLP_ENDPOINT") {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_otlp::WithExportConfig as _;
        let exporter = opentelemetry_otlp::SpanExporter::builder().with_tonic().with_endpoint(endpoint).build().expect("otlp exporter");
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().with_batch_exporter(exporter).with_resource(opentelemetry_sdk::Resource::builder().with_service_name("cori").build()).build();
        let tracer = provider.tracer("cori");
        opentelemetry::global::set_tracer_provider(provider);
        tracing_subscriber::registry().with(filter).with(fmt).with(tracing_opentelemetry::layer().with_tracer(tracer)).init();
    } else {
        tracing_subscriber::registry().with(filter).with(fmt).init();
    }
}

/// The `cori` binary, with the core's modules and the distribution's.
pub async fn run(distribution: Distribution) -> anyhow::Result<()> {
    init_tracing();
    // `cori --version` names what the distribution adds: the core's own modules are in every binary
    let added: Vec<&'static str> = distribution.modules.iter().map(|m| m.id()).collect();
    let modules = loaded(core_modules(), distribution.modules)?;
    // the workers this binary runs (`work`, `up`) carry the modules' step hooks
    cori_engine::hooks::install(modules.llm_step_hooks());
    let resolved = session::apply();
    // a human token refreshes itself where it lives (your login, or an app folder's own session); an agent session
    // cannot, and the hint says whom to ask
    let stored = match (&resolved.source, &resolved.session, &resolved.login) {
        (session::Source::Session(p), Some(s), _) => Some((session::Stored::Folder(p.clone()), s)),
        (session::Source::Login, _, Some(l)) => session::global_path().map(|p| (session::Stored::Login(p), l)),
        _ => None,
    };
    if let Some((at, s)) = stored {
        match session::refresh_if_needed(&at, s).await {
            Ok(Some(n)) => unsafe { std::env::set_var("CORI_TOKEN", &n.token) },
            Ok(None) => {}
            Err(e) => eprintln!("{}: refresh failed: {e}", at.file().display()),
        }
        if s.expired() && s.kind == "agent" { eprintln!("warning: {}", session::sign_in_hint(resolved.app_root.as_deref(), &s.kind, &resolved.server)); }
    }
    let version: &'static str = if added.is_empty() { env!("CARGO_PKG_VERSION") }
        else { Box::leak(format!("{} ({}: {})", env!("CARGO_PKG_VERSION"), distribution.name, added.join(", ")).into_boxed_str()) };
    let mut matches = Cli::command().version(version).get_matches();
    let cli = Cli::from_arg_matches_mut(&mut matches).map_err(|e| e.format(&mut Cli::command())).unwrap_or_else(|e| e.exit());
    // `cori releases app | head` must end quietly: a command that prints restores the default SIGPIPE (Rust ignores it). Not
    // one that writes to child processes (the step runner, stdio MCP servers): with the default, a child that exits before
    // reading its input would kill the whole server or worker instead of failing one step.
    #[cfg(unix)] if !matches!(cli.cmd, Cmd::Serve { .. } | Cmd::Up { .. } | Cmd::Work { .. } | Cmd::Run { .. } | Cmd::Sandbox { .. }) { unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); } }
    // the local dev identity (`dev:<you>:it_admin`) is an admin and an approver: gated commands never fall back to it silently
    let implicit_default = matches!(resolved.source, session::Source::Default) && !std::env::args().any(|a| a == "--token" || a.starts_with("--token="));
    if implicit_default && matches!(cli.cmd, Cmd::Promote { .. } | Cmd::Rollback { .. } | Cmd::Undeploy { .. } | Cmd::Apps { what: AppsCmd::Protect { .. } | AppsCmd::Delete { .. } | AppsCmd::SyncClient { .. } }) {
        anyhow::bail!("no identity for {0}: no login to it, no CORI_TOKEN and no token in this app folder's .cori/session. Promote, approve, rollback, protect and deletions never use the local dev identity: run `cori login {0}`, or pass `--token dev:<sub>:<roles>` against a local `cori up`", resolved.server);
    }
    match cli.cmd {
        Cmd::Login { url, token, device, global, org, list, forget } => {
            if list { println!("{}", commands::list_logins()?); }
            else if forget { println!("{}", commands::forget_login(url.as_deref(), org.as_deref(), &resolved)?); }
            else { println!("{}", commands::login(&commands::LoginArgs { url, token, device, global, org }, &resolved).await?); }
        }
        Cmd::Pull { app, dir, hash, force, conn: Conn { server, token } } => { println!("{}", commands::pull(&app, dir, hash.as_deref(), force, &server, &token, &resolved.source).await?); }
        Cmd::Workflow { what: WorkflowCmd::New { id, name, description, params, connectors, endpoint, roles, schedule, schedule_tz, dir } } => {
            println!("{}", commands::workflow_new(&commands::NewWorkflow { dir, id, name, description, params, connectors, endpoint, roles, schedule, schedule_tz })?);
        }
        Cmd::Scaffold { connector, tool, workflow, name, after, dir, force, print, conn: Conn { server, token } } => {
            let out = commands::scaffold(&commands::ScaffoldArgs { dir, connector, tool, workflow, name, after, force, print }, &server, &token).await?;
            if print { print!("{out}"); } else { println!("{out}"); }
        }
        c @ Cmd::Init { .. } => init::run(c, &resolved).await?,
        c @ Cmd::Apps { .. } => cmd::apps::run(c).await?,
        c @ (Cmd::Doctor { .. } | Cmd::Check { .. } | Cmd::Run { .. } | Cmd::Gen { .. }) => cmd::dev::run(c, &resolved).await?,
        c @ (Cmd::Serve { .. } | Cmd::Work { .. } | Cmd::Up { .. } | Cmd::Sandbox { .. }) => cmd::platform::run(c, &modules).await?,
        c @ (Cmd::Deploy { .. } | Cmd::Releases { .. } | Cmd::Promote { .. } | Cmd::Rollback { .. } | Cmd::Undeploy { .. } | Cmd::Reseed { .. }) => cmd::release::run(c).await?,
        c @ Cmd::Connectors { .. } => cmd::connectors::run(c).await?,
        c @ (Cmd::Secrets { .. } | Cmd::Pools { .. }) => cmd::admin::run(c).await?,
        c @ Cmd::Identity { .. } => cmd::identity::run(c).await?,
        c @ Cmd::Llm { .. } => cmd::llm::run(c).await?,
        c @ Cmd::Store { .. } => cmd::store::run(c).await?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str);
    impl Module for Named { fn id(&self) -> &'static str { self.0 } }

    #[test]
    fn every_binary_carries_the_cores_modules_first_and_never_twice() {
        assert_eq!(loaded(core_modules(), vec![]).unwrap().ids(), ["vault"], "the core alone");
        assert_eq!(loaded(core_modules(), vec![Arc::new(Named("llm-ops"))]).unwrap().ids(), ["vault", "llm-ops"]);
        let again = loaded(core_modules(), vec![Arc::new(Named("vault"))]).err().expect("the core's module twice");
        assert!(again.to_string().contains("module `vault` is the core's own"), "{again}");
    }
}

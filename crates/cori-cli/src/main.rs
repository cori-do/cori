//! The `cori` command-line interface.
//!
//! Phase 2 wires `run`/`runs` against the disk-as-truth layout in
//! `~/.cori/{cache,runs,runtime,state}`. There is no registry; every
//! `cori run` compiles (or loads from cache) the workflow folder
//! supplied on the CLI.

mod commands;
mod config;
mod embedded;
mod paths;
mod planner;
mod remote;
mod runtime;
mod temporal_endpoint;
mod workflow_loader;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Cori — author and run typed TypeScript workflows from your terminal.
#[derive(Debug, Parser)]
#[command(name = "cori", version = env!("CARGO_PKG_VERSION"), propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install the Cori skill into a supported agent.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Execute a workflow folder (local path or remote git reference).
    Run {
        /// Path to the workflow folder or a remote git ref
        /// (e.g. `github.com/org/workflows/translate@v1.1.12`).
        path: String,
        /// Emit the run trace as JSON instead of the friendly table.
        #[arg(long)]
        json: bool,
        /// Validate the plan without spawning any external process.
        #[arg(long)]
        dry_run: bool,
        /// For remote workflows: re-resolve the ref before running.
        /// No-op for exact tags/shas and for local paths.
        #[arg(long)]
        update: bool,
        /// Skip the first-run consent prompt for remote workflows.
        /// Equivalent to setting `CORI_ASSUME_YES=1`.
        #[arg(long = "yes", short = 'y')]
        assume_yes: bool,
        /// `key=value` parameters forwarded as the input to step 1.
        #[arg(value_name = "PARAM")]
        params: Vec<String>,
    },
    /// Inspect previously recorded runs.
    Runs {
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Read or write CLI configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Put this machine in the loop as a Cori worker.
    ///
    /// Default identity is the current OS user (queue
    /// `cori.user.<id>`). With `--shared <name>`, runs as a shared
    /// service worker (queue `cori.service.<name>`) whose credentials
    /// become usable by any authorized user whose run routes here.
    Work {
        /// Run as a shared service worker named `<name>`.
        #[arg(long, value_name = "NAME")]
        shared: Option<String>,
    },
    /// Sign in to a capability (MCP server, CLI, LLM provider).
    ///
    /// Idempotent and refresh-aware: rerunning with a still-valid
    /// token is a no-op. Tokens are stored in the OS keychain (with an
    /// encrypted-file fallback) and scoped to the current OS user.
    Login {
        /// Capability id — e.g. `notion`, `gws`, `openai`.
        capability: String,
    },
    /// Preflight a workflow folder — print per-step readiness and
    /// capability auth status without starting the run. Exit code
    /// reflects readiness (`0` ready, `2` not ready).
    Check {
        /// Path to the workflow folder or remote git ref.
        path: String,
        /// For remote refs: re-resolve before preflight.
        #[arg(long)]
        update: bool,
        /// Skip the consent prompt for remote workflows.
        #[arg(long = "yes", short = 'y')]
        assume_yes: bool,
    },
    /// Print machine-scoped overview: endpoint, identity, capabilities,
    /// and workers currently visible on the cluster.
    Status,
    /// Inspect a workflow folder — manifest, steps, required
    /// capabilities, and recent runs. Accepts a remote ref; if the
    /// ref has not been fetched locally, history is shown but the
    /// workflow body is omitted.
    Show {
        /// Path to the workflow folder or remote git ref.
        path: String,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Install the embedded Cori skill into an agent's skill directory.
    Install {
        /// Target agent. One of: claude-code, cursor, gemini-cli, copilot-cli.
        #[arg(long)]
        agent: Option<String>,
        /// Install to an arbitrary directory instead of an agent's
        /// conventional path. Mutually exclusive with `--agent`.
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum RunsCommand {
    /// List recent runs, most-recent first.
    List {
        /// Restrict to runs of one workflow.
        #[arg(long)]
        workflow_id: Option<String>,
        /// Maximum number of rows.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Show one run's trace.
    Show {
        run_id: String,
        /// Print only one activity's trace entry.
        #[arg(long)]
        activity: Option<String>,
        /// Include full activity output (default summarises).
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print one key or every key.
    Get { key: Option<String> },
    /// Write a key. Values are auto-coerced to bool/int/float when they
    /// parse cleanly; otherwise stored as a string.
    Set { key: String, value: String },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Command::Config { command }) => match command {
            ConfigCommand::Get { key } => commands::config::get(key.as_deref()),
            ConfigCommand::Set { key, value } => commands::config::set(&key, &value),
        },
        Some(Command::Run {
            path,
            json,
            dry_run,
            update,
            assume_yes,
            params,
        }) => commands::run::run(path, params, json, dry_run, update, assume_yes),
        Some(Command::Skill { command }) => match command {
            SkillCommand::Install { agent, path } => commands::skill::install(agent, path),
        },
        Some(Command::Runs { command }) => match command {
            RunsCommand::List {
                workflow_id,
                limit,
                json,
            } => commands::runs::list(workflow_id.as_deref(), limit, json),
            RunsCommand::Show {
                run_id,
                activity,
                full,
                json,
            } => commands::runs::show(&run_id, activity.as_deref(), full, json),
        },
        Some(Command::Work { shared }) => commands::work::work(shared),
        Some(Command::Login { capability }) => commands::login::login(&capability),
        Some(Command::Check {
            path,
            update,
            assume_yes,
        }) => commands::check::check(path, update, assume_yes),
        Some(Command::Status) => commands::status::status(),
        Some(Command::Show { path }) => commands::show::show(path),
    }
}

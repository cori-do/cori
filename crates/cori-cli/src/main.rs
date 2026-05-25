//! The `cori` command-line interface.
//!
//! Phase 2 wires up `init --local`, `workflows register|list|show`, and
//! `config get|set`. Every other verb still prints a "not implemented"
//! notice so `--help` is accurate.

mod commands;
mod config;
mod embedded;
mod paths;
mod registry;
mod runtime;

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
    /// Authenticate against a Cori account (optional in v1).
    Login,
    /// Run the bundled hello-world demo workflow.
    Demo,
    /// Install the Cori skill into a supported agent.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Manage registered workflows.
    Workflows {
        #[command(subcommand)]
        command: WorkflowsCommand,
    },
    /// Execute a registered workflow.
    Run {
        /// ID of a registered workflow.
        id: String,
        /// Emit the run trace as JSON instead of the friendly table.
        #[arg(long)]
        json: bool,
        /// Validate the plan without spawning any external process. `cli`,
        /// `mcp_tool`, and `llm` steps return mocked outputs; `code` and
        /// `builtin` steps still run for real (they're pure).
        #[arg(long)]
        dry_run: bool,
        /// `key=value` parameters forwarded as the input to step 1.
        ///
        /// Values are parsed as JSON when possible (numbers, booleans,
        /// objects, arrays, `null`); otherwise treated as a string.
        #[arg(value_name = "PARAM")]
        params: Vec<String>,
    },
    /// Inspect previously recorded runs.
    Runs {
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Start the long-running Cori worker process.
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Start the local HTTP server and web UI.
    Serve {
        #[command(subcommand)]
        command: ServeCommand,
    },
    /// Run the worker and server together for local development.
    Dev,
    /// Initialise the local Cori state directory at `~/.cori/`.
    Init {
        /// Required in v1; reserves the flag namespace for future modes.
        #[arg(long)]
        local: bool,
    },
    /// Read or write CLI configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Inspect connected workers.
    Workers {
        #[command(subcommand)]
        command: WorkersCommand,
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
enum WorkflowsCommand {
    /// Validate and register a runbook directory.
    Register {
        /// Path to the runbook directory (containing `manifest.md` and
        /// `steps/`).
        path: PathBuf,
    },
    /// List every registered workflow.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show a registered workflow's manifest or a single field.
    Show {
        /// Workflow id.
        id: String,
        /// Print only a single top-level field or `## <section>` body.
        #[arg(long)]
        field: Option<String>,
        /// Emit the compiled JSON representation instead of the manifest.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorkerCommand {
    /// Start the long-running worker daemon (supervises Temporal and
    /// hot-reloads workflows from `~/.cori/runbooks/`).
    Start,
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
enum ServeCommand {
    /// Start the local HTTP server + web UI (default bind `127.0.0.1:7510`).
    Start {
        /// Bind address, e.g. `0.0.0.0:7510`. Defaults to `127.0.0.1:7510`.
        #[arg(long)]
        bind: Option<String>,
        /// Allow binding to a non-loopback address. There is no auth in
        /// v1 — only set this if you understand what you're exposing.
        #[arg(long)]
        insecure: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorkersCommand {
    Status,
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
        Some(Command::Init { local }) => commands::init::run(local),
        Some(Command::Workflows { command }) => match command {
            WorkflowsCommand::Register { path } => commands::workflows::register(&path),
            WorkflowsCommand::List { json } => commands::workflows::list(json),
            WorkflowsCommand::Show { id, field, json } => {
                commands::workflows::show(&id, field.as_deref(), json)
            }
        },
        Some(Command::Config { command }) => match command {
            ConfigCommand::Get { key } => commands::config::get(key.as_deref()),
            ConfigCommand::Set { key, value } => commands::config::set(&key, &value),
        },
        Some(Command::Run {
            id,
            json,
            dry_run,
            params,
        }) => commands::run::run(&id, params, json, dry_run),
        Some(Command::Worker { command }) => match command {
            WorkerCommand::Start => commands::worker::start(),
        },
        Some(Command::Serve { command }) => match command {
            ServeCommand::Start { bind, insecure } => commands::serve::start(bind, insecure),
        },
        Some(Command::Dev) => commands::dev::run(),
        Some(Command::Demo) => commands::demo::run(),
        Some(Command::Login) => commands::login::run(),
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
        Some(other) => {
            let name = match other {
                Command::Workers { .. } => "workers",
                Command::Login
                | Command::Demo
                | Command::Skill { .. }
                | Command::Init { .. }
                | Command::Run { .. }
                | Command::Runs { .. }
                | Command::Workflows { .. }
                | Command::Config { .. }
                | Command::Worker { .. }
                | Command::Serve { .. }
                | Command::Dev => {
                    unreachable!()
                }
            };
            eprintln!("`cori {name}` is not implemented yet — see cori-v1-roadmap.md");
            std::process::exit(2);
        }
    }
}

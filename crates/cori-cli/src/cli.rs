// SPDX-License-Identifier: Apache-2.0
//! The command line: `cori <command>` and its flags (clap).
use crate::session;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cori", version, about = "Cori — durable, typed, permissioned backend for agent-built apps", after_help = "\
Build an app (in its folder):  init · pull · workflow · scaffold · store · check · run · gen
Ship it:                       deploy · releases · promote · approve · rollback · undeploy · reseed
Apps and sessions:             apps · login · doctor
Run the platform:              up · serve · work
Administer the org:            connectors · secrets · pools · llm · auth · identity")]
pub(crate) struct Cli { #[command(subcommand)] pub(crate) cmd: Cmd }

/// The server and identity of every command that talks to a server: the flags, else `CORI_SERVER` / `CORI_TOKEN`, which
/// `session::apply` fills from the app's `.cori/session` (its server, and a token of its own) or your logins in
/// `~/.cori/config.toml`, a token for the server it was issued for only, else a local `cori up` with the dev
/// identity of the person running the command (`session::decide`).
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct Conn {
    /// Server URL (default: the app's .cori/session, else the server you last logged in to, else a local `cori up`)
    #[arg(long, env = "CORI_SERVER", default_value = session::LOCAL_SERVER)] pub(crate) server: String,
    /// Bearer token (default: the app folder's own, else your login to that server; against a local `cori up`, your dev identity)
    #[arg(long, env = "CORI_TOKEN", hide_env_values = true, default_value_t = session::default_token())] pub(crate) token: String,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Compile workflows: `cori check workflows/<id>…` or `cori check <app>` (every workflow of the app): dataflow proof, effects, lineage, generated context
    #[command(display_order = 6)]
    Check { dirs: Vec<PathBuf>, #[arg(long)] json: bool, #[arg(long)] no_deno: bool,
        /// Print the conventions (layout, step kinds and their option shapes, result templates) instead of compiling; --json for a JSON object
        #[arg(long)] conventions: bool, #[command(flatten)] conn: Conn },
    /// Run a workflow locally on a Temporal dev server (connectors answered by dev mocks): `cori run workflows/<id> k=v`
    #[command(display_order = 7)]
    Run { dir: PathBuf,
        /// Record every live connector call (with --no-mock <connector>) as the connector's dev fixture
        #[arg(long)] record: bool,
        /// Run against a real Cori Store (Postgres admin URL): stores are provisioned from the app folder, mocks bypassed.
        /// Default: the embedded Postgres of a running `cori up`, else the store fixtures
        #[arg(long, env = "CORI_STORE_DATABASE_URL", hide_env_values = true)] store_database_url: Option<String>,
        /// Use the store fixtures even when `cori up` runs its Postgres
        #[arg(long)] no_store: bool,
        /// Drop the app's local store databases first: migrations and seed/*.sql apply again from scratch (after editing them)
        #[arg(long)] reset_store: bool,
        #[arg(value_name = "KEY=VALUE")] params: Vec<String>, /// Pre-record a human decision: step=<json>
        #[arg(long = "decide")] decide: Vec<String>, #[arg(long)] json: bool, #[arg(long, env = "CORI_TEMPORAL_ADDRESS", default_value = "http://127.0.0.1:7233")] temporal: String, /// Temporal namespace
        #[arg(long, env = "CORI_TEMPORAL_NAMESPACE", default_value = "default")] namespace: String,
        /// Ignore the dev mock of these connectors (use their live transport)
        #[arg(long = "no-mock")] no_mock: Vec<String>,
        /// Answer `llm` steps from a fixture whatever `[llm]` says (default file: `[llm.dev] mock`, else fixtures/llm.json)
        #[arg(long = "mock-llm", num_args = 0..=1, value_name = "FILE")] mock_llm: Option<Option<String>>,
        /// Run as this user: <sub>:<role,role> (a `dev:` prefix is accepted). Default: `dev` holding the endpoint's first `allow` role
        #[arg(long = "user")] user: Option<String>, #[command(flatten)] conn: Conn },
    /// Start the single-org server. Every flag also reads its environment variable (a container sets those).
    /// Listening beyond loopback requires the production settings (builtin auth, Postgres, CORI_MASTER_KEY, a worker token,
    /// no mocks, an https public URL); --allow-insecure (CORI_ALLOW_INSECURE) overrides that for a demo.
    #[command(display_order = 20)]
    Serve { #[arg(long, env = "CORI_LISTEN", default_value = "127.0.0.1:8080")] listen: String,
        /// Platform database: postgres://… (required)
        #[arg(long, env = "CORI_DATABASE_URL", hide_env_values = true)] database_url: Option<String>,
        /// Bundles, screenshots, registry (and master.key without CORI_MASTER_KEY); default ~/.cori/server
        #[arg(long, env = "CORI_DATA_DIR")] data_dir: Option<PathBuf>,
        #[arg(long, env = "CORI_TEMPORAL_ADDRESS", default_value = "http://127.0.0.1:7233")] temporal: String, /// Temporal namespace
        #[arg(long, env = "CORI_TEMPORAL_NAMESPACE", default_value = "default")] namespace: String,
        /// Shared task queue workers poll (a pool's queue is derived from it)
        #[arg(long, env = "CORI_TASK_QUEUE", default_value = "cori.shared")] task_queue: String,
        /// dev | builtin (any OIDC issuer: CORI_AUTH_ISSUER, CORI_AUTH_AUDIENCE, CORI_AUTH_ROLES_CLAIM, CORI_AUTH_SUBJECT_CLAIM) | off
        #[arg(long, env = "CORI_AUTH", default_value = "dev")] auth: String,
        #[arg(long, env = "CORI_DEV_MOCKS")] dev_mocks: bool,
        /// The URL people and agents reach this server at (OIDC redirects, generated clients, links in notifications)
        #[arg(long, env = "CORI_PUBLIC_URL")] public_url: Option<String>,
        /// The URL workers call the server at, e.g. the cluster service `http://cori-server:8080` (default: the public URL)
        #[arg(long, env = "CORI_INTERNAL_URL")] internal_url: Option<String>,
        #[arg(long, env = "CORI_APPROVER_ROLES", value_delimiter = ',')] approver_roles: Vec<String>,
        #[arg(long, env = "CORI_INGEST_TOKEN", hide_env_values = true, default_value = "")] ingest_token: String,
        #[arg(long, env = "CORI_STORE_DATABASE_URL", hide_env_values = true)] store_database_url: Option<String>,
        #[arg(long = "no-mock")] no_mock: Vec<String>,
        /// Start even with laptop settings on a non-loopback address (demo only)
        #[arg(long, env = "CORI_ALLOW_INSECURE")] allow_insecure: bool,
        /// Check, with these settings, everything the server relies on (databases, issuer, client hook, Temporal, bundle
        /// store, models, fronts, sandboxes, the modules' services), say what to fix, and exit: non-zero when a check fails
        #[arg(long)] preflight: bool,
        /// With --preflight: the checks as JSON
        #[arg(long)] json: bool },
    /// Poll a task queue as a worker (bundle mode: fetches bundles from the server by hash)
    #[command(display_order = 21)]
    Work { #[arg(long, env = "CORI_TASK_QUEUE", default_value = "cori.shared")] queue: String, #[arg(long, env = "CORI_TEMPORAL_ADDRESS", default_value = "http://127.0.0.1:7233")] temporal: String, /// Temporal namespace
        #[arg(long, env = "CORI_TEMPORAL_NAMESPACE", default_value = "default")] namespace: String,
        /// Poll the sharded shared queue `<queue>.0 … <queue>.N-1` (must match the server's CORI_SHARDS)
        #[arg(long, env = "CORI_SHARDS", default_value_t = 1)] shards: u32,
        /// Poll a named pool created on the server (`cori pools create <name>`); the key proves this machine may serve it
        #[arg(long)] pool: Option<String>, #[arg(long, env = "CORI_POOL_KEY", hide_env_values = true)] key: Option<String>, #[arg(long, env = "CORI_SERVER", default_value = "http://127.0.0.1:8080")] server: String },
    /// Sign in to a server: your login is kept in `~/.cori/config.toml`, one per server and organization, and the server
    /// becomes the default outside app folders; inside an app folder, the folder works with that server (and organization)
    /// too (`.cori/session`). Without --token the org's OIDC issuer is used (browser + PKCE, or --device). Without a URL:
    /// the folder's server, else your default one, else Cori Cloud (https://app.cori.do), where a new person signs up.
    #[command(display_order = 17)]
    Login { url: Option<String>, #[arg(long)] token: Option<String>, /// Device authorization grant (no browser on this machine)
        #[arg(long)] device: bool, /// Only save your login: leave the app folder this runs in working with the server it has
        #[arg(long)] global: bool, /// The organization to sign in to (its slug), on a server hosting several: the issuer asks otherwise
        #[arg(long)] org: Option<String>, /// List your logins: server, organization, the default one, when each expires
        #[arg(long, conflicts_with_all = ["token", "device", "forget"])] list: bool, /// Forget your login to the server (in --org's organization)
        #[arg(long, conflicts_with_all = ["token", "device"])] forget: bool },
    /// Check this machine and (optionally) a server: deno, temporal, reachability, database, store, secrets, and the
    /// identity checks (the issuer's discovery and keys, your token, one token exchange per connector audience)
    #[command(display_order = 18)]
    Doctor { #[command(flatten)] conn: Conn },
    /// Releases of an app (`v1`, `v2`…): the live one, those newer than live and what each needs to go live, older ones,
    /// and the go-live history
    #[command(display_order = 10)]
    Releases { app: String, #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// Go back to the release live before this one: no approval and no migration (`cori promote <app> v6` goes to any
    /// release, the same way)
    #[command(display_order = 13)]
    Rollback { app: String, #[command(flatten)] conn: Conn },
    /// The org's connector registry: `cori connectors list|find <words>|add|export|requests|refresh <name>|remove <name>`
    #[command(display_order = 22)]
    Connectors { #[command(subcommand)] what: ConnectorsCmd },
    /// LLM bindings and captured calls (`bind`, `calls`); with LLM Ops on the server, the evaluation loop (`eval`, `dataset`, `finetune`)
    #[command(display_order = 25)]
    Llm { #[command(subcommand)] what: LlmCmd },
    /// Cori Store: `cori store new <name>|list|tools <store>|query <store> <sql>`
    #[command(display_order = 5)]
    Store { #[command(subcommand)] what: StoreCmd },
    /// Manage broker secrets: `cori secrets set <name> [--app <app>]` (prompts, or --from-env VAR), `cori secrets list [--app <app>]`
    #[command(display_order = 23)]
    Secrets { #[command(subcommand)] what: SecretsCmd },
    /// Manage worker pools: `cori pools create <name>` (prints the key once), `cori pools list`
    #[command(display_order = 24)]
    Pools { #[command(subcommand)] what: PoolsCmd },
    /// What the identity provider is told of apps and roles: `cori identity catalog|events|retry <id>`, and
    /// `cori identity test-hook <url>`, `test-people <url>` and `test-access <url>`, which check a receiver of the client
    /// hook, a directory of people and an access webhook against their contracts
    #[command(display_order = 26)]
    Identity { #[command(subcommand)] what: IdentityCmd },
    /// Where a cluster's workers and server run builders' code (step modules, `cli` programs, stdio MCP servers, the deploy
    /// checks): `cori sandbox serve`, in a pod of its own under gVisor that holds no credential
    #[command(display_order = 21)]
    Sandbox { #[command(subcommand)] what: SandboxCmd },
    /// Everything local: Temporal dev server, the server with dev mocks, and an in-process worker (on CORI_TASK_QUEUE, default
    /// `cori.shared`: another `cori up` on the same Temporal polls that one too)
    #[command(display_order = 19)]
    Up { #[arg(long, default_value = "127.0.0.1:8080")] listen: String, /// master.key, bundles, the embedded Postgres (platform database and stores); default ~/.cori/up
        #[arg(long, env = "CORI_DATA_DIR")] data_dir: Option<PathBuf>,
        /// No stores on the embedded Postgres (dev mocks answer store steps); the platform database stays on it
        #[arg(long)] no_store: bool,
        /// Postgres admin URL for Cori Store (a database per store is created on it); without it, store steps use dev mocks
        #[arg(long, env = "CORI_STORE_DATABASE_URL", hide_env_values = true)] store_database_url: Option<String>,
        /// Ignore the dev mock of these connectors (e.g. --no-mock tickets to use the real store)
        #[arg(long = "no-mock")] no_mock: Vec<String> },
    /// Create an app folder (the folder is the app: cori.toml, AGENTS.md, workflows/, fixtures/, dist/, a git repository): empty (--roles), or from a template: `team-requests` (a small finished app that runs on any server as it is, its React front built) or `support-desk` (the demo of every step kind, with company connectors).
    /// With a server connection (session, env, `cori login`) the app is registered on the server and `.cori/session` is written.
    #[command(display_order = 1)]
    Init { dir: PathBuf, /// App id on the server (default: the folder name)
        #[arg(long)] app: Option<String>, /// Display name (cori.toml `[app] name` and the server registration; default: the app id, or the template's)
        #[arg(long)] name: Option<String>, #[arg(long)] template: Option<String>, /// The app's roles, strongest first (`manager,employee`): cori.toml lists them in this order, and a person holding several acts in the stores as the first. Default: the server's reservation, else `admin,app_user`; a template keeps its own
        #[arg(long, value_delimiter = ',')] roles: Vec<String>,
        /// The front: `react` (Vite + React + TypeScript + Tailwind in front/, built into dist/; the default for a new app), `plain` (one dist/index.html), `none`. A template keeps its own front unless this is given
        #[arg(long, value_enum)] front: Option<Front>, /// Do not run `git init`
        #[arg(long)] no_git: bool, /// Do not register the app on the server
        #[arg(long)] no_register: bool, #[command(flatten)] conn: Conn },
    /// Fetch an existing app from the server into a folder (promoted version, or --hash): `cori pull my-app ./my-app`
    #[command(display_order = 2)]
    Pull { app: String, dir: Option<PathBuf>, #[arg(long)] hash: Option<String>, #[arg(long)] force: bool, #[command(flatten)] conn: Conn },
    /// Workflows of the app: `cori workflow new <id> --params ticket_id:string --connectors helpdesk --endpoint sync --roles support_agent`
    #[command(display_order = 3)]
    Workflow { #[command(subcommand)] what: WorkflowCmd },
    /// Write a typed step file for a connector tool and list it in `steps:`: `cori scaffold helpdesk getTicket --workflow triage [--name load_ticket] [--after classify]`
    #[command(display_order = 4)]
    Scaffold { connector: String, tool: String, #[arg(long)] workflow: String, /// Step name (snake_case; default: the tool name)
        #[arg(long)] name: Option<String>, /// Insert after this step (`start` = first); default: last
        #[arg(long)] after: Option<String>, /// App folder (default: the current directory)
        #[arg(long, default_value = ".")] dir: PathBuf, #[arg(long)] force: bool, /// Print the file instead of writing it
        #[arg(long)] print: bool, #[command(flatten)] conn: Conn },
    /// Apps on the server: `cori apps list|get <id>|create <id> [--name] [--roles a,b] [--builders u1,u2]|protect <id>|delete <id>|sync-client <id>`
    #[command(display_order = 16)]
    Apps { #[command(subcommand)] what: AppsCmd },
    /// Generate the typed client (`front/src/lib/cori.ts`) for an app, from a local app folder or a server
    #[command(display_order = 8)]
    Gen { #[command(subcommand)] what: GenCmd },
    /// Deploy the app folder (default: the current one) as a new version: a tar.zst bundle with the git commit recorded, compiled and reviewed,
    /// with its own seeded sandbox stores and a preview url. Nothing users see changes until `cori promote`.
    #[command(display_order = 9)]
    Deploy { dir: Option<PathBuf>, /// Refuse to upload a tree with uncommitted changes
        #[arg(long)] require_clean: bool, /// Release message (default: the last commit's subject)
        #[arg(long, short = 'm')] message: Option<String>, /// Print the server's response (hash, status, review, sandbox, preview_url…) as JSON
        #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// Promote a deployed version: it becomes what the app's address serves and its pending migrations run on the live stores.
    /// A human action; on a protected app (or when the review adds writes) it waits for an approver unless you are one: an
    /// approver's promote goes live at once and closes the request waiting for that release.
    #[command(display_order = 11)]
    Promote { app: String, /// Release (`v9`), or a version hash or prefix (`cori releases <app>`)
        hash: String, #[command(flatten)] conn: Conn },
    /// Delete a deployed version: its sandbox databases, its bundle and its preview url (never the promoted version).
    /// `--force` terminates the runs still going on it. Owner, builders and admins.
    #[command(display_order = 14)]
    Undeploy { app: String, /// Release (`v9`), or a version hash or prefix (`cori releases <app>`)
        hash: String, #[arg(long)] force: bool, #[command(flatten)] conn: Conn },
    /// Drop and recreate a version's sandbox stores (migrations + seed/*.sql again): start testing from a clean slate
    #[command(display_order = 15)]
    Reseed { app: String, hash: String, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum ConnectorsCmd {
    /// Declare a connector in the org registry (admin): `cori connectors add https://mcp.acme.internal/mcp --name helpdesk`
    Add { /// Streamable HTTP MCP URL (http[s]://…), `activities` (with --yaml), or omit and pass --command for stdio
        url: Option<String>,
        /// `cori connectors add activities --yaml ./activities.yaml`: declare a native-activity connector from the team's declaration
        #[arg(long)] yaml: Option<PathBuf>, #[arg(long)] name: Option<String>, #[arg(long, default_value = "mcp")] kind: String, #[arg(long, value_delimiter = ' ', num_args = 1..)] command: Option<Vec<String>>, #[arg(long)] description: Option<String>, #[arg(long)] audience: Option<String>, #[arg(long)] secret: Option<String>, #[arg(long)] worker: Option<String>, #[arg(long, value_delimiter = ',')] roles: Vec<String>, #[arg(long)] base_url: Option<String>, #[arg(long)] auth_env: Option<String>,
        /// Import every server of an MCP host config (.mcp.json / claude_desktop_config.json shape: { "mcpServers": { name: { command, args, url } } })
        #[arg(long)] from: Option<PathBuf>,
        #[command(flatten)] conn: Conn },
    /// Write the declared servers into an MCP host config (`.mcp.json` shape) so the agent calls the same servers directly
    Export { #[arg(long, default_value = ".mcp.json")] to: PathBuf, #[command(flatten)] conn: Conn },
    /// Open requests from builders for servers not yet declared (admin)
    Requests { #[command(flatten)] conn: Conn },
    /// Declared connectors with their tools (one line each); --json for the full rows with schemas
    List { #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// Search the declared tools by words: `cori connectors find "post a message"` (ranked, with input schemas as --json)
    Find { query: Vec<String>, #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// Capture a declared connector's tools again (admin)
    Refresh { name: String, #[command(flatten)] conn: Conn },
    /// Remove a connector from the registry (admin)
    Remove { name: String, #[command(flatten)] conn: Conn },
    /// The key a native-activity connector's worker opens its tokens with (admin): set it as CORI_CONNECTOR_KEY there
    Key { name: String, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum WorkflowCmd {
    /// Create workflows/<id>/ with workflow.yaml (`steps: []`), README.md and types.ts
    New { id: String, #[arg(long)] name: Option<String>, #[arg(long)] description: Option<String>,
        /// Parameters as name:type[:description]; a trailing `?` on the type makes it optional (types: string, number, boolean, enum, list, object)
        #[arg(long, value_delimiter = ',')] params: Vec<String>, #[arg(long, value_delimiter = ',')] connectors: Vec<String>,
        /// sync | async (default sync when no --schedule)
        #[arg(long)] endpoint: Option<String>, /// Roles allowed to call the endpoint
        #[arg(long, value_delimiter = ',')] roles: Vec<String>, /// Cron expression instead of an endpoint, e.g. "0 7 * * 1-5"
        #[arg(long)] schedule: Option<String>, #[arg(long, default_value = "UTC")] schedule_tz: String,
        /// App folder (default: the current directory)
        #[arg(long, default_value = ".")] dir: PathBuf },
}

#[derive(Subcommand)]
pub(crate) enum AppsCmd {
    /// Apps you own or build (admins see all)
    List { #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// One app: owner, builders, roles, promoted and last version, urls, the `cori pull` command
    Get { app: String, #[command(flatten)] conn: Conn },
    /// Register the app's own sign-in client again through the server's client hook (owner or admin; idempotent)
    SyncClient { app: String, #[command(flatten)] conn: Conn },
    /// Protect an app: every promotion needs an approver (owner or admin); `--off` lifts it
    Protect { app: String, #[arg(long)] off: bool, #[command(flatten)] conn: Conn },
    /// Delete an app for good: every version, the live and sandbox store databases, schedules, runs and approvals (the audit
    /// keeps its history). Owner or admin, never on a protected app; an approver decides (yours runs at once, anyone else's
    /// waits in the approvals inbox). Asks you to type the app id back unless `--confirm <app>` is given
    Delete { app: String, #[arg(long)] confirm: Option<String>, #[command(flatten)] conn: Conn },
    /// Reserve an app id (you become its owner; builder or admin role)
    Create { app: String, #[arg(long)] name: Option<String>, /// The app's roles, strongest first (`manager,employee`): `cori init` writes them into cori.toml in this order
        #[arg(long, value_delimiter = ',')] roles: Vec<String>, /// Other principals allowed to publish versions
        #[arg(long, value_delimiter = ',')] builders: Vec<String>, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum LlmCmd {
    /// Bind a model to a step: cori llm bind <app> <workflow> <step> --model X [--provider mock|openai|vllm] [--mode primary|shadow|off] (shadow and off: LLM Ops)
    Bind { app: String, workflow: String, step: String, #[arg(long)] model: String, #[arg(long, default_value = "openai")] provider: String, #[arg(long, default_value = "primary")] mode: String, #[arg(long, default_value = "small")] tier: String, #[command(flatten)] conn: Conn },
    /// Recent captured calls
    Calls { app: String, #[command(flatten)] conn: Conn },
    /// Shadow report per step with a promote/hold recommendation (LLM Ops)
    Eval { app: String, #[command(flatten)] conn: Conn },
    /// Export the fine-tuning dataset (JSONL) to a file (LLM Ops)
    Dataset { app: String, #[arg(long, default_value = "dataset.jsonl")] out: PathBuf, #[command(flatten)] conn: Conn },
    /// Fine-tune a model on a step's captured calls, on the server with the org's provider (LLM Ops; `mock` only builds the training file)
    Finetune { app: String, #[arg(long)] workflow: String, #[arg(long)] step: String, #[arg(long, default_value = "mock")] provider: String, #[arg(long, default_value = "gpt-4.1-mini")] base_model: String, /// Where to save the training file the server built
        #[arg(long, default_value = "finetune.jsonl")] out: PathBuf, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum StoreCmd {
    /// Scaffold stores/<name>/ (rules.yaml, roles/<role>.yaml per app role, migrations/0001_init.sql), fixtures/<name>.json and the cori.toml binding
    New { name: String, /// Roles to write role files for (default: the app's [auth].roles)
        #[arg(long, value_delimiter = ',')] roles: Vec<String>, /// Rows belong to a user: `--tenant owner` scopes the table to the caller (a todo list); default: a shared table
        #[arg(long)] tenant: Option<String>, /// App folder (default: the current directory)
        #[arg(long, default_value = ".")] dir: PathBuf },
    /// The store databases of an app (live and sandboxes); every app's when no app is known
    List { /// App id (default: the cori.toml of the current folder)
        #[arg(long)] app: Option<String>, #[command(flatten)] conn: Conn },
    /// The typed tools of a store (getItem, listItems, …) per role. In an app folder: offline, from stores/<store>/ migrations
    /// and roles. With --online, --version or --app (or outside an app folder): what your session's role gets from the server
    Tools { store: String, /// Only this store role (offline)
        #[arg(long)] role: Option<String>, /// App folder (default: the current directory)
        #[arg(long, default_value = ".")] dir: PathBuf, /// Ask the server (the live database) instead of reading the folder
        #[arg(long)] online: bool, #[arg(long)] app: Option<String>, /// Version hash or prefix: the sandbox instead of the live database (implies --online)
        #[arg(long)] version: Option<String>, #[command(flatten)] conn: Conn },
    /// A read-only query on a store (what your role may read): the live database, or a version's sandbox with --version (an
    /// agent session reads sandboxes only)
    Query { store: String, sql: String, #[arg(long)] app: Option<String>, #[arg(long)] version: Option<String>, #[command(flatten)] conn: Conn },
    /// New passwords for the store databases' own logins (admin): every app's, or `--app`'s. Each is kept as its app's
    /// credential, then set on the database; open connections keep working
    RotateLogins { #[arg(long)] app: Option<String>, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum SecretsCmd {
    /// Store a secret in the broker (admin): the value from stdin, or `--from-env VAR`. With `--app`, the app's own
    /// credential (an admin or the app's owner): its runs find it before the deployment's secret of the same name
    Set { name: String, #[arg(long)] from_env: Option<String>, #[arg(long, default_value = "token")] kind: String, /// The app whose credential it is (default: the deployment's)
        #[arg(long)] app: Option<String>, #[command(flatten)] conn: Conn },
    /// Secret names, kinds, versions and what wraps their keys (never a value): the deployment's and every app's, or `--app`'s
    List { #[arg(long)] app: Option<String>, #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum IdentityCmd {
    /// Every app's sign-in client and roles as the identity provider was told of them (admins, operators, auditors)
    Catalog { #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// The events of the client hook, newest first: delivered, waiting (and why) or given up
    Events { #[arg(long)] app: Option<String>, /// queued, done, dead or cancelled
        #[arg(long)] status: Option<String>, #[arg(long, default_value_t = 50)] limit: u32, #[arg(long)] json: bool, #[command(flatten)] conn: Conn },
    /// Delivers a given-up (or waiting) event again, now (admin)
    Retry { id: String, #[command(flatten)] conn: Conn },
    /// Checks a receiver of the client hook against its contract: signatures, freshness, every
    /// event, idempotency, a client created at once or later (202). Talks to the receiver directly, with test data; it
    /// registers a client named `cori-conformance-<random>` and deletes it at the end
    TestHook { url: String, /// The environment variable holding the hook's secret (CORI_AUTH_CLIENT_HOOK_SECRET on the server)
        #[arg(long, default_value = "CORI_AUTH_CLIENT_HOOK_SECRET")] secret_env: String,
        /// How long to wait for a client the receiver answered 202 for (seconds)
        #[arg(long, default_value_t = 120)] wait: u64,
        /// The front origin the test app claims (its redirect URIs follow)
        #[arg(long, default_value = "https://cori-conformance.apps.example.invalid")] origin: String },
    /// Checks a directory of the organization's people (CORI_IDENTITY_PEOPLE_URL) against its contract:
    /// signatures, freshness, `people.list` answered page after page, each person named by id.
    /// Only asks: nothing is changed
    TestPeople { url: String, /// The environment variable holding the directory's secret (CORI_IDENTITY_PEOPLE_SECRET on the server)
        #[arg(long, default_value = "CORI_IDENTITY_PEOPLE_SECRET")] secret_env: String },
    /// Checks an access webhook (CORI_IDENTITY_ACCESS_URL) against its contract: signatures,
    /// freshness, events it does not handle answered 200, `access.granted` and `access.revoked` for nobody refused with a
    /// message. Grants nothing: the person named does not exist
    TestAccess { url: String, /// The environment variable holding the webhook's secret (CORI_IDENTITY_ACCESS_SECRET on the server)
        #[arg(long, default_value = "CORI_IDENTITY_ACCESS_SECRET")] secret_env: String },
}

#[derive(Subcommand)]
pub(crate) enum PoolsCmd {
    /// Create a worker pool (admin); its key is printed once
    Create { name: String, #[command(flatten)] conn: Conn },
    /// Worker pools and when they last polled
    List { #[command(flatten)] conn: Conn },
}

#[derive(Subcommand)]
pub(crate) enum SandboxCmd {
    /// Serve the workers and the server (`CORI_SANDBOX_URL` names it to them), who present CORI_SANDBOX_TOKEN
    Serve { #[arg(long, env = "CORI_SANDBOX_LISTEN", default_value = "127.0.0.1:8090")] listen: String,
        /// The only folder it writes (its home, temporary and cache folders too), emptied between leases; default a folder
        /// of its own in the temporary folder
        #[arg(long, env = "CORI_SANDBOX_WORK_DIR")] work: Option<PathBuf>,
        /// Alone in a container: starting over stops every process of its user (as a sandbox pod runs it)
        #[arg(long)] contained: bool },
}

#[derive(Subcommand)]
pub(crate) enum GenCmd {
    /// cori gen client --dir . --out front/src/lib/cori.ts (from the app folder)   |   cori gen client --app id --out … (the
    /// promoted version, from the server: the app's builders)
    Client { #[arg(long)] dir: Option<PathBuf>, #[arg(long)] app: Option<String>, #[arg(long, default_value = "front/src/lib/cori.ts")] out: PathBuf, #[command(flatten)] conn: Conn },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Front { React, Plain, None }

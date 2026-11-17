// SPDX-License-Identifier: Apache-2.0
//! Cori single-org server: apps, versions (publish + change review), bundles by hash, endpoints, run index, approvals.
pub mod api;
pub mod auth;
pub mod bundles;
pub mod db;
pub mod scheduler;
pub mod secrets;
pub mod stores;
pub mod webhooks;
pub mod client_hook;
pub mod schedules;
pub mod clientgen;
pub mod state;
pub mod mcp;
pub mod identity;
pub mod workers;
pub mod llm;
pub mod module;
pub mod brand;
pub mod names;
pub mod realm;
pub mod usage;
pub mod lifecycle;
pub mod preflight;
pub mod connections;

use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: String,
    pub public_url: String,
    pub database_url: String,
    pub data_dir: PathBuf,
    pub temporal: String,
    pub namespace: String,
    pub task_queue: String,
    /// Sharded shared queue: runs of one app always land on `<task_queue>.<hash(app) % shards>` (1 = a single queue).
    /// Fallback for Temporal servers without task-queue fairness keys: a noisy org saturates one shard, not all.
    pub shards: u32,
    pub auth: auth::AuthMode,
    /// Answer connectors from the bundle's `[connectors.*.dev]` mocks (local `cori up`).
    pub dev_mocks: bool,
    pub sync_timeout_secs: u64,
    /// Delete runs, run events and captured LLM calls older than this many days (0 = keep everything). `CORI_RETENTION_DAYS`.
    pub retention_days: u32,
    /// The largest file a run takes (`CORI_FILES_MAX_BYTES`: bytes, or `25MiB`, `40MB`; 25 MiB by default): an upload reads
    /// at most this much, and a parameter's `max_size` can only lower it.
    pub files_max_bytes: u64,
    /// Roles allowed to approve a change review that adds writes (any of). The deployment's: a realm may have its own
    /// (`realm::RealmSettings`, read as `AppState::settings`), like `allow_self_approval` and `slack_users`.
    pub approver_roles: Vec<String>,
    /// Admin Postgres URL for Cori Store (databases `cori_store_<name>` are created on it): the store cluster named `main`.
    /// When set, store connectors always hit their store and their `[connectors.*.dev] mock` is ignored. None ⇒ stores
    /// unavailable (mocks only).
    pub store_database_url: Option<String>,
    /// Store clusters besides `main` (`CORI_STORE_CLUSTER_<NAME>=<admin URL>`, the name lower-cased): each store database
    /// is created on one and records which (`store_instances.cluster`), so that a deployment adds clusters as it grows.
    pub other_store_clusters: Vec<(String, String)>,
    /// The cluster new store databases go to (`CORI_STORE_PLACEMENT`, default `main`); a realm's settings may name another.
    pub store_placement: String,
    /// Connectors whose dev mock is ignored even when `dev_mocks` is on (e.g. `tickets` to hit the real store).
    pub no_mocks: Vec<String>,
    /// OIDC client id the platform uses for token exchange (`cori-platform` on Keycloak).
    pub auth_client_id: String,
    /// Shared secret workers present on `/v1/ingest/events` (`X-Cori-Worker`). Empty = local mode, no check.
    pub ingest_token: String,
    /// Start on a non-loopback address even with laptop settings (`--allow-insecure`, demo only). See `production_problems`.
    pub allow_insecure: bool,
    /// The URL workers reach this server at (bundles, events, leases, store calls), carried in each run's input:
    /// in a cluster the service address rather than the public ingress. None = `public_url`.
    pub internal_url: Option<String>,
    /// Extra browser origins allowed by CORS besides `public_url`'s (`CORI_CORS_ORIGINS`).
    pub cors_origins: Vec<String>,
    /// The OIDC clients of Cori's own tools, whose tokens reach every route their scopes allow, unlike an app front's: the
    /// CLI (`CORI_AUTH_CLI_CLIENT_ID`, default `cori-cli`) and the console (`CORI_AUTH_WEB_CLIENT_ID`, default `cori-web`).
    pub cli_client_id: String,
    pub web_client_id: String,
    /// The secret of `auth_client_id` for token requests, when the issuer asks for one (`CORI_AUTH_CLIENT_SECRET`).
    pub auth_client_secret: Option<String>,
    /// Scopes every sign-in at the issuer asks for besides its own (`openid`, `offline_access`, and for the command line
    /// and MCP hosts `profile` and `email`): `CORI_AUTH_SCOPES`, space- or comma-separated. An issuer that gives a token
    /// the audience of the scopes asked for rather than of the resource named (RFC 8707), as Entra ID does, needs the scope
    /// of Cori's own API here (`api://cori/access`): the console, the fronts, the command line and MCP hosts ask for it.
    pub auth_scopes: Vec<String>,
    /// The resource indicator (RFC 8707) the console, the fronts and the command line name when they sign in
    /// (`CORI_AUTH_RESOURCE`, `sign_in_resource`): the public URL by default, so that an issuer which honours it gives a
    /// token of Cori's audience; another URI, or `none`, for an issuer that refuses one its scopes do not name (Entra ID).
    pub auth_resource: Option<String>,
    /// The public client MCP hosts sign in with, where the issuer registers none for them on the fly (no open dynamic
    /// client registration): `CORI_AUTH_MCP_CLIENT_ID`, and the fixed loopback port of its callback when its redirect URIs
    /// name one (`CORI_AUTH_MCP_CALLBACK_PORT`). The console's commands for coding agents carry them.
    pub mcp_client_id: Option<String>,
    pub mcp_callback_port: Option<u16>,
    /// When a run starts under builtin auth, ask the issuer for a refresh token (`CORI_AUTH_RUN_REFRESH`, default on), an
    /// offline one for runs that outlive the SSO session (`CORI_AUTH_RUN_OFFLINE`).
    pub run_refresh: bool,
    pub run_offline: bool,
    /// Version sandboxes kept per app, the oldest dropped first (`CORI_SANDBOXES_PER_APP`, default 5).
    pub sandboxes_per_app: usize,
    /// A Chromium binary for version screenshots (`CORI_CHROMIUM`); none, no screenshots.
    pub chromium: Option<String>,
    /// The person who started a run or parked a change may decide its approval (`CORI_ALLOW_SELF_APPROVAL`): a team too
    /// small for four eyes.
    pub allow_self_approval: bool,
    /// Slack interactivity: the Slack app's signing secret (`CORI_SLACK_SIGNING_SECRET`) and the Slack users mapped to
    /// Cori principals (`CORI_SLACK_USERS`: `U0123=sam:support_lead,approver;U0456=jean:it_admin`).
    pub slack_signing_secret: Option<String>,
    pub slack_users: String,
    /// Email webhooks: the SMTP server and the sender (`CORI_SMTP_URL`, `CORI_SMTP_FROM`).
    pub smtp: Option<(String, String)>,
    /// The server's own cron starts every schedule's runs, not Temporal Schedules (`CORI_SCHEDULER=server`).
    pub server_scheduler: bool,
    /// The org's hook that registers an app's OIDC client (`CORI_AUTH_CLIENT_HOOK`, required under builtin auth), and the
    /// secret its deliveries are signed with (`CORI_AUTH_CLIENT_HOOK_SECRET`).
    pub client_hook: Option<String>,
    pub client_hook_secret: Option<String>,
    /// A directory of the organization's own that lists its people (`CORI_IDENTITY_PEOPLE_URL`): it answers `people.list`
    /// in place of the hook's receiver, its requests signed with `CORI_IDENTITY_PEOPLE_SECRET` (`client_hook::people_url`).
    pub people_url: Option<String>,
    pub people_secret: Option<String>,
    /// A webhook of the organization's own that takes the changes to apps' access (`CORI_IDENTITY_ACCESS_URL`):
    /// `access.granted` and `access.revoked` go to it in place of the hook's receiver, signed with
    /// `CORI_IDENTITY_ACCESS_SECRET` (`client_hook::access_url`).
    pub access_url: Option<String>,
    pub access_secret: Option<String>,
    /// What the console says where an app's roles are given (`CORI_IDENTITY_ACCESS_MESSAGE`): how long a change takes to
    /// reach people (an IAM that syncs every few minutes), whom to ask.
    pub access_message: Option<String>,
    /// What the hook tells the organization's IAM about apps' roles, and when (`client_hook::Settings`: `CORI_AUTH_CLIENT_HOOK_ROLES`,
    /// `CORI_ROLES_*`, `CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES`, `CORI_IDENTITY_GIVE_UP_SECS`).
    pub identity: client_hook::Settings,
    /// Where the identity provider grants an app's roles (`CORI_IDENTITY_ADMIN_URL`, `{app}` and `{client_id}` filled in):
    /// the app page links there, for a deployment whose provider grants them outside Cori (none when they are given in
    /// Cori: an access webhook, or a receiver that takes `access.granted`).
    pub identity_admin_url: Option<String>,
    /// Where a person manages their own account at the identity provider (`CORI_AUTH_ACCOUNT_URL`; Keycloak:
    /// `{issuer}/account`): their name, password, sessions. The console's menu links there.
    pub account_url: Option<String>,
    /// The modules the binary was built with (`module`): none in the core's own `cori`.
    pub modules: module::Modules,
    /// The deployment's brand file (`CORI_BRAND_FILE`, see `brand`): its name, logos and colours. None = Cori's.
    pub brand_file: Option<PathBuf>,
    /// What requests to addresses an app chose may reach (its own connectors, its `[llm] base_url`, its webhooks):
    /// `CORI_APP_EGRESS` = `public` (default), `private` or `any`. See `cori_engine::egress`.
    pub app_egress: Option<cori_engine::egress::Egress>,
    /// What the org registry's connectors and LLM providers may reach: `CORI_REGISTRY_EGRESS` = `private` (default: a
    /// company's systems live on its private network), `public` (a server hosting several organizations) or `any`.
    pub registry_egress: Option<cori_engine::egress::Egress>,
    /// A second address serving the same routes, for the workers of this cluster (`CORI_WORKER_LISTEN`, e.g.
    /// `0.0.0.0:8081`): when set, the cluster's worker token (`X-Cori-Worker`) is accepted there only, so the public
    /// address never takes it. Pools outside the cluster keep authenticating with their own keys on the public address.
    pub worker_listen: Option<String>,
    /// Each app's front on a host of its own (`CORI_FRONTS_URL`, e.g. `https://*.cori.run`, a wildcard DNS name and
    /// certificate): `<label>.<domain>` serves that app's front and its runtime API only, and the server's own host no
    /// front. None: every front is served under the server's origin (`/apps/<app>/`), where its code shares the console's
    /// storage and every other front's.
    pub fronts_url: Option<names::FrontsUrl>,
    /// A front's files for the people who may open it only (`CORI_FRONTS_PRIVATE`; with `fronts_url` and builtin auth,
    /// `fronts_gated`): a front host gives a browser its files once it holds a pass of that host, which the console hands
    /// out to whoever may open the app (`api::passes`). Off, the files are anyone's (the data never is: the API wants a
    /// token), so any page a builder deploys is shown to whoever has its link.
    pub fronts_private: bool,
    /// The host of the organization's apps portal, the page where its people find the apps they may use (`CORI_PORTAL_URL`,
    /// e.g. `https://apps.acme.internal`: an origin of its own, with its DNS name and certificate). None: no portal, unless
    /// a tenancy serves one per organization under the fronts' domain.
    pub portal_url: Option<String>,
    /// Who may frame a front besides itself (`CORI_FRAME_ANCESTORS`, space-separated CSP sources: the MCP hosts whose
    /// preview widget nests it). The console is never framed.
    pub frame_ancestors: Vec<String>,
    /// The page a release's host sends its sign-in to when it is framed (`CORI_PREVIEW_SIGN_IN_URL`, e.g. the page
    /// `https://tools.acme.internal/cori/sign-in` of a tool that frames releases), instead of the console, which is never
    /// framed: that page asks for the code as the person (`preview-codes`, `pass-codes`) and sends the frame back to the
    /// release's callback. None: the console, framed or not.
    pub preview_sign_in_url: Option<String>,
    /// The runs a realm may have under way at once, those waiting for a person aside (`CORI_MAX_ACTIVE_RUNS`; 0, no
    /// limit). The deployment's: a realm may have its own (`realm::RealmSettings`).
    pub max_active_runs: usize,
    /// What a realm may use (`CORI_MAX_*`, `realm::Limits`). The deployment's: a realm may have its own.
    pub limits: realm::Limits,
    /// The oldest `cori` command line this server serves (`CORI_MIN_CLI_VERSION`, default `MIN_CLI_VERSION`). Every API
    /// answer names it (`x-cori-cli-min`), and an older command line says to upgrade: a server moves on, the command lines
    /// people installed do not.
    pub min_cli: String,
    /// The command that installs the deployment's plugin for coding agents, its MCP server and skill in one go
    /// (`CORI_AGENT_PLUGIN`): the console's Connect your agent page shows it before each agent's own commands.
    pub agent_plugin: Option<String>,
    /// The model the deployment offers every realm for the tiers it set no default for (`CORI_LLM_STARTER_*`, see
    /// `llm::Starter`): rate-limited, at no cost to the realm, until its own providers replace it.
    pub llm_starter: Option<llm::Starter>,
}

/// The oldest command line the server's API serves. Raise it with a change older command lines cannot follow: a
/// control-plane route they call renamed or removed, or an answer they read reshaped.
pub const MIN_CLI_VERSION: &str = "0.2.0";

/// A yes/no setting: `1`, `true`, `yes`, `on` or `0`, `false`, `no`, `off` (any case); unset or anything else is `default`.
pub fn env_flag(var: &str, default: bool) -> bool {
    match std::env::var(var).map(|v| v.trim().to_ascii_lowercase()) {
        Ok(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => true,
        Ok(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

/// A setting's value, unset when empty.
fn env_str(var: &str) -> Option<String> { std::env::var(var).ok().filter(|v| !v.trim().is_empty()) }

/// OAuth scopes as a setting lists them, separated by spaces or commas, each once.
pub fn scope_list(v: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()) { if !out.iter().any(|o| o == s) { out.push(s.to_string()); } }
    out
}

/// The scopes of a sign-in: `base`, then the deployment's own (`ServerConfig::auth_scopes`) not already in it.
pub fn sign_in_scopes(base: &[&str], extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    for s in extra { if !out.contains(s) { out.push(s.clone()); } }
    out
}

/// The largest file a run takes when `CORI_FILES_MAX_BYTES` says nothing.
pub const FILES_MAX_DEFAULT: u64 = 25 * 1024 * 1024;

impl ServerConfig {

    pub fn local(data_dir: PathBuf) -> Self {
        Self { listen: "127.0.0.1:8080".into(), public_url: "http://127.0.0.1:8080".into(), database_url: std::env::var("CORI_DATABASE_URL").unwrap_or_default(), data_dir, temporal: "http://127.0.0.1:7233".into(), namespace: "default".into(), task_queue: env_str("CORI_TASK_QUEUE").unwrap_or_else(|| "cori.shared".into()), shards: std::env::var("CORI_SHARDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1), auth: auth::AuthMode::Dev, dev_mocks: true, sync_timeout_secs: 30, retention_days: std::env::var("CORI_RETENTION_DAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(0), files_max_bytes: env_str("CORI_FILES_MAX_BYTES").and_then(|v| cori_compiler::files::parse_size(&serde_json::Value::String(v)).ok()).unwrap_or(FILES_MAX_DEFAULT), approver_roles: vec!["it_admin".into(), "org_admin".into(), "approver".into()], ingest_token: String::new(), store_database_url: std::env::var("CORI_STORE_DATABASE_URL").ok(), no_mocks: vec![], auth_client_id: std::env::var("CORI_AUTH_CLIENT_ID").unwrap_or_else(|_| "cori-platform".into()), allow_insecure: false, internal_url: None, cors_origins: std::env::var("CORI_CORS_ORIGINS").map(|v| v.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect()).unwrap_or_default(),
            cli_client_id: env_str("CORI_AUTH_CLI_CLIENT_ID").unwrap_or_else(|| "cori-cli".into()),
            web_client_id: env_str("CORI_AUTH_WEB_CLIENT_ID").unwrap_or_else(|| "cori-web".into()),
            auth_client_secret: env_str("CORI_AUTH_CLIENT_SECRET"),
            auth_scopes: env_str("CORI_AUTH_SCOPES").map(|v| scope_list(&v)).unwrap_or_default(),
            auth_resource: env_str("CORI_AUTH_RESOURCE").map(|v| v.trim().to_string()),
            mcp_client_id: env_str("CORI_AUTH_MCP_CLIENT_ID").map(|v| v.trim().to_string()),
            mcp_callback_port: env_str("CORI_AUTH_MCP_CALLBACK_PORT").and_then(|v| v.trim().parse().ok()).filter(|p: &u16| *p > 0),
            run_refresh: env_flag("CORI_AUTH_RUN_REFRESH", true),
            run_offline: env_flag("CORI_AUTH_RUN_OFFLINE", false),
            sandboxes_per_app: env_str("CORI_SANDBOXES_PER_APP").and_then(|v| v.parse().ok()).unwrap_or(5),
            chromium: env_str("CORI_CHROMIUM"),
            allow_self_approval: env_flag("CORI_ALLOW_SELF_APPROVAL", false),
            slack_signing_secret: env_str("CORI_SLACK_SIGNING_SECRET"),
            slack_users: env_str("CORI_SLACK_USERS").unwrap_or_default(),
            smtp: env_str("CORI_SMTP_URL").zip(env_str("CORI_SMTP_FROM")),
            server_scheduler: std::env::var("CORI_SCHEDULER").map(|v| v.trim() == "server").unwrap_or(false),
            client_hook: env_str("CORI_AUTH_CLIENT_HOOK"),
            client_hook_secret: env_str("CORI_AUTH_CLIENT_HOOK_SECRET"),
            people_url: env_str("CORI_IDENTITY_PEOPLE_URL").map(|v| v.trim().to_string()),
            people_secret: env_str("CORI_IDENTITY_PEOPLE_SECRET"),
            access_url: env_str("CORI_IDENTITY_ACCESS_URL").map(|v| v.trim().to_string()),
            access_secret: env_str("CORI_IDENTITY_ACCESS_SECRET"),
            access_message: env_str("CORI_IDENTITY_ACCESS_MESSAGE").map(|v| v.trim().chars().filter(|c| !c.is_control()).take(500).collect::<String>()).filter(|m| !m.is_empty()),
            identity: client_hook::Settings::from_env(),
            identity_admin_url: env_str("CORI_IDENTITY_ADMIN_URL").map(|v| v.trim().to_string()),
            account_url: env_str("CORI_AUTH_ACCOUNT_URL").map(|v| v.trim().to_string()),
            modules: module::Modules::default(),
            brand_file: env_str("CORI_BRAND_FILE").map(PathBuf::from),
            app_egress: env_str("CORI_APP_EGRESS").and_then(|v| cori_engine::egress::Egress::parse(&v)),
            registry_egress: env_str("CORI_REGISTRY_EGRESS").and_then(|v| cori_engine::egress::Egress::parse(&v)),
            worker_listen: env_str("CORI_WORKER_LISTEN"),
            fronts_url: env_str("CORI_FRONTS_URL").and_then(|v| names::FrontsUrl::parse(&v).ok()),
            fronts_private: env_flag("CORI_FRONTS_PRIVATE", false),
            portal_url: env_str("CORI_PORTAL_URL").and_then(|v| names::portal_origin(&v).ok()),
            frame_ancestors: env_str("CORI_FRAME_ANCESTORS").map(|v| v.split_whitespace().map(String::from).collect()).unwrap_or_default(),
            preview_sign_in_url: env_str("CORI_PREVIEW_SIGN_IN_URL").and_then(|v| names::sign_in_page(&v).ok()),
            max_active_runs: env_str("CORI_MAX_ACTIVE_RUNS").and_then(|v| v.parse().ok()).unwrap_or(0),
            limits: realm::Limits::from_env(),
            other_store_clusters: other_store_clusters_from_env(),
            store_placement: env_str("CORI_STORE_PLACEMENT").map(|v| v.trim().to_ascii_lowercase()).unwrap_or_else(|| MAIN_STORE_CLUSTER.into()),
            min_cli: env_str("CORI_MIN_CLI_VERSION").map(|v| v.trim().to_string()).unwrap_or_else(|| MIN_CLI_VERSION.into()),
            agent_plugin: env_str("CORI_AGENT_PLUGIN").map(|v| v.trim().to_string()),
            llm_starter: llm::Starter::from_env(),
        }
    }
}

/// The store cluster of `store_database_url`.
pub const MAIN_STORE_CLUSTER: &str = "main";

/// `CORI_STORE_CLUSTER_<NAME>=<admin URL>`: the store clusters besides `main`, by name (lower-cased), sorted.
fn other_store_clusters_from_env() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::env::vars()
        .filter_map(|(k, v)| Some((k.strip_prefix("CORI_STORE_CLUSTER_")?.to_ascii_lowercase(), v.trim().to_string())))
        .filter(|(_, v)| !v.is_empty()).collect();
    out.sort();
    out
}

/// Why `name` cannot name a store cluster: 1 to 32 lowercase letters, digits and `_`.
pub fn store_cluster_problem(name: &str) -> Option<String> {
    let ok = (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    (!ok).then(|| format!("store cluster `{name}`: 1 to 32 lowercase letters, digits and `_`"))
}

impl ServerConfig {
    /// The laptop modes (`dev` and `off` auth): the machine and its network are the builder's own.
    pub fn laptop_auth(&self) -> bool { matches!(self.auth, auth::AuthMode::Dev | auth::AuthMode::Off) }

    /// Off the laptop modes, an app's own connector declarations run nothing on a worker and pin nothing to a pool
    /// (`AppConfig::merge_registry_hosted`).
    pub fn strict_connectors(&self) -> bool { !self.laptop_auth() }

    /// Whether front hosts give their files to the people who may open them only (`fronts_private`): it takes fronts on
    /// hosts of their own, where a pass is a cookie of one host, and an identity provider the console signs people in with.
    pub fn fronts_gated(&self) -> bool { self.fronts_private && self.fronts_url.is_some() && matches!(self.auth, auth::AuthMode::Builtin { .. }) }

    /// What addresses an app chose may reach: `CORI_APP_EGRESS`, else `any` in the laptop modes and `public` otherwise.
    pub fn app_egress(&self) -> cori_engine::egress::Egress {
        self.app_egress.unwrap_or(if self.laptop_auth() { cori_engine::egress::Egress::Any } else { cori_engine::egress::Egress::Public })
    }

    /// What the org registry's connectors and providers may reach: `CORI_REGISTRY_EGRESS`, else `any` in the laptop modes
    /// and `private` otherwise.
    pub fn registry_egress(&self) -> cori_engine::egress::Egress {
        self.registry_egress.unwrap_or(if self.laptop_auth() { cori_engine::egress::Egress::Any } else { cori_engine::egress::Egress::Private })
    }

    /// Every store cluster by name, its admin URL: `main` (`store_database_url`) and the others. Empty without stores.
    pub fn store_clusters(&self) -> std::collections::BTreeMap<String, String> {
        let Some(main) = &self.store_database_url else { return Default::default() };
        std::iter::once((MAIN_STORE_CLUSTER.to_string(), main.clone())).chain(self.other_store_clusters.iter().cloned()).collect()
    }

    /// Why the store clusters cannot be used as configured: a bad name, `main` named twice, clusters without `main`, or a
    /// placement no cluster answers to.
    pub fn store_clusters_problem(&self) -> Option<String> {
        if self.store_database_url.is_none() {
            return (!self.other_store_clusters.is_empty()).then(|| "CORI_STORE_CLUSTER_* is set without CORI_STORE_DATABASE_URL (the `main` store cluster)".to_string());
        }
        for (name, _) in &self.other_store_clusters {
            if let Some(p) = store_cluster_problem(name) { return Some(format!("CORI_STORE_CLUSTER_{}: {p}", name.to_ascii_uppercase())); }
            if name == MAIN_STORE_CLUSTER { return Some("CORI_STORE_CLUSTER_MAIN: the `main` store cluster is CORI_STORE_DATABASE_URL".into()); }
        }
        (!self.store_clusters().contains_key(&self.store_placement)).then(|| format!("CORI_STORE_PLACEMENT=`{}` names no store cluster (`main`, or one of CORI_STORE_CLUSTER_<NAME>)", self.store_placement))
    }

    /// The resource Cori's own sign-ins name at the issuer (`auth_resource`): the public URL unless the setting names
    /// another, or none (`none`).
    pub fn sign_in_resource(&self) -> Option<String> {
        match self.auth_resource.as_deref() {
            None => Some(self.public_url.trim_end_matches('/').to_string()),
            Some(r) if r.eq_ignore_ascii_case("none") => None,
            Some(r) => Some(r.to_string()),
        }
    }

    /// The URL written into runs for workers to call back (`internal_url`, else `public_url`).
    pub fn worker_url(&self) -> String { self.internal_url.clone().filter(|u| !u.is_empty()).unwrap_or_else(|| self.public_url.clone()) }

    /// Whether the server only listens on this machine (`127.0.0.1`, `::1`, `localhost`): the laptop settings are fine there.
    pub fn is_loopback(&self) -> bool {
        let host = self.listen.rsplit_once(':').map(|(h, _)| h).unwrap_or(&self.listen).trim_start_matches('[').trim_end_matches(']');
        host == "localhost" || host.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
    }

    /// The laptop settings that must not reach a network: every one is a way in or a way to lose data.
    pub fn production_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        match &self.auth {
            auth::AuthMode::Dev => out.push("auth is `dev`: anyone can claim any identity with a header or a `dev:` token. Use --auth builtin (CORI_AUTH=builtin, CORI_AUTH_ISSUER)".into()),
            auth::AuthMode::Off => out.push("auth is `off`: every caller is an anonymous administrator. Use --auth builtin (CORI_AUTH=builtin, CORI_AUTH_ISSUER)".into()),
            auth::AuthMode::Builtin { audience: None, .. } => out.push("CORI_AUTH_AUDIENCE is not set: any token the issuer signs, for any client, would be accepted".into()),
            auth::AuthMode::Builtin { .. } => {}
        }
        if self.dev_mocks { out.push("--dev-mocks (CORI_DEV_MOCKS) answers connectors from fixtures: turn it off".into()); }
        if std::env::var("CORI_MASTER_KEY").map(|k| k.trim().is_empty()).unwrap_or(true) {
            out.push("CORI_MASTER_KEY is not set: a key generated into the data dir encrypts every secret and signs agent tokens, and is lost with the volume (generate one with `openssl rand -hex 32`)".into());
        }
        if self.ingest_token.is_empty() { out.push("CORI_INGEST_TOKEN is empty: worker endpoints (events, leases, store calls as a run's user) accept anyone".into()); }
        if !self.public_url.starts_with("https://") { out.push(format!("the public URL `{}` is not https (--public-url / CORI_PUBLIC_URL)", self.public_url)); }
        if self.fronts_url.as_ref().is_some_and(|f| !f.is_https()) { out.push("CORI_FRONTS_URL is not https: fronts' tokens would travel in clear".into()); }
        if self.portal_url.as_deref().is_some_and(|p| !p.starts_with("https://")) { out.push("CORI_PORTAL_URL is not https: the portal's sessions would travel in clear".into()); }
        if self.preview_sign_in_url.as_deref().is_some_and(|p| !p.starts_with("https://")) { out.push("CORI_PREVIEW_SIGN_IN_URL is not https: releases' sign-in codes would travel in clear".into()); }
        for (var, e) in [("CORI_APP_EGRESS", self.app_egress()), ("CORI_REGISTRY_EGRESS", self.registry_egress())] {
            if e == cori_engine::egress::Egress::Any { out.push(format!("{var}=any: requests to addresses builders and admins chose may reach this machine and the cloud's metadata endpoint (use `public` or `private`)")); }
        }
        if let Some(s) = cori_engine::sandbox::SandboxConfig::from_env() && s.token.is_empty() {
            out.push("CORI_SANDBOX_URL is set and CORI_SANDBOX_TOKEN is not: the sandboxes would answer anyone who reaches them".into());
        }
        out.extend(self.modules.iter().flat_map(|m| m.production_problems()));
        out
    }
}

pub async fn serve(cfg: ServerConfig) -> Result<()> {
    if !cfg.is_loopback() {
        let problems = cfg.production_problems();
        if !problems.is_empty() {
            let list = problems.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n");
            if !cfg.allow_insecure {
                anyhow::bail!("refusing to listen on {} with laptop settings:\n{list}\n(--allow-insecure / CORI_ALLOW_INSECURE=true starts anyway, for a demo only)", cfg.listen);
            }
            tracing::warn!("listening on {} with insecure settings (--allow-insecure):\n{list}", cfg.listen);
        }
    }
    if matches!(cfg.auth, auth::AuthMode::Builtin { .. }) && cfg.client_hook.is_none() {
        anyhow::bail!("--auth builtin: set CORI_AUTH_CLIENT_HOOK (every app gets its own OIDC client from the org's client hook; a small service of yours against the issuer's admin API)");
    }
    // the people's directory: a URL that does not parse would leave the console without its people, and nobody compared
    if let Some(u) = &cfg.people_url {
        reqwest::Url::parse(u).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some()).ok_or_else(|| anyhow::anyhow!("CORI_IDENTITY_PEOPLE_URL `{u}`: expected the http(s) URL of your directory's webhook"))?;
        if !matches!(cfg.auth, auth::AuthMode::Builtin { .. }) { tracing::warn!("CORI_IDENTITY_PEOPLE_URL is set, and auth is not builtin: the people are those Cori sees"); }
    }
    // the access webhook: a URL that does not parse would leave every grant queued, never delivered
    if let Some(u) = &cfg.access_url {
        reqwest::Url::parse(u).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some()).ok_or_else(|| anyhow::anyhow!("CORI_IDENTITY_ACCESS_URL `{u}`: expected the http(s) URL of your access webhook"))?;
        if !matches!(cfg.auth, auth::AuthMode::Builtin { .. }) { tracing::warn!("CORI_IDENTITY_ACCESS_URL is set, and auth is not builtin: apps' roles are not given through Cori"); }
        if cfg.access_secret.is_none() { tracing::warn!("CORI_IDENTITY_ACCESS_URL is set without CORI_IDENTITY_ACCESS_SECRET: its deliveries are unsigned, and anyone who reaches it could grant roles"); }
    }
    // where tokens carry app roles: a setting that does not parse would leave everyone without their roles in every app
    if matches!(cfg.auth, auth::AuthMode::Builtin { .. }) { auth::AppRolesClaim::from_env().map_err(|e| anyhow::anyhow!("{e}"))?; }
    if env_str("CORI_AUTH_MCP_CALLBACK_PORT").is_some() && cfg.mcp_callback_port.is_none() { anyhow::bail!("CORI_AUTH_MCP_CALLBACK_PORT: a port number (1 to 65535)"); }
    if let Some(p) = cfg.store_clusters_problem() { anyhow::bail!("{p}"); }
    // a fronts URL that does not parse would leave every front on the console's origin: refuse it rather than ignore it
    if let Some(v) = env_str("CORI_FRONTS_URL") { names::FrontsUrl::parse(&v).map_err(|e| anyhow::anyhow!("CORI_FRONTS_URL: {e}"))?; }
    if let Some(v) = env_str("CORI_PORTAL_URL") { names::portal_origin(&v).map_err(|e| anyhow::anyhow!("CORI_PORTAL_URL: {e}"))?; }
    if let Some(v) = env_str("CORI_PREVIEW_SIGN_IN_URL") { names::sign_in_page(&v).map_err(|e| anyhow::anyhow!("CORI_PREVIEW_SIGN_IN_URL: {e}"))?; }
    if cfg.preview_sign_in_url.is_some() && (cfg.fronts_url.is_none() || !matches!(cfg.auth, auth::AuthMode::Builtin { .. })) {
        tracing::warn!("CORI_PREVIEW_SIGN_IN_URL is set, and releases sign in as before: it takes CORI_FRONTS_URL and builtin auth");
    }
    if let Some(s) = cori_engine::sandbox::SandboxConfig::from_env() {
        for u in &s.urls { reqwest::Url::parse(u).ok().filter(|u| u.host_str().is_some()).ok_or_else(|| anyhow::anyhow!("CORI_SANDBOX_URL `{u}`: expected http://<host>:<port> (a headless service: every address of the host a sandbox), several separated by commas"))?; }
    } else if !cfg.is_loopback() {
        tracing::warn!("CORI_SANDBOX_URL is not set: builders' code (step modules, cli programs, stdio MCP servers, the deploy checks) runs on the workers and this server, next to their credentials");
    }
    if cfg.fronts_url.is_none() && !cfg.is_loopback() {
        tracing::warn!("CORI_FRONTS_URL is not set: every app's front is served on this server's origin, where its code can read the console's and the other fronts' tokens");
    }
    if cfg.fronts_private && !cfg.fronts_gated() {
        tracing::warn!("CORI_FRONTS_PRIVATE is set, and fronts' files stay anyone's: it takes CORI_FRONTS_URL and builtin auth");
    }
    std::fs::create_dir_all(&cfg.data_dir)?;
    let st = state::AppState::new(cfg.clone()).await?;
    // the deployment's LLM providers and tier defaults, when a file declares them
    llm::apply_providers_file(&st).await?;
    // the deployment's connectors' tools, read now and every few hours (every replica: each answers its own discovery)
    tokio::spawn(connections::capture_tools(st.clone()));
    // the background work (schedule sync, live stores, the scheduler's loops) runs on one replica: the leader
    tokio::spawn(scheduler::run(st.clone()));
    let app = api::router(st.clone());
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, modules = ?cfg.modules, "cori server listening");
    // Ctrl+C or SIGTERM stops accepting and lets in-flight requests finish; a second Ctrl+C is handled by the CLI.
    let public = axum::serve(listener, app).with_graceful_shutdown(async { cori_engine::runtime::shutdown_signal().await; tracing::info!("shutting down"); });
    let Some(addr) = cfg.worker_listen.clone() else {
        if !cfg.is_loopback() && !cfg.ingest_token.is_empty() { tracing::warn!("CORI_WORKER_LISTEN is not set: the cluster's worker token is accepted on the public address too"); }
        public.await?;
        return Ok(());
    };
    // the workers' address: the same routes, marked so that the cluster's worker token is accepted here only
    let internal = api::router(st).layer(axum::Extension(workers::InternalListener));
    let worker_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(worker_listen = %addr, "cori server listening for this cluster's workers");
    let workers = axum::serve(worker_listener, internal).with_graceful_shutdown(async { cori_engine::runtime::shutdown_signal().await; });
    tokio::try_join!(async { public.await }, async { workers.await })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(listen: &str) -> ServerConfig { let mut c = ServerConfig::local(std::env::temp_dir()); c.listen = listen.into(); c }

    #[test]
    fn loopback_detection() {
        assert!(cfg("127.0.0.1:8080").is_loopback());
        assert!(cfg("localhost:8080").is_loopback());
        assert!(cfg("[::1]:8080").is_loopback());
        assert!(!cfg("0.0.0.0:8080").is_loopback());
        assert!(!cfg("[::]:8080").is_loopback());
        assert!(!cfg("10.0.0.4:8080").is_loopback());
    }

    #[test]
    fn laptop_settings_are_reported() {
        let p = cfg("0.0.0.0:8080").production_problems();
        for needle in ["auth is `dev`", "dev-mocks", "CORI_INGEST_TOKEN", "not https"] { assert!(p.iter().any(|x| x.contains(needle)), "{needle}: {p:?}"); }
        let mut c = cfg("0.0.0.0:8080");
        c.auth = auth::AuthMode::Builtin { issuer: "https://sso.example/realms/x".into(), audience: Some("cori-platform".into()), roles_claim: "roles".into(), subject_claim: "sub".into() };
        c.database_url = "postgres://cori@db/cori".into(); c.ingest_token = "t".into(); c.dev_mocks = false; c.public_url = "https://cori.example".into();
        let p = c.production_problems();
        assert!(p.iter().all(|x| x.contains("CORI_MASTER_KEY")), "{p:?}");
        // a module's own settings count too
        struct Keys;
        impl module::Module for Keys { fn id(&self) -> &'static str { "keys" } fn production_problems(&self) -> Vec<String> { vec!["keys are local".into()] } }
        c.modules = module::Modules(vec![std::sync::Arc::new(Keys)]);
        assert!(c.production_problems().iter().any(|x| x == "keys are local"));
    }
}

// SPDX-License-Identifier: Apache-2.0
//! App configuration (`cori.toml`) as the engine needs it: connectors and LLM settings, with dev mocks.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DevMock {
    #[serde(default)]
    pub mock: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConfig {
    /// Empty when the app block only overrides an org connector (e.g. `[connectors.x.dev] mock = …`): the registry fills the rest.
    #[serde(default)]
    pub kind: String,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth_env: Option<String>,
    /// Name of a broker secret holding the bearer token (hosted mode); `auth_env` is the local fallback.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub task_queue: Option<String>,
    /// MCP over HTTP (streamable): the server URL.
    #[serde(default)]
    pub url: Option<String>,
    /// MCP over stdio: the command to spawn (argv).
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub dev: Option<DevMock>,
    /// The deployment's connectors only: the OAuth provider whose access token each call carries, a connection's (the
    /// run's person's, or the app's: `account`), and the scopes its tools need. An app's own declaration and an admin's
    /// never have one (`merge_registry_hosted`, the server's registry): the token would go wherever they point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthUse>,
    /// Whose connection an OAuth connector calls with (cori.toml): the run's person (`person`, the default) or the app's
    /// (`app`), which its owner connects: for runs with no person (schedules), or an account the app's people share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Hosted runs: the org registry declared this connector (`merge_registry_hosted`), so what it reaches follows the
    /// server's registry egress rather than its app egress.
    #[serde(skip)]
    pub registry: bool,
    /// Hosted runs: why this connector, which only the app declares, cannot run here (`merge_registry_hosted`, strict).
    #[serde(skip)]
    pub refused: Option<String>,
}

/// An OAuth connector's provider (the deployment's: `google`) and the scopes its tools need.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OAuthUse { pub provider: String, #[serde(default)] pub scopes: Vec<String> }

/// What `merge_registry_hosted` took from the connectors an app declares, for the deploy's warnings.
#[derive(Debug, Default, PartialEq)]
pub struct HostedOverrides {
    /// App-only connectors whose credential (`secret`, `audience`, `auth_env`, `oauth`) is ignored.
    pub credentials: Vec<String>,
    /// App-only connectors that would run something on a worker (`cli`, an MCP server over stdio, a native activity
    /// queue): refused on a strict server, with the reason (`ConnectorConfig::refused`).
    pub refused: Vec<(String, String)>,
    /// Connectors whose `worker` pin in cori.toml is ignored: where a connector runs is the org registry's decision.
    pub pins: Vec<String>,
}

/// An LLM provider: declared by the app (`[llm.providers.<name>]`, key from `api_key_env` on the worker) or by the org in the
/// console (`kind`, `models` with prices, key in the credential broker: `secret`, leased per call, never in the run input).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderCfg { #[serde(default)] pub base_url: Option<String>, #[serde(default)] pub api_key_env: Option<String>,
    /// `openai` (any OpenAI-compatible API: OpenAI, vLLM, Mistral…) or `anthropic`; defaults to the provider's name
    #[serde(default)] pub kind: Option<String>,
    /// broker secret holding the key (org providers only: an app's cori.toml cannot name one, `resolve_llm` drops it)
    #[serde(default)] pub secret: Option<String>,
    /// model id → price, for the cost of each call (and so the app's monthly LLM budget)
    #[serde(default)] pub models: BTreeMap<String, ModelPrice> }

/// Prices in euros per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelPrice { #[serde(default)] pub input_eur_per_mtok: f64, #[serde(default)] pub output_eur_per_mtok: f64 }

/// A model bound to a step (or to every step of a tier): `mode` is `primary`, or a mode a module's `llm` step hook reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding { pub provider: String, pub model: String, pub mode: String }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub mock: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub tiers: BTreeMap<String, String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderCfg>,
    /// `[llm.dev] provider = "mock", mock = "fixtures/llm.json"`: what `cori run` and `cori up` use instead of the
    /// `[llm]` provider, like `[connectors.<x>.dev]` does for a connector. Dropped in production (`without_mocks`).
    #[serde(default)]
    pub dev: Option<LlmDev>,
}
fn default_provider() -> String { "mock".into() }
impl LlmConfig {
    /// The app chose its provider (anything but the implicit `mock` default, or no `[llm]` table at all).
    fn explicit_provider(&self) -> bool { !(self.provider.is_empty() || self.provider == "mock") || self.mock.is_some() }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmDev { #[serde(default)] pub provider: Option<String>, #[serde(default)] pub mock: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub app: BTreeMap<String, Value>,
    #[serde(default)]
    pub auth: BTreeMap<String, Value>,
    #[serde(default)]
    pub connectors: BTreeMap<String, ConnectorConfig>,
    #[serde(default)]
    pub llm: LlmConfig,
    /// Outgoing webhooks: `[[webhooks]] url, events = ["run.completed", "run.failed", "approval.pending"], secret = "<broker name>" | secret_env`
    #[serde(default)]
    pub webhooks: Vec<WebhookConfig>,
    #[serde(skip)]
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig { pub url: String, #[serde(default = "default_webhook_events")] pub events: Vec<String>, #[serde(default)] pub secret: Option<String>, #[serde(default)] pub secret_env: Option<String>,
    /// `json` (default: the signed Cori envelope) or `slack` (an incoming-webhook message with a link to the console)
    #[serde(default = "default_webhook_format")] pub format: String,
    /// Console base URL used in Slack and email messages (default: the server's public URL)
    #[serde(default)] pub console_url: Option<String>,
    /// `format = "email"`: recipients; the server's `CORI_SMTP_URL` (smtp://user:pass@host:port) and `CORI_SMTP_FROM` send it
    #[serde(default)] pub to: Vec<String> }
fn default_webhook_format() -> String { "json".into() }
fn default_webhook_events() -> Vec<String> { vec!["run.completed".into(), "run.failed".into(), "approval.pending".into()] }

impl AppConfig {
    pub fn from_toml(text: &str, root: &Path) -> anyhow::Result<Self> {
        let mut cfg: AppConfig = toml::from_str(text)?;
        cfg.root = root.display().to_string();
        Ok(cfg)
    }
    /// The token claims a run's user (`ctx.user`) carries besides its subject, roles and email: `[auth] principal_claims`.
    pub fn principal_claims(&self) -> Vec<String> {
        self.auth.get("principal_claims").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    }
    /// Drop every `[connectors.*.dev]` mock and `[llm.dev]` (production mode).
    pub fn without_mocks(mut self) -> Self { for c in self.connectors.values_mut() { c.dev = None; } self.llm.dev = None; if self.llm.provider == "mock" { self.llm.mock = None; } self }

    /// Force the mock LLM provider for a local run (`cori run --mock-llm [file]`).
    pub fn mock_llm(&mut self, file: Option<&str>) {
        self.llm.provider = "mock".into();
        if let Some(f) = file { self.llm.mock = Some(f.to_string()); } else if self.llm.mock.is_none() { self.llm.mock = self.llm.dev.as_ref().and_then(|d| d.mock.clone()).or(Some("fixtures/llm.json".into())); }
    }

    pub fn load(app_dir: &Path) -> anyhow::Result<Self> {
        let path = app_dir.join("cori.toml");
        let mut cfg: AppConfig = if path.exists() { toml::from_str(&std::fs::read_to_string(&path)?)? } else { AppConfig::default() };
        cfg.root = app_dir.display().to_string();
        Ok(cfg)
    }

    /// Hosted runs (the server): the org registry is the authority on what a connector reaches and with which credential.
    /// A connector the registry declares is the registry's, whole (the app keeps its dev mock); one only the app declares
    /// keeps its transport but holds no credential (`secret`, `audience`, `auth_env`, `oauth`). Otherwise an app could lease
    /// any broker secret, its users' tokens exchanged for any audience or their connections' tokens, and send them to a
    /// host of its choosing. Of a registry connector's block, the app keeps its dev mock and whose connection it calls with
    /// (`account`).
    ///
    /// `strict` (every server but the laptop modes) also keeps an app's own declarations from running anything on a
    /// worker: a connector only the app declares loses its `command` and `task_queue`, so a `cli` program, an MCP server
    /// over stdio or a native activity queue of its own is refused (with the fix) unless its dev mock answers instead (a
    /// server with `--dev-mocks`; drop the mocks first, as production does, to see what runs without them), and no app
    /// pins a connector to a worker pool. Otherwise any builder could start `sh` next to the workers' credentials. Off a
    /// strict server the app keeps its pins and its programs: the laptop is the builder's own.
    /// `cori run` keeps `merge_registry`: a local run's credentials are the developer's own.
    pub fn merge_registry_hosted(&mut self, registry: &BTreeMap<String, ConnectorConfig>, strict: bool) -> HostedOverrides {
        let mut o = HostedOverrides::default();
        for (name, app) in self.connectors.iter_mut() {
            match registry.get(name) {
                Some(reg) => {
                    let dev = app.dev.take(); let worker = app.worker.take(); let account = app.account.take();
                    *app = reg.clone(); app.registry = true;
                    if dev.is_some() { app.dev = dev; }
                    // whose connection an OAuth connector calls with is the app's choice
                    if account.is_some() { app.account = account; }
                    if worker.is_some() && worker != app.worker {
                        if strict { o.pins.push(name.clone()); } else { app.worker = worker; }
                    }
                }
                None => {
                    if app.kind != "store" && (app.secret.is_some() || app.audience.is_some() || app.auth_env.is_some() || app.oauth.is_some()) { o.credentials.push(name.clone()); }
                    app.secret = None; app.audience = None; app.auth_env = None; app.oauth = None;
                    if !strict { continue; }
                    if app.worker.take().is_some() { o.pins.push(name.clone()); }
                    let runs = match app.kind.as_str() {
                        "cli" => Some("runs a program (`cli`)"),
                        "activity" => Some("sends to a native activity queue"),
                        "mcp" if app.url.is_none() && app.command.is_some() => Some("starts an MCP server over stdio"),
                        _ => None,
                    };
                    // nothing of the app's runs on a worker any more; its dev mock, when the server answers from mocks
                    // (a demo's `--dev-mocks`), still does, and without one the connector cannot be used here
                    app.command = None; app.task_queue = None;
                    if let (Some(what), None) = (runs, &app.dev) {
                        let why = format!("connector `{name}` {what}, which only a connector of the org registry may do on this server: ask an admin to declare it (`cori connectors add --name {name} …`), or request it (`request_connector`)");
                        o.refused.push((name.clone(), why.clone()));
                        app.refused = Some(why);
                    }
                }
            }
        }
        for (name, reg) in registry { self.connectors.entry(name.clone()).or_insert_with(|| ConnectorConfig { registry: true, ..reg.clone() }); }
        o
    }

    /// Fills every connector the app does not fully declare from the org registry (`GET /v1/connectors` rows or
    /// their `config`), keeping the app's own `dev` mock and worker pin as overrides (local runs; hosted runs use
    /// `merge_registry_hosted`).
    pub fn merge_registry(&mut self, registry: &BTreeMap<String, ConnectorConfig>) {
        for (name, reg) in registry {
            match self.connectors.get_mut(name) {
                None => { self.connectors.insert(name.clone(), reg.clone()); }
                Some(app) if app.kind.is_empty() => { let dev = app.dev.take(); let worker = app.worker.take(); let account = app.account.take(); *app = reg.clone(); if dev.is_some() { app.dev = dev; } if worker.is_some() { app.worker = worker; } if account.is_some() { app.account = account; } }
                Some(_) => {}
            }
        }
    }

    /// Resolved view of one connector for an activity: mock content loaded, secrets left as env names.
    pub fn resolve_connector(&self, name: &str) -> anyhow::Result<ResolvedConnector> {
        let c = self.connectors.get(name).ok_or_else(|| anyhow::anyhow!("connector `{name}` is not declared in cori.toml nor in the org registry"))?;
        if let Some(why) = &c.refused { anyhow::bail!("{why}"); }
        if c.kind.is_empty() { anyhow::bail!("connector `{name}` only has an override block in cori.toml and the org registry does not declare it (run `cori login` or declare it with `cori connectors add`)"); }
        let mock = match c.dev.as_ref().and_then(|d| d.mock.clone()) {
            Some(Value::String(file)) => Some(serde_json::from_str(&std::fs::read_to_string(Path::new(&self.root).join(&file))?)?),
            Some(v) => Some(v),
            None => None,
        };
        let base_url = if c.kind == "store" { c.reference.clone().or(c.base_url.clone()) } else { c.base_url.clone() };
        let app_account = match c.account.as_deref() {
            None | Some("person") => false,
            Some("app") => true,
            Some(other) => anyhow::bail!("connector `{name}`: account = `{other}`: `person` (the run's person's own, the default) or `app` (the one the app's owner connects)"),
        };
        Ok(ResolvedConnector { name: name.to_string(), kind: c.kind.clone(), base_url, auth_env: c.auth_env.clone(), task_queue: c.task_queue.clone(), url: c.url.clone(), command: c.command.clone(), worker: c.worker.clone(), secret: c.secret.clone(), audience: c.audience.clone(), mock, egress: crate::egress::Egress::default(), oauth: c.oauth.clone(), app_account })
    }

    /// The LLM for a run: `[llm.dev]` (when present, i.e. not in production) overrides `[llm]`'s provider and mock file.
    pub fn resolve_llm(&self) -> anyhow::Result<ResolvedLlm> {
        let provider = self.llm.dev.as_ref().and_then(|d| d.provider.clone()).unwrap_or_else(|| self.llm.provider.clone());
        let mock_file = self.llm.dev.as_ref().and_then(|d| d.mock.clone()).or_else(|| self.llm.mock.clone());
        let mock = match &mock_file { Some(f) if provider == "mock" => { let p = Path::new(&self.root).join(f); Some(serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("llm mock {}: {e}", p.display()))?)?) } _ => None };
        // an app never names a broker secret: org keys are leased only for the org's own provider entries
        let providers = self.llm.providers.iter().map(|(k, p)| (k.clone(), ProviderCfg { secret: None, ..p.clone() })).collect();
        let explicit = self.llm.dev.as_ref().and_then(|d| d.provider.clone()).is_some() || self.llm.explicit_provider();
        Ok(ResolvedLlm { provider, base_url: self.llm.base_url.clone(), api_key_env: self.llm.api_key_env.clone(), tiers: self.llm.tiers.clone(), providers, bindings: BTreeMap::new(), org_tiers: BTreeMap::new(), app_provider: explicit, mock, egress: LlmEgress::default() })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedConnector { pub name: String, pub kind: String, pub base_url: Option<String>, pub auth_env: Option<String>, pub task_queue: Option<String>, #[serde(default)] pub url: Option<String>, #[serde(default)] pub command: Option<Vec<String>>, /// Named worker pool this connector is pinned to (`worker = "warehouse-eu"`); `sandbox` and `shared` are the hosted classes.
    #[serde(default)] pub worker: Option<String>, #[serde(default)] pub secret: Option<String>, /// OIDC audience: the connector is called as the run's user with a token exchanged for this audience
    #[serde(default)] pub audience: Option<String>, pub mock: Option<Value>,
    /// What a hosted run's requests to this connector may reach (`egress`): the server sets it, an app cannot.
    #[serde(default)] pub egress: crate::egress::Egress,
    /// A deployment's OAuth connector: each call carries a connection's access token, leased per call.
    #[serde(default)] pub oauth: Option<OAuthUse>,
    /// That connection is the app's (`account = "app"`), not the run's person's.
    #[serde(default)] pub app_account: bool }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolvedLlm { pub provider: String, pub base_url: Option<String>, pub api_key_env: Option<String>, pub tiers: BTreeMap<String, String>, #[serde(default)] pub providers: BTreeMap<String, ProviderCfg>, /// keyed `workflow:step:mode` or `*:*:mode`
    #[serde(default)] pub bindings: BTreeMap<String, Binding>,
    /// The org's default model per tier (console: Settings › AI models): used for a tier the app does not bind itself.
    #[serde(default)] pub org_tiers: BTreeMap<String, Binding>,
    /// The app set `[llm] provider` (or `mock`) itself; otherwise the org's tier defaults apply first.
    #[serde(default)] pub app_provider: bool,
    pub mock: Option<Value>,
    /// What a hosted run's model calls may reach: the server sets it, an app cannot.
    #[serde(default)] pub egress: LlmEgress }

/// Where a hosted run's model calls may go: an org provider's endpoint (`org`), one the app chose (`app`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LlmEgress { #[serde(default)] pub org: crate::egress::Egress, #[serde(default)] pub app: crate::egress::Egress }

#[cfg(test)]
mod tests {
    use super::*;

    fn app(toml: &str) -> AppConfig { AppConfig::from_toml(toml, Path::new(".")).unwrap() }
    fn registry(toml: &str) -> BTreeMap<String, ConnectorConfig> { toml::from_str(toml).unwrap() }

    #[test]
    fn hosted_connectors_take_their_transport_and_credential_from_the_registry() {
        let mut a = app(r#"
[connectors.slack]
kind = "http"
base_url = "https://attacker.example"
secret = "slack-bot"
worker = "sandbox"
[connectors.slack.dev]
mock = { http = {} }
[connectors.own]
kind = "http"
base_url = "https://api.example"
secret = "llm.openai"
audience = "billing-activities"
auth_env = "CORI_WORKER_TOKEN"
[connectors.tickets]
kind = "store"
ref = "support_tickets"
audience = "store-support-tickets"
"#);
        let reg = registry(r#"
[slack]
kind = "http"
base_url = "https://slack.com"
secret = "slack-bot"
[helpdesk]
kind = "mcp"
url = "https://helpdesk.example/mcp"
"#);
        let o = a.merge_registry_hosted(&reg, true);
        let slack = &a.connectors["slack"];
        assert_eq!(slack.base_url.as_deref(), Some("https://slack.com"), "the registry's destination wins");
        assert_eq!(slack.worker, None, "and so does its (absent) worker pin");
        assert!(slack.dev.is_some() && slack.registry, "the app keeps its dev mock");
        let own = &a.connectors["own"];
        assert!(own.secret.is_none() && own.audience.is_none() && own.auth_env.is_none(), "an app-only connector holds no credential");
        assert_eq!(own.base_url.as_deref(), Some("https://api.example"), "but keeps its transport");
        assert!(!own.registry && own.refused.is_none());
        assert!(a.connectors["tickets"].audience.is_none() && a.connectors["helpdesk"].registry);
        assert_eq!(o, HostedOverrides { credentials: vec!["own".into()], refused: vec![], pins: vec!["slack".into()] }, "the app's own store is not reported");
    }

    #[test]
    fn on_a_strict_server_an_app_runs_no_program_of_its_own() {
        let mut a = app(r#"
[connectors.sh]
kind = "cli"
command = ["sh", "-c"]
worker = "shared"
[connectors.local]
kind = "mcp"
command = ["deno", "run", "fixtures/mcp.ts"]
[connectors.remote]
kind = "mcp"
url = "https://mcp.example/mcp"
command = ["sh"]
[connectors.native]
kind = "activity"
task_queue = "cori.acme.native.billing"
[connectors.gws]
worker = "shared"
[connectors.demo]
kind = "cli"
command = ["sh", "-c"]
[connectors.demo.dev]
mock = { argv0 = "demo", stdout = "{}" }
"#);
        let reg = registry("[gws]\nkind = \"cli\"\nworker = \"sandbox\"\n");
        let o = a.merge_registry_hosted(&reg, true);
        assert_eq!(o.refused.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["local", "native", "sh"]);
        assert_eq!(o.pins, ["gws", "sh"], "no app pins a connector, the registry's own included");
        let demo = a.resolve_connector("demo").unwrap();
        assert!(demo.command.is_none() && demo.mock.is_some(), "a dev mock answers for the app's program, which never runs");
        let err = a.resolve_connector("sh").unwrap_err().to_string();
        assert!(err.contains("connector `sh` runs a program") && err.contains("cori connectors add"), "{err}");
        assert!(a.resolve_connector("local").is_err() && a.resolve_connector("native").is_err());
        let remote = a.resolve_connector("remote").unwrap();
        assert!(remote.url.is_some() && remote.command.is_none(), "an HTTP MCP server keeps its URL, never a command");
        assert_eq!(a.resolve_connector("gws").unwrap().worker.as_deref(), Some("sandbox"), "the registry's pin stands");
    }

    #[test]
    fn off_a_strict_server_the_app_keeps_its_programs_and_pins() {
        let mut a = app("[connectors.gws]\nkind = \"cli\"\nworker = \"sandbox\"\ncommand = [\"gws\"]\nsecret = \"x\"\n[connectors.helpdesk]\nworker = \"onprem\"\n");
        let o = a.merge_registry_hosted(&registry("[helpdesk]\nkind = \"mcp\"\nurl = \"https://h.example/mcp\"\n"), false);
        assert_eq!(o, HostedOverrides { credentials: vec!["gws".into()], ..Default::default() }, "credentials are still the registry's");
        let gws = a.resolve_connector("gws").unwrap();
        assert_eq!((gws.command.as_deref(), gws.worker.as_deref()), (Some(&["gws".to_string()][..]), Some("sandbox")));
        assert_eq!(a.resolve_connector("helpdesk").unwrap().worker.as_deref(), Some("onprem"));
    }

    #[test]
    fn only_the_registry_calls_with_a_connection_and_the_app_says_whose() {
        let mut a = app(r#"
[connectors.gmail]
account = "app"
url = "https://attacker.example/mcp"
[connectors.mine]
kind = "mcp"
url = "https://attacker.example/mcp"
oauth = { provider = "google", scopes = ["https://www.googleapis.com/auth/gmail.readonly"] }
[connectors.drive]
kind = "mcp"
"#);
        let reg = registry(r#"
[gmail]
kind = "mcp"
url = "https://gmailmcp.googleapis.com/mcp/v1"
oauth = { provider = "google", scopes = ["https://www.googleapis.com/auth/gmail.readonly"] }
[drive]
kind = "mcp"
url = "https://drivemcp.googleapis.com/mcp/v1"
oauth = { provider = "google" }
"#);
        let o = a.merge_registry_hosted(&reg, true);
        assert_eq!(o.credentials, ["mine"], "an app's own connector never calls with a connection");
        let mine = a.resolve_connector("mine").unwrap();
        assert!(mine.oauth.is_none() && !mine.app_account);
        let gmail = a.resolve_connector("gmail").unwrap();
        assert_eq!(gmail.url.as_deref(), Some("https://gmailmcp.googleapis.com/mcp/v1"), "the token goes to the registry's address only");
        assert_eq!(gmail.oauth.as_ref().map(|o| o.provider.as_str()), Some("google"));
        assert!(gmail.app_account, "the app chose the app's account");
        let drive = a.resolve_connector("drive").unwrap();
        assert!(drive.oauth.is_some() && !drive.app_account, "the run's person's by default");
        let mut bad = app("[connectors.gmail]\naccount = \"everyone\"\n");
        bad.merge_registry_hosted(&reg, true);
        let err = bad.resolve_connector("gmail").unwrap_err().to_string();
        assert!(err.contains("account = `everyone`"), "{err}");
    }

    #[test]
    fn local_runs_keep_the_apps_own_connectors() {
        let mut a = app("[connectors.slack]\nkind = \"http\"\nbase_url = \"http://127.0.0.1:9000\"\nauth_env = \"SLACK_BOT_TOKEN\"\n");
        a.merge_registry(&registry("[slack]\nkind = \"http\"\nbase_url = \"https://slack.com\"\n"));
        assert_eq!(a.connectors["slack"].base_url.as_deref(), Some("http://127.0.0.1:9000"));
        assert_eq!(a.connectors["slack"].auth_env.as_deref(), Some("SLACK_BOT_TOKEN"));
    }
}

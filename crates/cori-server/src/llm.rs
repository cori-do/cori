// SPDX-License-Identifier: Apache-2.0
//! The org's LLM providers and the monthly LLM budget of each app (console: Settings › AI models, and each app's Settings).
//!
//! - Providers: `kind` (`openai` for any OpenAI-compatible API, `anthropic`), base URL, models with their prices (€ per
//!   million tokens) and an API key kept in the credential broker as `llm.<provider>` (write-only; a worker gets it as a
//!   lease for one call of a running run). Admins manage them; operators read them.
//! - Tier defaults: the model answering `small` / `medium` / `large` steps of apps that do not choose their own provider.
//! - Prices: the server prices every call of a hosted run itself (`price`, `settle`), from the tokens the worker reports:
//!   the app's own `[llm.providers]` prices for a provider it declares, else the org's for an org provider.
//! - Budgets: euros per calendar month (UTC) per app, and for the realm as a whole (`realm::Limits::llm_eur_per_month`).
//!   Every LLM call of a hosted run first asks for a lease (`POST /v1/leases` with `llm`), which sets aside what the call
//!   may cost (its prompt, and `CORI_LLM_RESERVE_OUTPUT_TOKENS` of answer, default 4096). Under one lock per realm, the
//!   lease is refused with 402 (the step fails with `BudgetExceeded`, not retried) when the month's spend (every call,
//!   whatever its mode), what calls in flight set aside and this one would pass a budget: concurrent calls cannot go over
//!   it together. A call's capture (`settle`) releases what it set aside; what a call that never reported set aside is
//!   released after `CORI_LLM_RESERVATION_SECS` (default 600).
//! - The starter model (`Starter`): one model the deployment offers every realm, on its own key, for the tiers the realm
//!   set no default for; a few calls a minute and a day per realm, at no cost to it. A realm's own providers replace it.
use crate::api::{ApiError, Caller, InRealm, require_human};
use crate::{db, state::AppState};
use axum::{Json, extract::Path, http::StatusCode, response::{IntoResponse, Response}};
use cori_engine::config::{Binding, ModelPrice, ProviderCfg};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::Arc;

type Res = Result<Response, ApiError>;

pub const TIERS: [&str; 3] = ["small", "medium", "large"];
const KINDS: [&str; 2] = ["openai", "anthropic"];

/// The broker secret holding an org provider's key.
pub fn secret_name(provider: &str) -> String { format!("llm.{provider}") }

/// Who providers and tier defaults the deployment's providers file sets were updated by (`apply_providers_file`): the console
/// and the API leave them alone.
pub const MANAGED: &str = "cori:providers-file";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvidersFile { #[serde(default)] providers: Vec<FileProvider>, #[serde(default)] tiers: BTreeMap<String, FileTier> }

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileProvider {
    name: String,
    #[serde(default = "openai_kind")] kind: String,
    #[serde(default)] base_url: Option<String>,
    /// The environment variable holding its key (from the deployment's secret); none, a keyless endpoint.
    #[serde(default)] key_env: Option<String>,
    #[serde(default)] models: Vec<FileModel>,
    #[serde(default = "yes")] enabled: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModel { id: String, #[serde(default)] input_eur_per_mtok: f64, #[serde(default)] output_eur_per_mtok: f64 }

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTier { provider: String, model: String }

fn openai_kind() -> String { "openai".into() }
fn yes() -> bool { true }

/// The deployment's LLM providers and tier defaults, declared in a file (`CORI_LLM_PROVIDERS_FILE`, YAML or JSON) and applied
/// at every start: each provider (its kind, base URL, models and their prices, its key from the environment variable it
/// names) and each tier's default model are written as an admin would, marked as the file's (`MANAGED`). The console shows
/// them, and refuses to change them; what the file no longer declares goes. A deployment then starts with working models,
/// priced, so that the usage ledger and the budgets count them. A file that does not read stops the server.
pub async fn apply_providers_file(st: &Arc<AppState>) -> anyhow::Result<()> {
    let Some(path) = std::env::var("CORI_LLM_PROVIDERS_FILE").ok().filter(|p| !p.trim().is_empty()) else { return Ok(()) };
    if st.tenancy.is_some() { anyhow::bail!("CORI_LLM_PROVIDERS_FILE: a server of several organizations has each declare its own providers"); }
    let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("CORI_LLM_PROVIDERS_FILE `{path}`: {e}"))?;
    let file: ProvidersFile = serde_yaml::from_str(&text).map_err(|e| anyhow::anyhow!("CORI_LLM_PROVIDERS_FILE `{path}`: {e}"))?;
    // checked as the console checks an admin's: names, kinds, URLs, prices; tiers name a provider and a model it lists
    let mut names = std::collections::BTreeSet::new();
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    for p in &file.providers {
        if p.name.is_empty() || p.name == "mock" || p.name == STARTER || !p.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { anyhow::bail!("{path}: provider name `{}`: [A-Za-z0-9-_], not `mock` nor `{STARTER}`", p.name); }
        if !names.insert(p.name.clone()) { anyhow::bail!("{path}: provider `{}` is declared twice", p.name); }
        if !KINDS.contains(&p.kind.as_str()) { anyhow::bail!("{path}: provider `{}`: kind one of {}", p.name, KINDS.join(", ")); }
        if let Some(u) = &p.base_url && reqwest::Url::parse(u).map(|u| !matches!(u.scheme(), "http" | "https")).unwrap_or(true) { anyhow::bail!("{path}: provider `{}`: base_url `{u}` is not an http(s) URL", p.name); }
        for m in &p.models { if m.id.trim().is_empty() || !(m.input_eur_per_mtok.is_finite() && m.input_eur_per_mtok >= 0.0 && m.output_eur_per_mtok.is_finite() && m.output_eur_per_mtok >= 0.0) { anyhow::bail!("{path}: provider `{}`: a model needs an id and prices ≥ 0", p.name); } }
        if let Some(var) = &p.key_env {
            if var.starts_with("CORI_") && !var.starts_with("CORI_LLM_KEY_") { anyhow::bail!("{path}: provider `{}`: key_env `{var}` is one of Cori's own settings; name a variable of its own (CORI_LLM_KEY_*)", p.name); }
            let v = std::env::var(var).ok().filter(|v| !v.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("{path}: provider `{}` takes its key from `{var}`, which is not set", p.name))?;
            keys.insert(p.name.clone(), v.trim().to_string());
        }
    }
    for (tier, t) in &file.tiers {
        if !TIERS.contains(&tier.as_str()) { anyhow::bail!("{path}: tier `{tier}`: one of {}", TIERS.join(", ")); }
        let p = file.providers.iter().find(|p| p.name == t.provider).ok_or_else(|| anyhow::anyhow!("{path}: tier `{tier}` names provider `{}`, which the file does not declare", t.provider))?;
        if !p.models.iter().any(|m| m.id == t.model) { anyhow::bail!("{path}: tier `{tier}`: provider `{}` does not list the model `{}`", t.provider, t.model); }
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = st.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('cori_llm_providers_file'))").execute(&mut *tx).await?;
    for p in &file.providers {
        // a provider of the same name an admin made is taken over: the file is the deployment's word
        let models: Vec<Value> = p.models.iter().map(|m| json!({ "id": m.id.trim(), "input_eur_per_mtok": m.input_eur_per_mtok, "output_eur_per_mtok": m.output_eur_per_mtok })).collect();
        sqlx::query("INSERT INTO llm_providers (name, kind, base_url, models, enabled, updated_at, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT ON CONSTRAINT llm_providers_pkey DO UPDATE SET kind = $2, base_url = $3, models = $4, enabled = $5, updated_at = $6, updated_by = $7")
            .bind(&p.name).bind(&p.kind).bind(&p.base_url).bind(Value::Array(models).to_string()).bind(p.enabled as i64).bind(&now).bind(MANAGED).execute(&mut *tx).await?;
    }
    let gone: Vec<String> = sqlx::query_scalar("SELECT name FROM llm_providers WHERE updated_by = $1").bind(MANAGED).fetch_all(&mut *tx).await?.into_iter().filter(|n: &String| !names.contains(n)).collect();
    for n in &gone {
        sqlx::query("DELETE FROM llm_tiers WHERE provider = $1").bind(n).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM llm_providers WHERE name = $1").bind(n).execute(&mut *tx).await?;
        sqlx::query("UPDATE secrets SET revoked_at = $1 WHERE scope = '' AND name = $2 AND revoked_at IS NULL").bind(&now).bind(secret_name(n)).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM llm_tiers WHERE updated_by = $1").bind(MANAGED).execute(&mut *tx).await?;
    for (tier, t) in &file.tiers {
        sqlx::query("INSERT INTO llm_tiers (tier, provider, model, updated_at, updated_by) VALUES ($1, $2, $3, $4, $5) ON CONFLICT ON CONSTRAINT llm_tiers_pkey DO UPDATE SET provider = $2, model = $3, updated_at = $4, updated_by = $5")
            .bind(tier).bind(&t.provider).bind(&t.model).bind(&now).bind(MANAGED).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    // keys: a new broker version only when the value changed, so that restarts do not pile versions up
    for (name, key) in &keys {
        if st.broker.reveal(&st.db, &secret_name(name)).await.ok().as_deref() != Some(key.as_str()) { st.broker.put(&st.db, &secret_name(name), "llm_key", key, MANAGED).await?; }
    }
    if !gone.is_empty() { tracing::warn!(providers = ?gone, "LLM providers the providers file no longer declares were removed: steps bound to them fail until rebound"); }
    tracing::info!(providers = file.providers.len(), tiers = file.tiers.len(), "LLM providers file applied");
    Ok(())
}

/// Whether the providers file manages provider `name`.
async fn managed_provider(db: &crate::db::Db, name: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, String>("SELECT updated_by FROM llm_providers WHERE name = $1").bind(name).fetch_optional(db).await?.as_deref() == Some(MANAGED))
}

/// The provider name the starter model takes in a run's configuration; no org provider may take it.
pub const STARTER: &str = "starter";

/// How long a lease waits for a starter call of the minute to free up before it refuses (the call is then retried).
const STARTER_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The model the deployment offers every realm for the tiers it set no default for (`CORI_LLM_STARTER_*`): one model of an
/// OpenAI-compatible endpoint on the deployment's key, a few calls a minute and a day per realm, at no cost to the realm.
/// The key reaches a worker only in the lease of one call of that model, sent to that endpoint.
#[derive(Clone)]
pub struct Starter { pub model: String, pub base_url: String, pub key: String, pub per_minute: u32, pub per_day: u32, pub note: Option<String> }

// the key never in a log
impl std::fmt::Debug for Starter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Starter").field("model", &self.model).field("base_url", &self.base_url).field("per_minute", &self.per_minute).field("per_day", &self.per_day).finish_non_exhaustive()
    }
}

impl Starter {
    /// `CORI_LLM_STARTER_MODEL` and `_BASE_URL`, both or no starter model, and `_KEY` (none for a keyless gateway); calls per realm,
    /// `_CALLS_PER_MINUTE` (default 5) and `_CALLS_PER_DAY` (default 50; 0, no limit); `_NOTE`, what the console says of
    /// it (where the calls go, what its provider keeps).
    pub fn from_env() -> Option<Starter> {
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let (model, base_url, key) = (env("CORI_LLM_STARTER_MODEL"), env("CORI_LLM_STARTER_BASE_URL"), env("CORI_LLM_STARTER_KEY"));
        // a gateway on the private network may take no key: the calls then carry none
        let (Some(model), Some(base_url)) = (model.clone(), base_url) else {
            if model.is_some() { eprintln!("warning: CORI_LLM_STARTER_MODEL needs CORI_LLM_STARTER_BASE_URL too: no starter model"); }
            return None;
        };
        let key = key.unwrap_or_default();
        let calls = |k: &str, default: u32| env(k).and_then(|v| v.parse().ok()).unwrap_or(default);
        Some(Starter { model, base_url, key, per_minute: calls("CORI_LLM_STARTER_CALLS_PER_MINUTE", 5), per_day: calls("CORI_LLM_STARTER_CALLS_PER_DAY", 50), note: env("CORI_LLM_STARTER_NOTE") })
    }

    /// What the console shows of it, for a realm: the model, its calls a minute and a day, today's, and its note.
    pub async fn status(&self, db: &db::Db) -> Result<Value, ApiError> {
        Ok(json!({ "model": self.model, "calls_per_minute": self.per_minute, "calls_per_day": self.per_day, "calls_today": starter_calls_since(db, today()).await?, "note": self.note }))
    }
}

/// Midnight (UTC) of today, when a realm's starter calls start again.
fn today() -> chrono::DateTime<chrono::Utc> { chrono::Utc::now().date_naive().and_hms_opt(0, 0, 0).unwrap_or_default().and_utc() }

/// The starter calls a realm was granted since `t`.
async fn starter_calls_since(db: &db::Db, t: chrono::DateTime<chrono::Utc>) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM secret_use WHERE name = $1 AND outcome = 'granted' AND at >= $2").bind(secret_name(STARTER)).bind(t.to_rfc3339()).fetch_one(db).await?)
}

pub struct Provider { pub name: String, pub kind: String, pub base_url: Option<String>, pub models: Vec<Value>, pub enabled: bool, pub updated_at: String, pub updated_by: String }

pub async fn providers(db: &crate::db::Db) -> Result<Vec<Provider>, ApiError> {
    Ok(sqlx::query("SELECT name, kind, base_url, models, enabled, updated_at, updated_by FROM llm_providers ORDER BY name").fetch_all(db).await?.iter().map(|r| Provider {
        name: r.get("name"), kind: r.get("kind"), base_url: r.get("base_url"), models: serde_json::from_str(&r.get::<String, _>("models")).unwrap_or_default(),
        enabled: r.get::<i64, _>("enabled") != 0, updated_at: r.get("updated_at"), updated_by: r.get("updated_by"),
    }).collect())
}

async fn has_key(db: &crate::db::Db, provider: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query("SELECT 1 FROM secrets WHERE scope = '' AND name = $1 AND revoked_at IS NULL").bind(secret_name(provider)).fetch_optional(db).await?.is_some())
}

/// The org's tier defaults: tier → (provider, model).
pub async fn tier_defaults(db: &crate::db::Db) -> Result<BTreeMap<String, Binding>, ApiError> {
    Ok(sqlx::query("SELECT tier, provider, model FROM llm_tiers").fetch_all(db).await?.iter()
        .map(|r| (r.get::<String, _>("tier"), Binding { provider: r.get("provider"), model: r.get("model"), mode: "primary".into() })).collect())
}

/// Adds the org's enabled providers (without keys: those are leased per call) and tier defaults to a run's LLM
/// configuration, then the deployment's starter model for every tier the org set no default for. A provider the app
/// declares itself under the same name stays the app's.
pub async fn merge_into(db: &crate::db::Db, starter: Option<&Starter>, llm: &mut cori_engine::config::ResolvedLlm) -> Result<(), ApiError> {
    for p in providers(db).await?.into_iter().filter(|p| p.enabled) {
        if llm.providers.contains_key(&p.name) { continue; }
        let models = p.models.iter().filter_map(|m| Some((m["id"].as_str()?.to_string(), ModelPrice { input_eur_per_mtok: m["input_eur_per_mtok"].as_f64().unwrap_or(0.0), output_eur_per_mtok: m["output_eur_per_mtok"].as_f64().unwrap_or(0.0) }))).collect();
        llm.providers.insert(p.name.clone(), ProviderCfg { base_url: p.base_url.clone(), api_key_env: None, kind: Some(p.kind.clone()), secret: Some(secret_name(&p.name)), models });
    }
    llm.org_tiers = tier_defaults(db).await?;
    // as an org provider would be: a `secret` (its key leased per call) and a base_url of its own, which an app's
    // `[llm] base_url` never replaces; its one model costs nothing
    if let Some(s) = starter.filter(|_| !llm.providers.contains_key(STARTER)) {
        llm.providers.insert(STARTER.into(), ProviderCfg { base_url: Some(s.base_url.clone()), api_key_env: None, kind: Some("openai".into()), secret: Some(format!("deployment:{STARTER}")), models: [(s.model.clone(), ModelPrice::default())].into() });
        for tier in TIERS { llm.org_tiers.entry(tier.to_string()).or_insert_with(|| Binding { provider: STARTER.into(), model: s.model.clone(), mode: "primary".into() }); }
    }
    Ok(())
}

/// An app's LLM month, from the usage ledger: spend (every mode), calls, tokens, what calls in flight set aside, and its
/// budget when it has one.
pub async fn month_usage(db: &crate::db::Db, app: &str) -> Result<Value, ApiError> {
    let m = |metric: &'static str| crate::usage::month(db, metric, Some(app));
    let (calls, tin, tout, spent) = (m("llm_calls").await?, m("llm_tokens_in").await?, m("llm_tokens_out").await?, m("llm_eur").await?);
    let reserved: f64 = sqlx::query_scalar::<_, Option<f64>>("SELECT SUM(eur) FROM llm_reservations WHERE app = $1 AND expires_at >= $2").bind(app).bind(chrono::Utc::now().to_rfc3339()).fetch_one(db).await?.unwrap_or(0.0);
    let budget: Option<f64> = sqlx::query("SELECT monthly_eur FROM llm_budgets WHERE app = $1").bind(app).fetch_optional(db).await?.map(|r| r.get("monthly_eur"));
    Ok(json!({ "month": chrono::Utc::now().format("%Y-%m").to_string(), "calls": calls as i64, "tokens_in": tin as i64, "tokens_out": tout as i64, "spent_eur": spent, "reserved_eur": reserved,
        "budget_eur": budget, "remaining_eur": budget.map(|b| (b - spent).max(0.0)), "blocked": budget.map(|b| spent >= b).unwrap_or(false) }))
}

/// What a call to `model` of `provider` costs in a run of `version`: the app's own `[llm.providers]` prices for a provider
/// it declares (a provider of the app stays the app's, as the run's configuration has it: `merge_into`), else the org's.
/// None for a model nobody priced: its calls cost 0 €.
pub async fn price(db: &crate::db::Db, version: &str, provider: &str, model: &str) -> Result<Option<ModelPrice>, ApiError> {
    let app_cfg = sqlx::query("SELECT cori_toml FROM versions WHERE hash = $1").bind(version).fetch_optional(db).await?
        .and_then(|row| cori_engine::config::AppConfig::from_toml(&row.get::<String, _>("cori_toml"), std::path::Path::new(".")).ok());
    if let Some(p) = app_cfg.as_ref().and_then(|c| c.llm.providers.get(provider)) { return Ok(p.models.get(model).cloned()); }
    Ok(providers(db).await?.into_iter().find(|p| p.name == provider).and_then(|p| p.models.iter().find(|m| m["id"].as_str() == Some(model)).map(|m| ModelPrice { input_eur_per_mtok: m["input_eur_per_mtok"].as_f64().unwrap_or(0.0), output_eur_per_mtok: m["output_eur_per_mtok"].as_f64().unwrap_or(0.0) })))
}

/// Euros for `tokens_in` and `tokens_out` at `p`.
pub fn cost(p: &ModelPrice, tokens_in: u64, tokens_out: u64) -> f64 { (tokens_in as f64 * p.input_eur_per_mtok + tokens_out as f64 * p.output_eur_per_mtok) / 1_000_000.0 }

/// The answer tokens a lease sets aside (`CORI_LLM_RESERVE_OUTPUT_TOKENS`, default 4096).
fn reserve_output_tokens() -> u64 { std::env::var("CORI_LLM_RESERVE_OUTPUT_TOKENS").ok().and_then(|v| v.parse().ok()).unwrap_or(4096) }

/// How long a lease's reservation holds without the call's capture (`CORI_LLM_RESERVATION_SECS`, default 600).
fn reservation_ttl() -> chrono::Duration { chrono::Duration::seconds(std::env::var("CORI_LLM_RESERVATION_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(600)) }

/// A captured call of a hosted run (`llm_call`): priced here (the worker's figure is not trusted), added to the ledger,
/// and the oldest reservation of its run and provider released. Returns its cost.
#[allow(clippy::too_many_arguments)]
pub async fn settle(db: &crate::db::Db, run: &str, app: &str, version: &str, provider: &str, model: &str, tokens_in: u64, tokens_out: u64) -> Result<f64, ApiError> {
    let eur = if provider.is_empty() || provider == "mock" { 0.0 } else { price(db, version, provider, model).await?.map(|p| cost(&p, tokens_in, tokens_out)).unwrap_or(0.0) };
    sqlx::query("DELETE FROM llm_reservations WHERE id = (SELECT id FROM llm_reservations WHERE run_id = $1 AND provider = $2 ORDER BY expires_at LIMIT 1)").bind(run).bind(provider).execute(db).await?;
    crate::usage::add(db, "llm_calls", app, 1.0).await;
    crate::usage::add(db, "llm_tokens_in", app, tokens_in as f64).await;
    crate::usage::add(db, "llm_tokens_out", app, tokens_out as f64).await;
    crate::usage::add(db, "llm_eur", app, eur).await;
    Ok(eur)
}

/// The LLM part of `POST /v1/leases` (`{ run_id, llm: <provider>, model, tokens_in }`), for a worker about to call a model
/// for a running run: refused (402) when the call could take the app's or the realm's monthly spend over its budget, or
/// (403) when the org disabled the provider; else what the call may cost is set aside, and for an org provider the answer
/// carries its key, for a provider the app declares itself only the go-ahead.
pub async fn lease(st: &Arc<AppState>, worker: &crate::workers::Worker, run: &str, provider: &str, body: &Value) -> Res {
    let log = |outcome: &'static str| { let (st, run, provider, w) = (st.clone(), run.to_string(), provider.to_string(), worker.label()); async move {
        sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, $2, $3, $4, $5, $6, $7)").bind(uuid::Uuid::new_v4().to_string()).bind(secret_name(&provider)).bind(run).bind(format!("llm:{provider}")).bind(w).bind(chrono::Utc::now().to_rfc3339()).bind(outcome).execute(&st.db).await.ok();
    } };
    let Some(r) = crate::workers::run_ref(st, run).await? else { log("unknown_run").await; return Err(ApiError(StatusCode::FORBIDDEN, "lease refused: unknown_run".into())) };
    if r.status != "running" { log("run_not_running").await; return Err(ApiError(StatusCode::FORBIDDEN, "lease refused: run_not_running".into())); }
    if !crate::workers::serves_app(worker, &crate::workers::app_connectors(st, &r.app, &r.version).await?) { log("worker_does_not_serve_app").await; return Err(ApiError(StatusCode::FORBIDDEN, format!("lease refused: {} does not serve app `{}`", worker.label(), r.app))); }
    // what the call may cost: its prompt and the answer tokens set aside, at the price it is billed
    let model = body["model"].as_str().unwrap_or("");
    let estimate = price(&st.db, &r.version, provider, model).await?.map(|p| cost(&p, body["tokens_in"].as_u64().unwrap_or(0), reserve_output_tokens())).unwrap_or(0.0);
    let lock = format!("cori_llm:{}", st.realm().map(|r| r.as_str()).unwrap_or(""));
    if let Err(why) = reserve(&st.db, &lock, st.settings.limits.llm_eur_per_month, run, &r.app, provider, estimate).await? {
        log("llm_budget_exceeded").await;
        return Err(ApiError(StatusCode::PAYMENT_REQUIRED, why));
    }
    let usage = month_usage(&st.db, &r.app).await?;
    let org = providers(&st.db).await?.into_iter().find(|p| p.name == provider);
    // a provider the app declares itself (its key is an env var on the worker): only the budget applies
    let app_own = sqlx::query("SELECT cori_toml FROM versions WHERE hash = $1").bind(&r.version).fetch_optional(&st.db).await?
        .and_then(|row| cori_engine::config::AppConfig::from_toml(&row.get::<String, _>("cori_toml"), std::path::Path::new(".")).ok()).map(|c| c.llm.providers.contains_key(provider)).unwrap_or(false);
    // the deployment's starter model: its one model, within the realm's calls, on the deployment's key
    if provider == STARTER && !app_own && org.is_none() {
        let Some(s) = &st.cfg.llm_starter else { log("no_starter").await; return Err(ApiError(StatusCode::FORBIDDEN, "lease refused: this server offers no starter model".into())) };
        if model != s.model { log("starter_model").await; return Err(ApiError(StatusCode::FORBIDDEN, format!("lease refused: the starter model is `{}` only", s.model))); }
        if let Err(e) = starter_call(&st.db, &lock, s, run, &worker.label()).await { log(if e.0 == StatusCode::PAYMENT_REQUIRED { "starter_day_used" } else { "starter_busy" }).await; return Err(e); }
        return Ok(Json(json!({ "value": s.key, "ttl_secs": 300, "budget": usage })).into_response());
    }
    let Some(p) = org.filter(|_| !app_own) else { log("budget_ok").await; return Ok(Json(json!({ "value": null, "budget": usage })).into_response()) };
    if !p.enabled { log("provider_disabled").await; return Err(ApiError(StatusCode::FORBIDDEN, format!("lease refused: the org disabled the LLM provider `{provider}`"))); }
    match st.broker.reveal(&st.db, &secret_name(provider)).await {
        Ok(v) => { log("granted").await; Ok(Json(json!({ "value": v, "ttl_secs": 300, "budget": usage })).into_response()) }
        // a keyless provider (a vLLM on the private network)
        Err(_) => { log("no_key").await; Ok(Json(json!({ "value": "", "budget": usage })).into_response()) }
    }
}

/// Sets aside `estimate` € for a call of `app` (`lease`), or says which budget it would pass: the app's, or the realm's
/// monthly `cap` (0, none). One lease at a time per realm (a transaction-scoped advisory lock on `lock`): two calls cannot
/// both take the last of a budget.
#[allow(clippy::too_many_arguments)]
async fn reserve(db: &crate::db::Db, lock: &str, cap: f64, run: &str, app: &str, provider: &str, estimate: f64) -> Result<Result<(), String>, ApiError> {
    let now = chrono::Utc::now();
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(lock).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM llm_reservations WHERE expires_at < $1").bind(now.to_rfc3339()).execute(&mut *tx).await?;
    let sum = |sql: &'static str, app: Option<&str>| { let mut q = sqlx::query_scalar::<_, Option<f64>>(sql).bind(crate::usage::month_first()); if let Some(a) = app { q = q.bind(a.to_string()); } q };
    let budget: Option<f64> = sqlx::query_scalar("SELECT monthly_eur FROM llm_budgets WHERE app = $1").bind(app).fetch_optional(&mut *tx).await?;
    if let Some(b) = budget {
        let spent = sum("SELECT SUM(quantity) FROM usage WHERE metric = 'llm_eur' AND day >= $1 AND app = $2", Some(app)).fetch_one(&mut *tx).await?.unwrap_or(0.0);
        let held: f64 = sqlx::query_scalar::<_, Option<f64>>("SELECT SUM(eur) FROM llm_reservations WHERE app = $1").bind(app).fetch_one(&mut *tx).await?.unwrap_or(0.0);
        if spent >= b || spent + held + estimate > b {
            return Ok(Err(format!("`{app}` spent {spent:.2} € of its {b:.2} € monthly LLM budget, calls under way may cost {held:.2} € more, and this one up to {estimate:.2} €: LLM calls resume next month, as calls under way end, or when an admin raises the budget (console: the app's Settings › AI budget)")));
        }
    }
    if cap > 0.0 {
        let spent = sum("SELECT SUM(quantity) FROM usage WHERE metric = 'llm_eur' AND day >= $1", None).fetch_one(&mut *tx).await?.unwrap_or(0.0);
        let held: f64 = sqlx::query_scalar::<_, Option<f64>>("SELECT SUM(eur) FROM llm_reservations").fetch_one(&mut *tx).await?.unwrap_or(0.0);
        if spent >= cap || spent + held + estimate > cap {
            return Ok(Err(format!("limit reached: the apps here spent {spent:.2} € of the {cap:.2} € of LLM calls allowed this month, calls under way may cost {held:.2} € more, and this one up to {estimate:.2} €: LLM calls resume next month")));
        }
    }
    if estimate > 0.0 {
        sqlx::query("INSERT INTO llm_reservations (id, run_id, app, provider, eur, expires_at) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(run).bind(app).bind(provider).bind(estimate).bind((now + reservation_ttl()).to_rfc3339()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Takes one of the realm's starter calls, counted from the leases granted for it (`secret_use`), under the realm's LLM
/// lock: two leases cannot both take the last. The day's used, 402 (the step fails, saying when the model answers again
/// and how to use one's own). The minute's used, the lease waits for the oldest to age out, the lock released meanwhile,
/// up to `STARTER_WAIT`; then 429 (the call is retried).
async fn starter_call(db: &db::Db, lock: &str, s: &Starter, run: &str, worker: &str) -> Result<(), ApiError> {
    let deadline = std::time::Instant::now() + STARTER_WAIT;
    loop {
        let now = chrono::Utc::now();
        let mut tx = db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(lock).execute(&mut *tx).await?;
        let calls = |since: chrono::DateTime<chrono::Utc>| sqlx::query("SELECT COUNT(*) AS n, MIN(at) AS oldest FROM secret_use WHERE name = $1 AND outcome = 'granted' AND at >= $2").bind(secret_name(STARTER)).bind(since.to_rfc3339());
        if s.per_day > 0 && calls(today()).fetch_one(&mut *tx).await?.get::<i64, _>("n") >= s.per_day as i64 {
            return Err(ApiError(StatusCode::PAYMENT_REQUIRED, format!("the starter model answers an organization {} calls a day, and today's are used: it answers again at midnight UTC; for more, add a provider of your own (console, LLM)", s.per_day)));
        }
        let minute = calls(now - chrono::Duration::seconds(60)).fetch_one(&mut *tx).await?;
        if s.per_minute == 0 || minute.get::<i64, _>("n") < s.per_minute as i64 {
            sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, $2, $3, $4, $5, $6, 'granted')")
                .bind(uuid::Uuid::new_v4().to_string()).bind(secret_name(STARTER)).bind(run).bind(format!("llm:{STARTER}")).bind(worker).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
        drop(tx);
        // the minute's oldest call ages out: then one more fits
        let oldest = minute.get::<Option<String>, _>("oldest").and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.with_timezone(&chrono::Utc)).unwrap_or(now);
        let wait = (oldest + chrono::Duration::seconds(60) - now).to_std().unwrap_or_default().max(std::time::Duration::from_millis(100));
        if std::time::Instant::now() + wait > deadline {
            return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, format!("the starter model answers an organization {} calls a minute, and this minute's are used", s.per_minute)));
        }
        tokio::time::sleep(wait).await;
    }
}

fn require_admin(st: &AppState, who: &crate::auth::Principal, what: &str) -> Result<(), ApiError> {
    require_human(who, what)?;
    crate::api::require_power(st, who, &[crate::api::Power::Admin], what)
}

fn provider_json(p: &Provider, key: bool) -> Value {
    json!({ "name": p.name, "kind": p.kind, "base_url": p.base_url, "models": p.models, "enabled": p.enabled, "has_key": key, "updated_at": p.updated_at, "updated_by": p.updated_by, "managed": p.updated_by == MANAGED })
}

/// `GET /v1/llm`: providers (never their keys), tier defaults, and the budget and month of every app. Operators and admins.
pub(crate) async fn overview(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    crate::api::require_power(&st, &who, &[crate::api::Power::Operator, crate::api::Power::Billing], "the LLM providers and budgets")?;
    let mut provs = Vec::new();
    for p in providers(&st.db).await? { let k = has_key(&st.db, &p.name).await?; provs.push(provider_json(&p, k)); }
    let managed: Vec<String> = sqlx::query_scalar("SELECT tier FROM llm_tiers WHERE updated_by = $1").bind(MANAGED).fetch_all(&st.db).await?;
    let tiers: BTreeMap<String, Value> = tier_defaults(&st.db).await?.into_iter().map(|(t, b)| { let m = managed.contains(&t); (t, json!({ "provider": b.provider, "model": b.model, "managed": m })) }).collect();
    let mut apps = Vec::new();
    for r in sqlx::query("SELECT id FROM apps ORDER BY id").fetch_all(&st.db).await? {
        let app: String = r.get("id");
        let mut u = month_usage(&st.db, &app).await?;
        u["app"] = json!(app);
        apps.push(u);
    }
    // the deployment's starter model, and the tiers it answers here (those without a default of the org's)
    let starter = match &st.cfg.llm_starter {
        Some(s) => { let mut v = s.status(&st.db).await?; v["tiers"] = json!(TIERS.iter().filter(|t| !tiers.contains_key(**t)).collect::<Vec<_>>()); v }
        None => Value::Null,
    };
    Ok(Json(json!({ "providers": provs, "tiers": tiers, "tier_names": TIERS, "kinds": KINDS, "apps": apps, "starter": starter, "can_edit": who.is_admin() && !who.is_agent() })).into_response())
}

/// `PUT /v1/llm/providers/{name}` `{ kind, base_url?, models: [{ id, input_eur_per_mtok, output_eur_per_mtok }], enabled, api_key? }`:
/// creates or updates a provider; a non-empty `api_key` replaces the stored one (a new broker version), an absent one keeps it.
pub(crate) async fn put_provider(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_admin(&st, &who, "managing LLM providers")?;
    if name.is_empty() || name == "mock" || name == STARTER || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { return Err(ApiError(StatusCode::BAD_REQUEST, format!("provider name: [A-Za-z0-9-_], not `mock` nor `{STARTER}` (the deployment's starter model)"))); }
    if managed_provider(&st.db, &name).await? { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is the deployment's (CORI_LLM_PROVIDERS_FILE): change it in that file"))); }
    let kind = b["kind"].as_str().unwrap_or("openai");
    if !KINDS.contains(&kind) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("kind: one of {}", KINDS.join(", ")))); }
    let base_url = b["base_url"].as_str().map(str::trim).filter(|u| !u.is_empty()).map(String::from);
    if let Some(u) = &base_url { if reqwest::Url::parse(u).map(|u| !matches!(u.scheme(), "http" | "https")).unwrap_or(true) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("base_url `{u}` is not an http(s) URL"))); } }
    let mut models = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for m in b["models"].as_array().cloned().unwrap_or_default() {
        let id = m["id"].as_str().map(str::trim).unwrap_or("").to_string();
        if id.is_empty() { continue; }
        if !seen.insert(id.clone()) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("model `{id}` is listed twice"))); }
        let price = |k: &str| -> Result<f64, ApiError> { let v = m[k].as_f64().unwrap_or(0.0); if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(ApiError(StatusCode::BAD_REQUEST, format!("{k} of `{id}`: a price ≥ 0"))) } };
        models.push(json!({ "id": id, "input_eur_per_mtok": price("input_eur_per_mtok")?, "output_eur_per_mtok": price("output_eur_per_mtok")? }));
    }
    let enabled = b["enabled"].as_bool().unwrap_or(true);
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO llm_providers (name, kind, base_url, models, enabled, updated_at, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT ON CONSTRAINT llm_providers_pkey DO UPDATE SET kind = $2, base_url = $3, models = $4, enabled = $5, updated_at = $6, updated_by = $7")
        .bind(&name).bind(kind).bind(&base_url).bind(Value::Array(models.clone()).to_string()).bind(enabled as i64).bind(&now).bind(&who.sub).execute(&st.db).await?;
    let key_set = match b["api_key"].as_str().map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => { if !st.broker.ready() { return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "the credential broker has no master key".into())); } st.broker.put(&st.db, &secret_name(&name), "llm_key", k, &who.sub).await?; true }
        None => false,
    };
    db::audit(&st.db, &who.sub, "llm.provider.put", &name, json!({ "kind": kind, "base_url": base_url, "models": models, "enabled": enabled, "key_rotated": key_set })).await?;
    resync_schedules(&st).await;
    let p = providers(&st.db).await?.into_iter().find(|p| p.name == name).ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "provider vanished".into()))?;
    let k = has_key(&st.db, &name).await?;
    Ok(Json(provider_json(&p, k)).into_response())
}

/// `DELETE /v1/llm/providers/{name}`: refused while a tier default or a step binding uses it; its key is revoked.
pub(crate) async fn delete_provider(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller) -> Res {
    require_admin(&st, &who, "managing LLM providers")?;
    if managed_provider(&st.db, &name).await? { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is the deployment's (CORI_LLM_PROVIDERS_FILE): remove it from that file"))); }
    let tiers: Vec<String> = sqlx::query("SELECT tier FROM llm_tiers WHERE provider = $1").bind(&name).fetch_all(&st.db).await?.iter().map(|r| r.get("tier")).collect();
    if !tiers.is_empty() { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is the default for tier {}: choose another model there first", tiers.join(", ")))); }
    let bound: Vec<String> = sqlx::query("SELECT DISTINCT app FROM bindings WHERE provider = $1").bind(&name).fetch_all(&st.db).await?.iter().map(|r| r.get("app")).collect();
    if !bound.is_empty() { return Err(ApiError(StatusCode::CONFLICT, format!("steps of {} are bound to `{name}`: rebind them first", bound.join(", ")))); }
    let n = sqlx::query("DELETE FROM llm_providers WHERE name = $1").bind(&name).execute(&st.db).await?.rows_affected();
    if n == 0 { return Err(ApiError(StatusCode::NOT_FOUND, format!("no LLM provider `{name}`"))); }
    sqlx::query("UPDATE secrets SET revoked_at = $1 WHERE scope = '' AND name = $2 AND revoked_at IS NULL").bind(chrono::Utc::now().to_rfc3339()).bind(secret_name(&name)).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "llm.provider.delete", &name, json!({})).await?;
    resync_schedules(&st).await;
    Ok(Json(json!({ "deleted": name })).into_response())
}

/// `GET /v1/llm/providers/{name}/models`: asks the provider for its model list with the stored key; the console uses it to
/// test a key and to offer model ids.
pub(crate) async fn provider_models(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller) -> Res {
    require_admin(&st, &who, "testing an LLM provider")?;
    let p = providers(&st.db).await?.into_iter().find(|p| p.name == name).ok_or(ApiError(StatusCode::NOT_FOUND, format!("no LLM provider `{name}`")))?;
    let key = st.broker.reveal(&st.db, &secret_name(&name)).await.unwrap_or_default();
    let anthropic = p.kind == "anthropic";
    let base = p.base_url.clone().unwrap_or_else(|| if anthropic { "https://api.anthropic.com/v1".into() } else { "https://api.openai.com/v1".into() });
    let url = reqwest::Url::parse(&format!("{}/models{}", base.trim_end_matches('/'), if anthropic { "?limit=1000" } else { "" })).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("base_url `{base}`: {e}")))?;
    // the server calls an address an admin chose: what the registry's egress allows (CORI_REGISTRY_EGRESS)
    let client = cori_engine::egress::client_for(&url, st.cfg.registry_egress(), Some(std::time::Duration::from_secs(15))).await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    let req = if anthropic { client.get(url).header("x-api-key", &key).header("anthropic-version", "2023-06-01") } else { client.get(url).bearer_auth(&key) };
    let res = req.send().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("`{name}` did not answer: {e}")))?;
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if status >= 400 { return Err(ApiError(StatusCode::BAD_GATEWAY, format!("`{name}` answered {status}: {}", v["error"]["message"].as_str().or(v["error"].as_str()).unwrap_or("")))); }
    let mut ids: Vec<String> = v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    ids.sort();
    Ok(Json(json!({ "provider": name, "models": ids })).into_response())
}

/// `PUT /v1/llm/tiers` `{ small: { provider, model } | null, medium: …, large: … }`: the org's default model per tier.
pub(crate) async fn put_tiers(InRealm(st): InRealm, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_admin(&st, &who, "choosing the default models")?;
    let provs = providers(&st.db).await?;
    let now = chrono::Utc::now().to_rfc3339();
    let managed: Vec<String> = sqlx::query_scalar("SELECT tier FROM llm_tiers WHERE updated_by = $1").bind(MANAGED).fetch_all(&st.db).await?;
    let current = tier_defaults(&st.db).await?;
    for tier in TIERS {
        let Some(t) = b.get(tier) else { continue };
        if managed.iter().any(|m| m == tier) {
            let same = current.get(tier).is_some_and(|c| Some(c.provider.as_str()) == t["provider"].as_str() && Some(c.model.as_str()) == t["model"].as_str());
            if !same { return Err(ApiError(StatusCode::CONFLICT, format!("{tier}: the deployment sets its default (CORI_LLM_PROVIDERS_FILE): change it in that file"))); }
            continue;
        }
        if t.is_null() || t["model"].as_str().unwrap_or("").is_empty() { sqlx::query("DELETE FROM llm_tiers WHERE tier = $1").bind(tier).execute(&st.db).await?; continue; }
        let (provider, model) = (t["provider"].as_str().unwrap_or(""), t["model"].as_str().unwrap_or(""));
        // the starter model answers a tier by default already: a tier is set to a provider of the org's own
        if provider == STARTER { return Err(ApiError(StatusCode::BAD_REQUEST, format!("{tier}: the starter model answers every tier you set no default for; clear the tier's default to use it"))); }
        let p = provs.iter().find(|p| p.name == provider).ok_or(ApiError(StatusCode::BAD_REQUEST, format!("{tier}: no LLM provider `{provider}`")))?;
        if !p.models.iter().any(|m| m["id"].as_str() == Some(model)) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("{tier}: `{provider}` does not list the model `{model}`"))); }
        sqlx::query("INSERT INTO llm_tiers (tier, provider, model, updated_at, updated_by) VALUES ($1, $2, $3, $4, $5) ON CONFLICT ON CONSTRAINT llm_tiers_pkey DO UPDATE SET provider = $2, model = $3, updated_at = $4, updated_by = $5").bind(tier).bind(provider).bind(model).bind(&now).bind(&who.sub).execute(&st.db).await?;
    }
    db::audit(&st.db, &who.sub, "llm.tiers", "llm", b.clone()).await?;
    resync_schedules(&st).await;
    let tiers: BTreeMap<String, Value> = tier_defaults(&st.db).await?.into_iter().map(|(t, b)| (t, json!({ "provider": b.provider, "model": b.model }))).collect();
    Ok(Json(json!({ "tiers": tiers })).into_response())
}

/// `PUT /v1/apps/{app}/llm/budget` `{ monthly_eur: number | null }`: the app's monthly LLM budget (null: no limit).
pub(crate) async fn put_budget(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_admin(&st, &who, "setting LLM budgets")?;
    crate::api::require_app(&st, &app).await?;
    let previous: Option<f64> = sqlx::query("SELECT monthly_eur FROM llm_budgets WHERE app = $1").bind(&app).fetch_optional(&st.db).await?.map(|r| r.get("monthly_eur"));
    match &b["monthly_eur"] {
        Value::Null => { sqlx::query("DELETE FROM llm_budgets WHERE app = $1").bind(&app).execute(&st.db).await?; }
        v => {
            let eur = v.as_f64().filter(|e| e.is_finite() && *e >= 0.0).ok_or(ApiError(StatusCode::BAD_REQUEST, "monthly_eur: an amount ≥ 0, or null for no limit".into()))?;
            sqlx::query("INSERT INTO llm_budgets (app, monthly_eur, updated_at, updated_by) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT llm_budgets_pkey DO UPDATE SET monthly_eur = $2, updated_at = $3, updated_by = $4").bind(&app).bind(eur).bind(chrono::Utc::now().to_rfc3339()).bind(&who.sub).execute(&st.db).await?;
        }
    }
    db::audit(&st.db, &who.sub, "llm.budget", &app, json!({ "monthly_eur": b["monthly_eur"], "previous": previous })).await?;
    let mut u = month_usage(&st.db, &app).await?;
    u["app"] = json!(app);
    Ok(Json(u).into_response())
}

/// Schedules carry their workflow input resolved at sync time: a provider or tier change reaches them on a resync.
async fn resync_schedules(st: &Arc<AppState>) {
    let apps: Vec<String> = sqlx::query("SELECT DISTINCT app FROM schedules").fetch_all(&st.db).await.map(|rows| rows.iter().map(|r| r.get("app")).collect()).unwrap_or_default();
    for app in apps { crate::schedules::sync_app(st, &app).await; }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter() -> Starter { Starter { model: "free-model".into(), base_url: "https://models.example/v1".into(), key: "k".into(), per_minute: 2, per_day: 4, note: None } }

    /// A realm's starter calls: a minute's at most, the next one waiting for the oldest to age out (refused when that
    /// takes longer than a lease waits), and a day's at most, refused at once once used.
    #[tokio::test]
    async fn starter_calls_keep_to_a_minute_and_a_day() {
        let Some(db) = crate::db::test_db().await else { return };
        let granted = |at: chrono::DateTime<chrono::Utc>| sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, 'llm.starter', 'r', 'llm:starter', 'w', $2, 'granted')").bind(uuid::Uuid::new_v4().to_string()).bind(at.to_rfc3339());
        let now = chrono::Utc::now();
        // the minute's two calls, one about to age out: a third waits for it, 800 ms after `now`
        granted(now - chrono::Duration::milliseconds(59_200)).execute(&db).await.unwrap();
        granted(now - chrono::Duration::seconds(5)).execute(&db).await.unwrap();
        starter_call(&db, "cori_llm:", &starter(), "r", "w").await.map_err(|e| e.1).unwrap();
        // timed from `now`, as the oldest's age is: slow inserts (a loaded CI database) take from the wait, not from this
        let waited = chrono::Utc::now() - now;
        assert!(waited >= chrono::Duration::milliseconds(800), "it waited for the oldest to age out: {} ms", waited.num_milliseconds());
        // the minute's two now: 5 s old (ages out in 55 s, longer than a lease waits) and the one just granted
        let busy = starter_call(&db, "cori_llm:", &starter(), "r", "w").await.unwrap_err();
        assert_eq!(busy.0, StatusCode::TOO_MANY_REQUESTS, "{}", busy.1);
        // a fourth today: the day's used, refused at once (402: the step fails without retrying)
        granted(today()).execute(&db).await.unwrap();
        let t = std::time::Instant::now();
        let day = starter_call(&db, "cori_llm:", &starter(), "r", "w").await.unwrap_err();
        assert_eq!(day.0, StatusCode::PAYMENT_REQUIRED, "{}", day.1);
        assert!(t.elapsed() < std::time::Duration::from_secs(2) && day.1.contains("midnight UTC"), "{}", day.1);
        assert_eq!(starter_calls_since(&db, today()).await.map_err(|e| e.1).unwrap(), 4);
    }

    /// The starter model answers the tiers the org set no default for, as an org provider: its key leased per call, its
    /// base_url its own; a provider the app declares under its name stays the app's.
    #[tokio::test]
    async fn the_starter_model_fills_the_tiers_the_org_left_open() {
        let Some(db) = crate::db::test_db().await else { return };
        sqlx::query("INSERT INTO llm_providers (name, kind, base_url, models, enabled, updated_at, updated_by) VALUES ('own', 'openai', NULL, '[{\"id\":\"big\"}]', 1, '', 't')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO llm_tiers (tier, provider, model, updated_at, updated_by) VALUES ('large', 'own', 'big', '', 't')").execute(&db).await.unwrap();
        let mut llm = cori_engine::config::ResolvedLlm::default();
        merge_into(&db, Some(&starter()), &mut llm).await.map_err(|e| e.1).unwrap();
        let tier = |t: &str| llm.org_tiers.get(t).map(|b| format!("{}/{}", b.provider, b.model));
        assert_eq!((tier("small"), tier("medium"), tier("large")), (Some("starter/free-model".into()), Some("starter/free-model".into()), Some("own/big".into())));
        let p = &llm.providers[STARTER];
        assert!(p.secret.is_some() && p.base_url.as_deref() == Some("https://models.example/v1") && p.models["free-model"].input_eur_per_mtok == 0.0);
        // without a starter model, the open tiers stay open
        let mut none = cori_engine::config::ResolvedLlm::default();
        merge_into(&db, None, &mut none).await.map_err(|e| e.1).unwrap();
        assert!(!none.org_tiers.contains_key("small") && !none.providers.contains_key(STARTER));
        // an app's own provider of that name is the app's: no deployment key goes to it
        let mut app = cori_engine::config::ResolvedLlm::default();
        app.providers.insert(STARTER.into(), ProviderCfg { base_url: Some("https://app.example/v1".into()), ..Default::default() });
        merge_into(&db, Some(&starter()), &mut app).await.map_err(|e| e.1).unwrap();
        assert!(app.providers[STARTER].secret.is_none() && app.providers[STARTER].base_url.as_deref() == Some("https://app.example/v1"));
    }

    /// Concurrent leases share a budget: what they set aside counts, so they cannot all take its last euros.
    #[tokio::test]
    async fn concurrent_leases_cannot_spend_a_budget_twice() {
        let Some(db) = crate::db::test_db().await else { return };
        sqlx::query("INSERT INTO llm_budgets (app, monthly_eur, updated_at, updated_by) VALUES ('desk', 1.0, '', 't')").execute(&db).await.unwrap();
        crate::usage::add(&db, "llm_eur", "desk", 0.5).await;
        let tries = (0..10).map(|i| { let db = db.clone(); tokio::spawn(async move { reserve(&db, "cori_llm:", 0.0, &format!("run-{i}"), "desk", "openai", 0.1).await.map_err(|e| e.1).unwrap().is_ok() }) });
        let granted = futures::future::join_all(tries).await.into_iter().filter(|r| *r.as_ref().unwrap()).count();
        assert_eq!(granted, 5, "0.5 € left, 0.1 € a call");
        assert!(reserve(&db, "cori_llm:", 0.0, "run-x", "desk", "openai", 0.01).await.map_err(|e| e.1).unwrap().is_err(), "nothing left");
        // a capture releases what its call set aside (an unpriced model costs nothing)
        let granted_run: String = sqlx::query_scalar("SELECT run_id FROM llm_reservations LIMIT 1").fetch_one(&db).await.unwrap();
        settle(&db, &granted_run, "desk", "nope", "openai", "gpt", 10, 10).await.map_err(|e| e.1).unwrap();
        assert!(reserve(&db, "cori_llm:", 0.0, "run-y", "desk", "openai", 0.1).await.map_err(|e| e.1).unwrap().is_ok(), "one call's share is back");
        // the realm's monthly cap, every app together: 0.5 + 1.9 spent, 0.5 set aside
        crate::usage::add(&db, "llm_eur", "crm", 1.9).await;
        assert!(reserve(&db, "cori_llm:", 3.0, "run-c", "crm", "openai", 0.05).await.map_err(|e| e.1).unwrap().is_ok(), "2.95 € of 3");
        assert!(reserve(&db, "cori_llm:", 3.0, "run-d", "crm", "openai", 0.1).await.map_err(|e| e.1).unwrap().is_err(), "3.05 € of 3");
    }
}

// SPDX-License-Identifier: Apache-2.0
//! Realms: the organizations one deployment keeps apart. The core serves a single realm and never names it. A module that
//! hosts several organizations (`Module::tenancy`) tells the server which realm each request acts in, lists the realms for
//! the background work, and admits each one with its key and settings. Everything a request does is then scoped to its
//! realm (`AppState::in_realm`):
//! - the platform database as the realm sees it (`db::Db`): every query sets `cori.realm`, which the module's row-level
//!   security reads;
//! - the realm's broker, sealing its secrets and its runs' tokens with its own key, and its agent tokens' signing key;
//! - its settings (`RealmSettings`);
//! - every name the platform shares with another system (`crate::names`), and Temporal's fairness key, so that one
//!   organization's runs do not starve another's.
//!
//! Where a request's realm comes from (`api::realm_layer`):
//! - a person or a service account of the issuer: their token, through `Tenancy::realm_of` (the organization they signed
//!   in to);
//! - an agent session: the realm its token was minted in, which that realm's key signs;
//! - a worker: the `x-cori-realm` header of the run it works on (`BundleRef::realm`), heard with a worker's credentials only;
//! - a Slack button: the realm written into its value (`names::slack_action`);
//! - the background work: each admitted realm in turn (`AppState::realms`).
//!
//! A request that names no realm acts outside any: the platform database shows it nothing.
pub use crate::db::Realm;
use crate::{ServerConfig, api::ApiError, auth::Principal, db::Db};

pub type TenancyFuture<'a, T> = futures::future::BoxFuture<'a, T>;

/// How a deployment hosting several organizations tells them apart. At most one module of a binary provides it. `db` is
/// always the platform database outside any realm, where the module's own tables are.
pub trait Tenancy: Send + Sync + 'static {
    /// The realm a person or a service account of the issuer acts in: in practice a claim of their token (the organization
    /// they signed in to). The issuer vouches for it, so the module may set the realm up the first time it is named. A
    /// refusal is answered as it is (401 for a token naming no organization).
    fn realm_of<'a>(&'a self, db: &'a Db, who: &'a Principal) -> TenancyFuture<'a, Result<Realm, ApiError>>;

    /// Every realm, for the background work the leading replica does in each (schedules, live stores, run outcomes,
    /// retention).
    fn realms<'a>(&'a self, db: &'a Db) -> TenancyFuture<'a, anyhow::Result<Vec<Realm>>>;

    /// Whether `realm` may act now, with its key and its settings (`defaults`: the deployment's); a refusal (an unknown or
    /// suspended organization) is answered as it is and pauses the realm's background work. Called for every request
    /// and for any realm a request names, before its credentials are checked: a module caches what it looks up, and
    /// never sets a realm up here.
    fn admit<'a>(&'a self, db: &'a Db, realm: &'a Realm, defaults: &'a RealmSettings) -> TenancyFuture<'a, Result<Admitted, ApiError>>;

    /// Why `id` may not name a new app here, beyond the core's own rule (letters, digits, `-`, `_`): an app's names in
    /// this deployment (its front's host, its sign-in client) may need a narrower one.
    fn app_id_problem(&self, _id: &str) -> Option<String> { None }

    /// The realm and app a front host's label names (`CORI_FRONTS_URL`: `<app><suffix>`, the realm's `front_suffix`),
    /// None when it names none of this deployment's. Called for every request to a front host: a module caches it.
    fn front<'a>(&'a self, _db: &'a Db, _label: &'a str) -> TenancyFuture<'a, Option<(Realm, String)>> { Box::pin(async { None }) }

    /// The realm whose apps portal a host's label names under the fronts' domain (`RealmSettings::portal_label`), None
    /// when it names none of this deployment's. Called for every request to such a host: a module caches it.
    fn portal<'a>(&'a self, _db: &'a Db, _label: &'a str) -> TenancyFuture<'a, Option<Realm>> { Box::pin(async { None }) }

    /// Whether `who` may delete the organization they act in (its owner, say): what the console offers them.
    fn may_delete(&self, _who: &Principal) -> bool { false }

    /// Deletes the organization `realm` at `who`'s request (`DELETE /v1/organization?confirm=`, `confirm` naming it as the
    /// module asks): refused from then on, its data purged, its issuer told. The answer the caller gets, or None when this
    /// deployment deletes no organization from its console (the default).
    fn delete<'a>(&'a self, _st: &'a std::sync::Arc<crate::state::AppState>, _realm: &'a Realm, _who: &'a Principal, _confirm: Option<&'a str>) -> TenancyFuture<'a, Option<Result<serde_json::Value, ApiError>>> {
        Box::pin(async { None })
    }
}

/// What the server needs to serve an admitted realm.
pub struct Admitted {
    /// The realm's key: it seals the realm's secrets and its runs' tokens (`secrets::Broker`), and its agent tokens are
    /// signed with a key derived from it.
    pub key: [u8; 32],
    /// The realm's settings (the deployment's, unless the realm has its own).
    pub settings: RealmSettings,
}

/// What may differ between the realms of one deployment. The core's single realm has the deployment's.
#[derive(Debug, Clone)]
pub struct RealmSettings {
    /// Roles that approve a change review or a release going live (`CORI_APPROVER_ROLES`).
    pub approver_roles: Vec<String>,
    /// The person who started a run or parked a change may decide its approval (`CORI_ALLOW_SELF_APPROVAL`): a team too
    /// small for four eyes.
    pub allow_self_approval: bool,
    /// The Slack users mapped to principals (`CORI_SLACK_USERS`: `U0123=sam:support_lead,approver;U0456=jean:it_admin`).
    pub slack_users: String,
    /// What follows an app id in its front's host label (`CORI_FRONTS_URL`): `--<org>` in Cori Cloud, nothing for the
    /// core's single realm (`names::front_label`).
    pub front_suffix: String,
    /// The label of the realm's apps portal under the fronts' domain (`CORI_FRONTS_URL`): its organization's slug in Cori
    /// Cloud (`<slug>.cori.run`); empty for the core's single realm, whose portal is `CORI_PORTAL_URL` when set.
    pub portal_label: String,
    /// The runs the realm may have under way at once, those waiting for a person aside (`CORI_MAX_ACTIVE_RUNS`; 0, no
    /// limit): with the fairness key, what keeps one organization from taking every worker and sandbox.
    pub max_active_runs: usize,
    /// What the realm may use (`Limits`).
    pub limits: Limits,
    /// The realm's tag in the names of its store databases and roles (`names::store_database`, `names::store_role_prefix`):
    /// lowercase letters and digits, and no other realm's on the deployment, so that no two organizations' store objects
    /// share a name on a Postgres server. The tenancy gives it; empty for the core's single realm, whose names carry none.
    pub store_tag: String,
    /// The store cluster the realm's new store databases go to (a name of `ServerConfig::store_clusters`); empty, the
    /// deployment's (`CORI_STORE_PLACEMENT`).
    pub store_cluster: String,
    /// The short name people know the realm's organization by (its slug at the issuer), which the console and the command
    /// line show; empty for the core's single realm, which shows none.
    pub slug: String,
}

/// What a realm may use: under a tenancy its organization's (its plan's), else the deployment's (`CORI_MAX_*`). 0 is no
/// limit. Checked against the usage ledger (`crate::usage`); going over one is refused with 402 (413 for a bundle's size).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Limits {
    /// Apps (`CORI_MAX_APPS`).
    pub apps: u64,
    /// The versions an app keeps (`CORI_MAX_VERSIONS_PER_APP`): another deploy waits until one is deleted (`cori undeploy`).
    pub versions_per_app: u64,
    /// Runs started in a calendar month, UTC (`CORI_MAX_RUNS_PER_MONTH`). Those Temporal Schedules start are counted, not
    /// refused: their schedule bounds them.
    pub runs_per_month: u64,
    /// The size of one version's bundle as uploaded, compressed (`CORI_MAX_BUNDLE_BYTES`).
    pub bundle_bytes: u64,
    /// The realm's store databases, bundles and files together (`CORI_MAX_STORAGE_BYTES`), as last measured: over it,
    /// deploying, uploading files and writing to stores are refused, reading and deleting are not.
    pub storage_bytes: u64,
    /// The most one app may take of it (its store databases, its versions' bundles, its files; `CORI_MAX_STORAGE_BYTES_PER_APP`):
    /// over it, that app's deploys, uploads and store writes are refused. The organization's admins may set an app a lower
    /// limit of its own (`PUT /v1/apps/{app}/limits`, `usage::AppStorageLimit`).
    pub storage_bytes_per_app: u64,
    /// LLM spend in a calendar month, every app together (`CORI_MAX_LLM_EUR_PER_MONTH`), besides each app's own budget.
    pub llm_eur_per_month: f64,
    /// Stores (an app's `stores/<name>/`, a live database once promoted), every app together (`CORI_MAX_STORES`): a version
    /// declaring one more is refused.
    pub stores: u64,
    /// The sandbox databases kept at once (`CORI_MAX_SANDBOX_DATABASES`; a deployed version has one per store): at the limit,
    /// the realm's oldest sandboxes are retired to make room (they come back, seeded again, on their version's next store
    /// call), and a version needing more than the limit alone is refused.
    pub sandbox_databases: u64,
}

impl Limits {
    /// The deployment's, from the environment.
    pub fn from_env() -> Self {
        let n = |var: &str| std::env::var(var).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
        Self {
            apps: n("CORI_MAX_APPS"), versions_per_app: n("CORI_MAX_VERSIONS_PER_APP"), runs_per_month: n("CORI_MAX_RUNS_PER_MONTH"),
            bundle_bytes: n("CORI_MAX_BUNDLE_BYTES"), storage_bytes: n("CORI_MAX_STORAGE_BYTES"), storage_bytes_per_app: n("CORI_MAX_STORAGE_BYTES_PER_APP"),
            llm_eur_per_month: std::env::var("CORI_MAX_LLM_EUR_PER_MONTH").ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0),
            stores: n("CORI_MAX_STORES"), sandbox_databases: n("CORI_MAX_SANDBOX_DATABASES"),
        }
    }

    /// The limits as the usage endpoints show them (`null` for none).
    pub fn json(&self) -> serde_json::Value {
        let n = |v: u64| if v == 0 { serde_json::Value::Null } else { serde_json::json!(v) };
        serde_json::json!({ "apps": n(self.apps), "versions_per_app": n(self.versions_per_app), "runs_per_month": n(self.runs_per_month), "bundle_bytes": n(self.bundle_bytes), "storage_bytes": n(self.storage_bytes), "storage_bytes_per_app": n(self.storage_bytes_per_app), "llm_eur_per_month": if self.llm_eur_per_month > 0.0 { serde_json::json!(self.llm_eur_per_month) } else { serde_json::Value::Null },
                            "stores": n(self.stores), "sandbox_databases": n(self.sandbox_databases) })
    }
}

impl RealmSettings {
    /// The deployment's settings.
    pub fn of(cfg: &ServerConfig) -> Self {
        Self { approver_roles: cfg.approver_roles.clone(), allow_self_approval: cfg.allow_self_approval, slack_users: cfg.slack_users.clone(), front_suffix: String::new(), portal_label: String::new(), max_active_runs: cfg.max_active_runs, limits: cfg.limits.clone(), store_tag: String::new(), store_cluster: String::new(), slug: String::new() }
    }
}

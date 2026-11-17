// SPDX-License-Identifier: Apache-2.0
use crate::{ServerConfig, api::ApiError, auth::Authenticator, db, realm::{Realm, RealmSettings, Tenancy}};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use temporalio_client::{Client, ClientOptions, Connection};

/// The server's state as one request (or one piece of background work) sees it: the deployment's shared parts, and those
/// of the realm it acts in (`in_realm`): the platform database as the realm sees it, the realm's broker and agent-token key,
/// its settings. The core's single realm has the deployment's own.
#[derive(Clone)]
pub struct AppState {
    /// idempotency keys handed from start_run to launch_run (run id → key)
    pub idem: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// in-memory sliding-window rate limits keyed by realm, app, workflow and principal (Redis later, 17)
    pub rate: Arc<Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>>,
    pub cfg: Arc<ServerConfig>,
    /// the platform database: every query goes through it (`db::Db`)
    pub db: db::Db,
    pub auth: Authenticator,
    pub temporal: Arc<Client>,
    pub broker: crate::secrets::Broker,
    /// tokens obtained from the issuer for runs and service accounts
    pub tokens: Arc<crate::identity::TokenCache>,
    /// version bundles: the data dir, and S3 with a bundle store (`bundles::BundleStore`)
    pub bundles: Arc<crate::bundles::BundleStore>,
    /// the files runs are given: their own bucket (`CORI_FILE_STORE`), else the data dir (`bundles::FileStore`)
    pub files: Arc<crate::bundles::FileStore>,
    pub store_tx: tokio::sync::mpsc::UnboundedSender<(crate::stores::Scope, crate::stores::Job)>,
    /// the name, logos and colours people see (`brand`)
    pub brand: Arc<crate::brand::Brand>,
    pub settings: Arc<RealmSettings>,
    /// the organizations a module keeps apart (`crate::realm`); None, the core's single realm
    pub tenancy: Option<Arc<dyn Tenancy>>,
    /// when this process last wrote that a kind of client called, per realm (`api::client_seen`)
    pub seen: Arc<Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// what the identity hook's receiver handles, as its last `ping` said (`client_hook::capabilities`)
    pub hook_caps: Arc<tokio::sync::RwLock<Option<(std::time::Instant, crate::client_hook::Capabilities, bool)>>>,
    /// the receiver's recent answers to questions, per realm (`client_hook::ask_cached`)
    pub answers: Arc<Mutex<std::collections::HashMap<String, (std::time::Instant, serde_json::Value)>>>,
    /// when people left the organization, as this process last read it, per realm and person (`api::removed_at`)
    pub removed: Arc<Mutex<std::collections::HashMap<String, (std::time::Instant, Option<i64>)>>>,
    /// the deployment's connectors and OAuth providers (`CORI_CONNECTORS_FILE`), the same for every realm (`connections`)
    pub deployment: Arc<crate::connections::Deployment>,
}

impl AppState {
    pub async fn new(cfg: ServerConfig) -> Result<Arc<Self>> {
        let brand = crate::brand::Brand::load(cfg.brand_file.as_deref())?;
        let tenancy = cfg.modules.tenancy()?;
        if tenancy.is_some() && cfg.laptop_auth() {
            anyhow::bail!("this binary keeps organizations apart: it needs an issuer whose tokens name the organization (--auth builtin), not `{}` auth", if matches!(cfg.auth, crate::auth::AuthMode::Dev) { "dev" } else { "off" });
        }
        let mut db = db::Db::new(db::connect_with(&cfg.database_url, &cfg.modules.schema()).await?);
        if tenancy.is_some() { db = db.enforcing(); }
        let connection = Connection::connect(cori_engine::runtime::connection_options(&cfg.temporal, "cori-server")?).await
            .map_err(|e| anyhow::anyhow!("could not connect to Temporal at `{}` (namespace `{}`): {e}", cfg.temporal, cfg.namespace))?;
        let temporal = Arc::new(Client::new(connection, ClientOptions::new(cfg.namespace.clone()).build())?);
        let broker = crate::secrets::Broker::open(&cfg.data_dir)?.with_wrapper(cfg.modules.key_wrapper()?);
        let bundles = Arc::new(crate::bundles::BundleStore::from_env(cfg.data_dir.clone())?);
        let files = Arc::new(crate::bundles::FileStore::from_env(cfg.data_dir.clone(), &bundles)?);
        if let Some(why) = files.unavailable() { tracing::warn!("{why}"); }
        let store_tx = crate::stores::spawn_actor(cfg.store_clusters(), bundles.clone());
        // agent tokens are signed with a key derived from the master key: every replica sharing it verifies them
        let auth = Authenticator::new(cfg.auth.clone()).with_agent_key(broker.derive("agent-tokens")).with_session_key(broker.derive("front-sessions")).with_platform_client(cfg.auth_client_id.clone(), cfg.auth_client_secret.clone())
            .with_mcp_audience(format!("{}/mcp", cfg.public_url.trim_end_matches('/')));
        let settings = Arc::new(RealmSettings::of(&cfg));
        // the state of a consent in flight is sealed with the deployment's key: the callback opens it before it knows the realm
        let deployment = Arc::new(crate::connections::Deployment::from_env(broker.derive("connection-state"))?);
        Ok(Arc::new(Self { idem: Default::default(), rate: Default::default(), auth, cfg: Arc::new(cfg), db, temporal, broker, tokens: Default::default(), bundles, files, store_tx, brand: Arc::new(brand), settings, tenancy, seen: Default::default(), hook_caps: Default::default(), answers: Default::default(), removed: Default::default(), deployment }))
    }

    /// The realm this state acts in; None for the core's single realm (and outside any, under a tenancy).
    pub fn realm(&self) -> Option<&Realm> { self.db.realm() }

    /// The origin `app`'s front is served from in this realm (`names::front_origin`).
    pub fn front_origin(&self, app: &str) -> String { crate::names::front_origin(&self.cfg, &self.settings.front_suffix, app) }
    /// The address of `app`'s live front in this realm (`names::front_url`).
    pub fn front_url(&self, app: &str) -> String { crate::names::front_url(&self.cfg, &self.settings.front_suffix, app) }
    /// Where this realm's people find their apps (`names::portal_url`), None without a portal.
    pub fn portal_url(&self) -> Option<String> { crate::names::portal_url(&self.cfg, &self.settings.portal_label) }
    /// The address of one of `app`'s releases in this realm, on its sandbox stores (`names::release_url`).
    pub fn release_url(&self, app: &str, seq: i64) -> String { crate::names::release_url(&self.cfg, &self.settings.front_suffix, app, seq) }
    /// The origin of one of `app`'s releases in this realm (`names::release_origin`).
    pub fn release_origin(&self, app: &str, seq: i64) -> String { crate::names::release_origin(&self.cfg, &self.settings.front_suffix, app, seq) }

    /// This state as `realm` sees it, once the tenancy admits it: its database, broker, agent-token key and settings. A
    /// server without a tenancy serves its single realm only: another is unknown here (404).
    pub async fn in_realm(&self, realm: Realm) -> Result<Arc<AppState>, ApiError> {
        let Some(tenancy) = &self.tenancy else { return Err(ApiError(axum::http::StatusCode::NOT_FOUND, "this server serves a single organization".into())) };
        let admitted = tenancy.admit(&self.db.outside(), &realm, &RealmSettings::of(&self.cfg)).await?;
        let broker = crate::secrets::Broker::from_key(admitted.key).with_wrapper(self.broker.wrapper().cloned());
        let mut s = self.clone();
        s.auth = s.auth.with_agent_key(broker.derive("agent-tokens")).with_session_key(broker.derive("front-sessions"));
        s.broker = broker;
        s.settings = Arc::new(admitted.settings);
        s.db = self.db.in_realm(realm);
        Ok(Arc::new(s))
    }

    /// This state as `realm` sees it without the tenancy's admission: the realm's database, with the deployment's broker and
    /// settings. For a module acting on a realm it does not admit (an export or a deletion of a suspended organization), never
    /// for a request of the realm itself.
    pub fn in_realm_unadmitted(&self, realm: Realm) -> Arc<AppState> {
        let mut s = self.clone();
        s.db = self.db.in_realm(realm);
        Arc::new(s)
    }

    /// The states the background work runs in, one after the other: every admitted realm's under a tenancy (a refused one
    /// is skipped until it is admitted again), else this one.
    pub async fn realms(&self) -> Result<Vec<Arc<AppState>>> {
        let Some(tenancy) = &self.tenancy else { return Ok(vec![Arc::new(self.clone())]) };
        let mut out = Vec::new();
        for realm in tenancy.realms(&self.db.outside()).await? {
            match self.in_realm(realm.clone()).await {
                Ok(s) => out.push(s),
                Err(e) => tracing::debug!(realm = realm.as_str(), reason = %e.1, "realm not admitted: its background work waits"),
            }
        }
        Ok(out)
    }

    pub fn pending_idem(&self, run_id: &str, key: &str) { self.idem.lock().unwrap().insert(run_id.to_string(), key.to_string()); }
    pub fn take_idem(&self, run_id: &str) -> Option<String> { self.idem.lock().unwrap().remove(run_id) }
}

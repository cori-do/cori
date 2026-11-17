// SPDX-License-Identifier: Apache-2.0
//! Store lifecycle on the server. One Postgres database per (app, store, target): the live database serves the promoted
//! version, every deployed version gets its own seeded sandbox, `cori run` gets a local one. The Store executor's
//! futures (`cori-store-engine`) are not `Send`, so every store operation runs on a dedicated thread (an actor owning the
//! Store instances, keyed by cluster and database name) and handlers only await a oneshot reply.
//!
//! Each store database is on one of the deployment's store clusters (`ServerConfig::store_clusters`). Its record
//! (`store_instances`) says which, under which name, and the prefix of the store's Postgres roles: decided once, when the
//! database is created (a realm's new databases on its cluster, named with its store tag, labelled as its own), and read
//! back ever after. A realm's caps on stores and sandbox databases (`realm::Limits`) hold when databases are created.
use crate::auth::Principal;
use anyhow::{Result, anyhow};
use cori_store::{LIVE, LOCAL, Store, StoreDef};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Which database of a store a call addresses: `live`, a version hash (its sandbox), or `local`.
#[derive(Debug, Clone)]
pub struct Target { pub app: String, pub target: String }
impl Target {
    pub fn live(app: &str) -> Self { Self { app: app.into(), target: LIVE.into() } }
    pub fn sandbox(app: &str, hash: &str) -> Self { Self { app: app.into(), target: hash.into() } }
    pub fn is_live(&self) -> bool { self.target == LIVE }
}

/// The realm a store job acts in: its platform database (whose `store_instances` record its store databases), and what its
/// settings say of stores (`realm::RealmSettings`): the tag its new names carry, the cluster its new databases go to, its caps.
#[derive(Clone)]
pub struct Scope {
    pub db: crate::db::Db,
    /// `RealmSettings::store_tag`: empty for the core's single realm.
    pub tag: String,
    /// The store cluster the realm's new databases go to: its own, else the deployment's.
    pub placement: String,
    /// `realm::Limits::stores` and `sandbox_databases` (0, none).
    pub max_stores: u64,
    pub max_sandboxes: u64,
    /// The realm's broker, which keeps each store database's own logins as its app's credentials (`logins`); None for
    /// `cori run`'s local stores, which share the runtime login.
    pub broker: Option<crate::secrets::Broker>,
}

impl Scope {
    pub fn of(st: &crate::state::AppState) -> Self {
        let s = &st.settings;
        let placement = if s.store_cluster.is_empty() { st.cfg.store_placement.clone() } else { s.store_cluster.clone() };
        Self { db: st.db.clone(), tag: s.store_tag.clone(), placement, max_stores: s.limits.stores, max_sandboxes: s.limits.sandbox_databases, broker: Some(st.broker.clone()) }
    }
    /// `cori run`'s local stores: the single realm, the main cluster, no caps.
    fn local(db: crate::db::Db) -> Self { Self { db, tag: String::new(), placement: crate::MAIN_STORE_CLUSTER.into(), max_stores: 0, max_sandboxes: 0, broker: None } }
}

/// The secrets holding a store database's own logins, as its app's credentials: `store-login.<database>.owner`, `.runtime`.
fn login_secrets(db: &str) -> (String, String) {
    let p = crate::secrets::STORE_LOGIN_PREFIX;
    (format!("{p}{db}.owner"), format!("{p}{db}.runtime"))
}

/// What a store database's record says of its logins (`store_instances.logins`): their names.
fn logins_record(db: &str) -> String { json!({ "owner": cori_store::policy::owner_role(db), "runtime": cori_store::policy::runtime_role(db), "group": cori_store::policy::LOGIN_GROUP }).to_string() }

/// The logins `db` is reached with: its own when its record names them (their passwords from its app's credentials), else
/// the shared runtime login (a database made before stores had logins of their own, `cori run`'s local ones). Own logins
/// whose secrets are gone get new passwords (`None`: the caller provisions them, which sets them).
async fn access_of(scope: &Scope, app: &str, db: &str, logins: Option<&str>) -> Result<Option<cori_store::policy::Access>> {
    let (Some(broker), Some(_)) = (&scope.broker, logins) else { return Ok(Some(cori_store::policy::Access::Shared)) };
    let (o, r) = login_secrets(db);
    if !(broker.exists_in(&scope.db, Some(app), &o).await? && broker.exists_in(&scope.db, Some(app), &r).await?) { return Ok(None); }
    let owner = broker.reveal_in(&scope.db, Some(app), &o).await.map_err(|e| anyhow!("the owner login of `{db}`: {e:#}"))?;
    let runtime = broker.reveal_in(&scope.db, Some(app), &r).await.map_err(|e| anyhow!("the runtime login of `{db}`: {e:#}"))?;
    Ok(Some(cori_store::policy::Access::Own { owner, runtime }))
}

/// Keeps a database's own logins (`Access::Own`) as its app's credentials: a new version of each.
async fn keep_logins(scope: &Scope, app: &str, db: &str, access: &cori_store::policy::Access) -> Result<()> {
    let (Some(broker), cori_store::policy::Access::Own { owner, runtime }) = (&scope.broker, access) else { return Ok(()) };
    let (o, r) = login_secrets(db);
    broker.put_in(&scope.db, Some(app), &o, "store_login", owner, "cori").await.map_err(|e| anyhow!("keeping the owner login of `{db}`: {e:#}"))?;
    broker.put_in(&scope.db, Some(app), &r, "store_login", runtime, "cori").await.map_err(|e| anyhow!("keeping the runtime login of `{db}`: {e:#}"))?;
    Ok(())
}

/// The logins a database being provisioned gets: those it has, else new ones of its own (a new database, one made before
/// stores had logins, one whose secrets are gone), except for `cori run`'s local stores. Returns the access and whether
/// it is new (to keep once the database is provisioned).
async fn logins_for(scope: &Scope, at: &Target, db: Option<(&str, Option<&str>)>) -> Result<(cori_store::policy::Access, bool)> {
    if scope.broker.is_none() || at.target == LOCAL { return Ok((cori_store::policy::Access::Shared, false)); }
    if let Some((name, Some(logins))) = db && let Some(a) = access_of(scope, &at.app, name, Some(logins)).await? { return Ok((a, false)); }
    Ok((cori_store::policy::Access::generate(), true))
}

/// Why a store cannot serve a call, as the status every route answers with: `status_of`.
#[derive(Debug)]
pub enum StoreError {
    /// No such store (for this version), or no live database yet: 404.
    NotFound(String),
    /// No store database configured: 503.
    NotConfigured,
    /// The realm's cap on stores or sandbox databases (`realm::Limits`): 402.
    Limit(String),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound(m) => write!(f, "{m}"),
            StoreError::NotConfigured => write!(f, "no store database configured (CORI_STORE_DATABASE_URL)"),
            StoreError::Limit(m) => write!(f, "limit reached: {m}"),
        }
    }
}
impl std::error::Error for StoreError {}

/// The HTTP status of a store failure: 404 unknown store, 402 a cap of the realm's, 503 no store database or the database
/// unreachable (worth a retry), 500 anything else.
pub fn status_of(e: &anyhow::Error) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    match e.downcast_ref::<StoreError>() {
        Some(StoreError::NotFound(_)) => StatusCode::NOT_FOUND,
        Some(StoreError::NotConfigured) => StatusCode::SERVICE_UNAVAILABLE,
        Some(StoreError::Limit(_)) => StatusCode::PAYMENT_REQUIRED,
        None if e.downcast_ref::<cori_store::Unavailable>().is_some() => StatusCode::SERVICE_UNAVAILABLE,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// A failure to provision a target's databases as the API answers it: 402 for a cap of the realm's, else 422 after `what`.
pub fn provisioning_error(e: anyhow::Error, what: &str) -> crate::api::ApiError {
    use axum::http::StatusCode;
    if status_of(&e) == StatusCode::PAYMENT_REQUIRED { return crate::api::ApiError(StatusCode::PAYMENT_REQUIRED, e.to_string()); }
    crate::api::ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("{what}: {e}"))
}

pub enum Job {
    Ping { reply: oneshot::Sender<Result<()>> },
    /// Creates or migrates the databases of every `stores/<name>/` in `bundle_dir` for `target`; seeds them unless live
    /// (`seed_live`: the live ones too, for the dev-mode reset only).
    Provision { at: Target, bundle_dir: PathBuf, seed_live: bool, reply: oneshot::Sender<Result<Vec<Value>>> },
    /// Drops every database of (app, target) and forgets it. Returns the store names dropped. The live target only with
    /// `live` (the dev-mode reset of the live data).
    Drop { at: Target, live: bool, reply: oneshot::Sender<Result<Vec<String>>> },
    /// `inspect`: the caller may fall back to the read-only inspector role (the app's owner, builders, admins).
    Rpc { at: Target, store: String, who: Principal, inspect: bool, req: Value, reply: oneshot::Sender<Result<Value>> },
    /// Applies an approved parked change and marks its approvals row (`approval`) decided, in one step: see `apply`.
    Apply { at: Target, store: String, change: cori_store::ParkedChange, approval: String, by: String, reply: oneshot::Sender<Result<Value>> },
    /// New passwords for the store databases' own logins: every app's, or `app`'s. See `rotate_logins`.
    RotateLogins { app: Option<String>, reply: oneshot::Sender<Result<Vec<Value>>> },
}

/// The store instances the actor has open, by cluster and database name (`key`), with the `updated_at` of their
/// `store_instances` row when they were opened: another replica may since have dropped or provisioned the database again.
/// Borrowed only between awaits.
type Stores = Rc<std::cell::RefCell<HashMap<String, (Rc<Store>, String)>>>;

/// The store clusters by name, their admin URLs (`ServerConfig::store_clusters`).
type Clusters = Rc<BTreeMap<String, String>>;

fn key(cluster: &str, db: &str) -> String { format!("{cluster}/{db}") }

/// The admin URL of store cluster `name`.
fn admin<'a>(clusters: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    if clusters.is_empty() { return Err(StoreError::NotConfigured.into()); }
    clusters.get(name).map(String::as_str).ok_or_else(|| anyhow!("store cluster `{name}` is not configured (CORI_STORE_CLUSTER_{})", name.to_ascii_uppercase()))
}

/// The lock every replica shares (on the platform database) on one target's databases.
fn target_key(platform: &crate::db::Db, at: &Target) -> String {
    let realm = platform.realm().map(|r| format!("{}/", r.as_str())).unwrap_or_default();
    format!("cori_store:{realm}{}:{}", at.app, at.target)
}

/// Creating, migrating or dropping a target's databases holds a lock every replica shares (on the platform database), for
/// as long as a transaction that does nothing else is open: two replicas never provision one database at once.
async fn target_lock(platform: &crate::db::Db, at: &Target) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
    let mut tx = platform.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(target_key(platform, at)).execute(&mut *tx).await?;
    Ok(tx)
}

/// The lock on a store's Postgres roles, which its databases on a cluster share, taken with a target's: they are dropped once
/// no database of the store is left there, never while another target's databases are being given them.
async fn roles_lock(lock: &mut sqlx::Transaction<'static, sqlx::Postgres>, platform: &crate::db::Db, app: &str, store: &str) -> Result<()> {
    let realm = platform.realm().map(|r| format!("{}/", r.as_str())).unwrap_or_default();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(format!("cori_store_roles:{realm}{app}:{store}")).execute(&mut **lock).await?;
    Ok(())
}

/// `bundles` holds the versions: a sandbox whose database was retired is provisioned again from its version's bundle on the next call.
/// Every job runs as its own task on the actor's thread, so a slow query does not hold up other calls. Creating, migrating,
/// dropping or first opening a database goes through one lock (`changes`), so two jobs never provision the same database at once.
/// A job comes with the realm it acts in (`Scope`), whose records say where its databases are.
pub fn spawn_actor(clusters: BTreeMap<String, String>, bundles: Arc<crate::bundles::BundleStore>) -> mpsc::UnboundedSender<(Scope, Job)> {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Scope, Job)>();
    std::thread::Builder::new().name("cori-store".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("store runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let stores: Stores = Rc::new(std::cell::RefCell::new(HashMap::new()));
            let changes = Rc::new(tokio::sync::Mutex::new(()));
            let clusters: Clusters = Rc::new(clusters);
            while let Some((scope, job)) = rx.recv().await {
                let (clusters, bundles, stores, changes) = (clusters.clone(), bundles.clone(), stores.clone(), changes.clone());
                tokio::task::spawn_local(async move {
                    match job {
                        Job::Provision { at, bundle_dir, seed_live, reply } => { let _g = changes.lock().await; let r = provision(&scope, &clusters, &at, &bundle_dir, seed_live, &stores).await; let _ = reply.send(r); }
                        Job::Drop { at, live, reply } => { let _g = changes.lock().await; let r = drop_target(&scope, &clusters, &at, live, &stores).await; let _ = reply.send(r); }
                        Job::Ping { reply } => { let _ = reply.send(ping_clusters(&clusters).await); }
                        Job::RotateLogins { app, reply } => { let _g = changes.lock().await; let r = rotate(&scope, &clusters, app.as_deref(), &stores).await; let _ = reply.send(r); }
                        Job::Apply { at, store, change, approval, by, reply } => {
                            let r = match open(&scope, &clusters, &bundles, &at, &store, &stores, &changes).await { Ok(s) => apply_marked(&scope.db, &s, &change, &approval, &by).await, Err(e) => Err(e) };
                            let _ = reply.send(r);
                        }
                        Job::Rpc { at, store, who, inspect, req, reply } => {
                            let r = match open(&scope, &clusters, &bundles, &at, &store, &stores, &changes).await { Ok(s) => handle_rpc(&scope.db, &at, &s, &who, inspect, &req).await, Err(e) => Err(e) };
                            let _ = reply.send(r);
                        }
                    }
                });
            }
        });
    }).expect("store thread");
    tx
}

/// Every store cluster answers its admin.
async fn ping_clusters(clusters: &BTreeMap<String, String>) -> Result<()> {
    if clusters.is_empty() { return Err(anyhow!("no store database configured")); }
    let mut down = Vec::new();
    for (name, url) in clusters {
        match sqlx::postgres::PgPoolOptions::new().max_connections(1).acquire_timeout(std::time::Duration::from_secs(5)).connect(url).await {
            Ok(p) => p.close().await,
            Err(e) => down.push(format!("{name}: {e}")),
        }
    }
    if down.is_empty() { Ok(()) } else { Err(anyhow!("{}", down.join("; "))) }
}

async fn send<T>(st: &Arc<crate::state::AppState>, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Job) -> Result<T> {
    let (tx, rx) = oneshot::channel();
    st.store_tx.send((Scope::of(st), make(tx))).map_err(|_| anyhow!("store actor gone"))?;
    rx.await.map_err(|_| anyhow!("store actor dropped the reply"))?
}

/// Provisions the stores of a bundle for a target: the live databases at promote (migrations only), a version's sandbox at
/// deploy (migrations + seeds). Returns one report per store.
pub async fn provision_bundle(st: &Arc<crate::state::AppState>, at: Target, bundle_dir: PathBuf) -> Result<Vec<Value>> { send(st, |reply| Job::Provision { at, bundle_dir, seed_live: false, reply }).await }
/// Dev auth only (`reset_live_stores`): provisions the live databases of an app from a bundle with its seed files, like a sandbox.
pub async fn provision_live_seeded(st: &Arc<crate::state::AppState>, app: &str, bundle_dir: PathBuf) -> Result<Vec<Value>> { send(st, |reply| Job::Provision { at: Target::live(app), bundle_dir, seed_live: true, reply }).await }
pub async fn drop_sandbox(st: &Arc<crate::state::AppState>, at: Target) -> Result<Vec<String>> { send(st, |reply| Job::Drop { at, live: false, reply }).await }
/// Dev auth only (`reset_live_stores`): drops the live databases of an app so that they can be provisioned again, empty.
pub async fn drop_live(st: &Arc<crate::state::AppState>, app: &str) -> Result<Vec<String>> { send(st, |reply| Job::Drop { at: Target::live(app), live: true, reply }).await }
pub async fn ping(st: &Arc<crate::state::AppState>) -> Result<()> { send(st, |reply| Job::Ping { reply }).await }
/// New passwords for the store databases' own logins (every app's, or `app`'s): `cori stores rotate-logins`.
pub async fn rotate_logins(st: &Arc<crate::state::AppState>, app: Option<String>) -> Result<Vec<Value>> { send(st, |reply| Job::RotateLogins { app, reply }).await }
/// Applies an approved parked change (kept in its approvals row `approval`) and marks the row decided: the row goes from
/// pending to decided inside the change's transaction, before it commits, so that one decision applies it once however
/// many servers share the approvals table (another decision finds the row decided: `DecideError::Decided`).
pub async fn apply(st: &Arc<crate::state::AppState>, at: Target, store: String, change: cori_store::ParkedChange, approval: String, by: String) -> Result<Value> {
    send(st, |reply| Job::Apply { at, store, change, approval, by, reply }).await
}

async fn apply_marked(platform: &crate::db::Db, store: &Store, change: &cori_store::ParkedChange, approval: &str, by: &str) -> Result<Value> {
    let marked = std::cell::Cell::new(false);
    let r = store.apply_parked(change, || async {
        let n = sqlx::query("UPDATE approvals SET status = 'decided', decided_by = $1, decided_at = $2 WHERE id = $3 AND status = 'pending'")
            .bind(by).bind(chrono::Utc::now().to_rfc3339()).bind(approval).execute(platform).await?.rows_affected();
        if n != 1 { return Err(cori_store::DecideError::Decided(approval.to_string()).into()); }
        marked.set(true);
        Ok(())
    }).await;
    // marked, then the commit failed: nothing applied, so the change waits again
    if r.is_err() && marked.get() {
        let _ = sqlx::query("UPDATE approvals SET status = 'pending', decided_by = NULL, decided_at = NULL WHERE id = $1 AND status = 'decided'").bind(approval).execute(platform).await;
    }
    r
}
/// A store call as `who`'s store role; with `inspect`, a caller holding none of the store's roles reads as the inspector.
/// Over the realm's storage limit (`realm::Limits::storage_bytes`), or its app's (`usage::AppStorageLimit`), a call that
/// adds data is refused as a tool error: reading, deleting and rejecting stay possible.
pub async fn rpc(st: &Arc<crate::state::AppState>, at: Target, store: String, who: Principal, inspect: bool, req: Value) -> Result<Value> {
    let tool = req["params"]["name"].as_str().unwrap_or("");
    let grows = req["method"] == "tools/call" && !req["params"]["arguments"]["dryRun"].as_bool().unwrap_or(false)
        && ["create", "update", "approve"].iter().any(|p| tool.strip_prefix(p).and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_uppercase()));
    if grows && let Some(why) = crate::usage::storage_full(st, &at.app).await.map_err(|e| anyhow!(e.1))? {
        return Ok(json!({ "jsonrpc": "2.0", "id": req.get("id").cloned().unwrap_or(Value::Null), "result": { "content": [{ "type": "text", "text": why }], "isError": true } }));
    }
    send(st, |reply| Job::Rpc { at, store, who, inspect, req, reply }).await
}


/// At startup: the live stores a promoted release declares but that have no database (the server ran without a store
/// database when it was promoted, or it was pointed at a new one) are created and migrated now, never seeded. Provisioning
/// is idempotent, so a store that exists is left as is.
pub async fn provision_missing_live(st: &Arc<crate::state::AppState>) {
    if st.cfg.store_database_url.is_none() { return; }
    let apps = match sqlx::query("SELECT id, promoted_hash FROM apps WHERE promoted_hash IS NOT NULL").fetch_all(&st.db).await { Ok(r) => r, Err(e) => { tracing::warn!("live stores at startup: {e}"); return; } };
    for r in apps {
        let (app, hash): (String, String) = (r.get("id"), r.get("promoted_hash"));
        let dir = match st.bundles.dir(st.db.realm(), &hash).await { Ok(d) => d, Err(e) => { tracing::warn!(app = %app, error = %e, "live stores at startup: bundle"); continue } };
        let declared: Vec<String> = read_store_dirs(&dir).into_iter().map(|d| d.name).collect();
        if declared.is_empty() { continue; }
        let have: Vec<String> = sqlx::query("SELECT name FROM store_instances WHERE app = $1 AND target = 'live'").bind(&app).fetch_all(&st.db).await.map(|rs| rs.iter().map(|r| r.get::<String, _>("name")).collect()).unwrap_or_default();
        if declared.iter().all(|d| have.contains(d)) { continue; }
        match provision_bundle(st, Target::live(&app), dir).await {
            Ok(report) => tracing::info!(app = %app, stores = ?report.iter().filter_map(|x| x["store"].as_str()).collect::<Vec<_>>(), "live stores created for the promoted release"),
            Err(e) => tracing::warn!(app = %app, "live stores of the promoted release could not be created: {e}"),
        }
    }
}

/// One `stores/<name>/` folder of a bundle: rules, roles, migrations and seeds, each sorted by file name.
pub struct StoreDir { pub name: String, pub rules: String, pub roles: BTreeMap<String, String>, pub migrations: Vec<(String, String)>, pub seeds: Vec<(String, String)> }

/// Reads `stores/<name>/{rules.yaml, roles/*.yaml, migrations/*.sql, seed/*.sql}` from a bundle dir.
pub fn read_store_dirs(bundle_dir: &Path) -> Vec<StoreDir> {
    let sql_files = |dir: PathBuf| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) { for m in rd.filter_map(|m| m.ok()) { if m.path().extension().map(|x| x == "sql").unwrap_or(false) { out.push((m.file_name().to_string_lossy().to_string(), std::fs::read_to_string(m.path()).unwrap_or_default())); } } }
        out.sort();
        out
    };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(bundle_dir.join("stores")) else { return out };
    for e in rd.filter_map(|e| e.ok()) {
        if !e.path().is_dir() { continue; }
        let name = e.file_name().to_string_lossy().to_string();
        let rules = std::fs::read_to_string(e.path().join("rules.yaml")).unwrap_or_default();
        let mut roles = BTreeMap::new();
        if let Ok(rr) = std::fs::read_dir(e.path().join("roles")) { for r in rr.filter_map(|r| r.ok()) { if let Some(n) = r.path().file_stem().map(|s| s.to_string_lossy().to_string()) { if let Ok(y) = std::fs::read_to_string(r.path()) { roles.insert(n, y); } } } }
        out.push(StoreDir { name, rules, roles, migrations: sql_files(e.path().join("migrations")), seeds: sql_files(e.path().join("seed")) });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}


async fn provision(scope: &Scope, clusters: &Clusters, at: &Target, bundle_dir: &Path, seed_live: bool, stores: &Stores) -> Result<Vec<Value>> {
    let platform = &scope.db;
    let dirs = read_store_dirs(bundle_dir);
    if dirs.is_empty() { return Ok(vec![]); }
    // which of a caller's roles counts in the stores: the first the app lists
    let role_order = cori_compiler::roles::declared_order(&std::fs::read_to_string(bundle_dir.join("cori.toml")).unwrap_or_default());
    if clusters.is_empty() { tracing::warn!("bundle declares stores but no store database is configured; store steps fall back to dev mocks"); return Ok(vec![json!({ "warning": "no store database configured (CORI_STORE_DATABASE_URL); stores not provisioned" })]) }
    let mut lock = target_lock(platform, at).await?;
    make_room(scope, clusters, at, &dirs, stores, &mut lock).await?;
    let mut report = Vec::new();
    for d in dirs {
        roles_lock(&mut lock, platform, &at.app, &d.name).await?;
        let seeds: &[(String, String)] = if at.is_live() && !seed_live { &[] } else { &d.seeds };
        let label = crate::names::store_label(platform.realm(), &at.app, &d.name, &at.target);
        let row = sqlx::query("SELECT db_name, cluster, role_prefix, logins FROM store_instances WHERE app = $1 AND name = $2 AND target = $3").bind(&at.app).bind(&d.name).bind(&at.target).fetch_optional(platform).await?;
        let (db, cluster, prefix, p, access) = match row {
            // the store's database, where and as its record has it; with logins of its own from now on
            Some(r) => {
                let (db, cluster, prefix, logins): (String, String, String, Option<String>) = (r.get("db_name"), r.get("cluster"), r.get("role_prefix"), r.get("logins"));
                let prefix = if prefix.is_empty() { cori_store::policy::role_prefix(&at.app, &d.name) } else { prefix };
                let claim = cori_store::Claim { label, adopt_unlabelled: true };
                let (access, new) = logins_for(scope, at, Some((&db, logins.as_deref()))).await?;
                let p = cori_store::provision_with(admin(clusters, &cluster)?, &db, &claim, &access, &d.migrations, seeds).await.map_err(|e| anyhow!("store `{}`: {e}", d.name))?;
                if new { keep_logins(scope, &at.app, &db, &access).await?; }
                (db, cluster, prefix, p, access)
            }
            // a new one, on the realm's cluster and named with its tag; a name another store's database holds gives way to a
            // variant, never taken over
            None => {
                if platform.realm().is_some() && let Some(why) = crate::names::store_tag_problem(&scope.tag) {
                    return Err(anyhow!("store `{}`: this organization has no store tag of its own ({why}), so its databases could share names with another's", d.name));
                }
                let cluster = scope.placement.clone();
                let admin_url = admin(clusters, &cluster)?;
                // the core's single realm takes over a database of its name that predates labels (a platform database pointed
                // at an existing store server); a realm never does
                let claim = cori_store::Claim { label: label.clone(), adopt_unlabelled: platform.realm().is_none() };
                let name = crate::names::store_database(&scope.tag, &at.app, &d.name, &at.target);
                let (access, new) = logins_for(scope, at, None).await?;
                let (db, p) = match cori_store::provision_with(admin_url, &name, &claim, &access, &d.migrations, seeds).await {
                    Err(e) if e.downcast_ref::<cori_store::Taken>().is_some() => {
                        let other = cori_store::db_name_for(&name, &label);
                        tracing::warn!(store = %d.name, database = %name, instead = %other, "{e}");
                        let p = cori_store::provision_with(admin_url, &other, &claim, &access, &d.migrations, seeds).await.map_err(|e| anyhow!("store `{}`: {e}", d.name))?;
                        (other, p)
                    }
                    r => (name, r.map_err(|e| anyhow!("store `{}`: {e}", d.name))?),
                };
                if new { keep_logins(scope, &at.app, &db, &access).await?; }
                (db, cluster, crate::names::store_role_prefix(&scope.tag, &at.app, &d.name), p, access)
            }
        };
        let logins = access.is_own().then(|| logins_record(&db));
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO store_instances (app, name, target, db_name, schema_yaml, rules_yaml, roles_yaml, seeded, created_at, updated_at, cluster, role_prefix, logins, role_order) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $11, $12, $13) ON CONFLICT ON CONSTRAINT store_instances_pkey DO UPDATE SET db_name = $4, schema_yaml = $5, rules_yaml = $6, roles_yaml = $7, seeded = $8, updated_at = $9, cluster = $10, role_prefix = $11, logins = $12, role_order = $13")
            .bind(&at.app).bind(&d.name).bind(&at.target).bind(&db).bind(&p.schema_yaml).bind(&d.rules).bind(serde_json::to_string(&d.roles)?).bind(if seeds.is_empty() { 0i64 } else { 1 }).bind(&now).bind(&cluster).bind(&prefix).bind(&logins).bind(serde_json::to_string(&role_order)?).execute(platform).await?;
        let stale = stores.borrow_mut().remove(&key(&cluster, &db));   // reopened with the new definition on next use
        if let Some((s, _)) = stale { s.pool.close().await; }
        // the access policy (grants, row level security) of the new definition, now rather than at the first call
        let def = StoreDef { name: d.name.clone(), db_name: db.clone(), schema_yaml: p.schema_yaml.clone(), rules_yaml: d.rules.clone(), roles_yaml: d.roles.clone(), role_prefix: prefix, role_order: role_order.clone() };
        let policy_warnings = cori_store::policy::apply_def_with(admin(clusters, &cluster)?, &at.app, &def, &access).await.map_err(|e| anyhow!("store `{}`: access policy: {e}", d.name))?;
        report.push(json!({ "store": d.name, "target": at.target, "database": db, "cluster": cluster, "created": p.created, "migrations_applied": p.migrations_applied, "seeds_applied": p.seeds_applied, "changed": p.changed, "seeds": seeds.len(), "roles": d.roles.keys().collect::<Vec<_>>(), "policy_warnings": policy_warnings, "logins": if access.is_own() { json!({ "runtime": access.runtime_login(&db), "owner": cori_store::policy::owner_role(&db) }) } else { json!("shared") } }));
    }
    lock.commit().await?;
    Ok(report)
}

/// New passwords for the store databases' own logins (every app's, or `app`'s), one database at a time under its target's
/// lock: each a new version of its app's credentials, then set on the database by the admin (the secrets go back to the
/// old passwords when that fails). The record's `updated_at` moves, so every replica reopens its pool with the new ones;
/// connections already open keep working. Databases that still share the runtime login are left as they are: their next
/// provisioning gives them logins of their own.
async fn rotate(scope: &Scope, clusters: &Clusters, app: Option<&str>, stores: &Stores) -> Result<Vec<Value>> {
    let platform = &scope.db;
    if scope.broker.is_none() { return Err(anyhow!("no broker to keep store logins in")); }
    let rows: Vec<(String, String, String, String, String)> = sqlx::query("SELECT app, name, target, db_name, cluster FROM store_instances WHERE logins IS NOT NULL AND target <> $1 AND ($2 = '' OR app = $2) ORDER BY app, name, target")
        .bind(LOCAL).bind(app.unwrap_or("")).fetch_all(platform).await?
        .iter().map(|r| (r.get("app"), r.get("name"), r.get("target"), r.get("db_name"), r.get("cluster"))).collect();
    let mut out = Vec::new();
    for (a, name, target, db, cluster) in rows {
        let lock = target_lock(platform, &Target { app: a.clone(), target: target.clone() }).await?;
        let old = access_of(scope, &a, &db, Some("own")).await?;
        let new = cori_store::policy::Access::generate();
        keep_logins(scope, &a, &db, &new).await?;
        if let Err(e) = cori_store::policy::set_passwords(admin(clusters, &cluster)?, &db, &new).await {
            if let Some(old) = old { let _ = keep_logins(scope, &a, &db, &old).await; }
            return Err(e.context(format!("store `{name}` of `{a}` ({target})")));
        }
        sqlx::query("UPDATE store_instances SET updated_at = $1 WHERE app = $2 AND name = $3 AND target = $4").bind(chrono::Utc::now().to_rfc3339()).bind(&a).bind(&name).bind(&target).execute(platform).await?;
        let stale = stores.borrow_mut().remove(&key(&cluster, &db));
        if let Some((s, _)) = stale { s.pool.close().await; }
        lock.commit().await?;
        out.push(json!({ "app": a, "store": name, "target": target, "database": db, "logins": [cori_store::policy::owner_role(&db), cori_store::policy::runtime_role(&db)] }));
    }
    Ok(out)
}

/// The realm's caps, before `at`'s databases are created (`lock`: the target's). A store more than `max_stores` (an app's
/// `stores/<name>/`, every app together) is refused; at `max_sandboxes`, the realm's oldest sandboxes are retired to make
/// room (a promoted version's last; each comes back, seeded again, on its version's next store call), and a version needing
/// more sandbox databases than the cap alone is refused.
async fn make_room(scope: &Scope, clusters: &Clusters, at: &Target, dirs: &[StoreDir], stores: &Stores, lock: &mut sqlx::Transaction<'static, sqlx::Postgres>) -> Result<()> {
    let platform = &scope.db;
    if at.target == LOCAL || (scope.max_stores == 0 && scope.max_sandboxes == 0) { return Ok(()); }
    // a realm's databases are counted and created one target at a time
    let realm = platform.realm().map(|r| r.as_str().to_string()).unwrap_or_default();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(format!("cori_store_caps:{realm}")).execute(&mut **lock).await?;
    if scope.max_stores > 0 {
        let mut all: BTreeSet<(String, String)> = sqlx::query("SELECT DISTINCT app, name FROM store_instances WHERE target <> $1").bind(LOCAL).fetch_all(platform).await?
            .iter().map(|r| (r.get("app"), r.get("name"))).collect();
        let before = all.len();
        all.extend(dirs.iter().map(|d| (at.app.clone(), d.name.clone())));
        if all.len() > before && all.len() as u64 > scope.max_stores {
            return Err(StoreError::Limit(format!("this organization may have {} (an app's `stores/<name>/`, every app together); `{}` would make it {}", count(scope.max_stores, "store"), at.app, all.len())).into());
        }
    }
    if scope.max_sandboxes > 0 && !at.is_live() {
        let have: BTreeSet<String> = sqlx::query_scalar("SELECT name FROM store_instances WHERE app = $1 AND target = $2").bind(&at.app).bind(&at.target).fetch_all(platform).await?.into_iter().collect();
        let new = dirs.iter().filter(|d| !have.contains(&d.name)).count() as u64;
        if new == 0 { return Ok(()); }
        let short = at.target.trim_start_matches("sha256:").get(..12).unwrap_or(&at.target).to_string();
        if dirs.len() as u64 > scope.max_sandboxes {
            return Err(StoreError::Limit(format!("version {short} of `{}` needs {} sandbox databases (one per store), and this organization keeps at most {}", at.app, dirs.len(), count(scope.max_sandboxes, "sandbox database"))).into());
        }
        let mut kept = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM store_instances WHERE target NOT IN ('live', 'local')").fetch_one(platform).await? as u64;
        if kept + new <= scope.max_sandboxes { return Ok(()); }
        let victims = sqlx::query("SELECT s.app, s.target, COUNT(*) AS n FROM store_instances s LEFT JOIN apps a ON a.id = s.app WHERE s.target NOT IN ('live', 'local') AND NOT (s.app = $1 AND s.target = $2) GROUP BY s.app, s.target ORDER BY COALESCE(BOOL_OR(a.promoted_hash = s.target), false), MAX(s.updated_at)")
            .bind(&at.app).bind(&at.target).fetch_all(platform).await?;
        for v in victims {
            if kept + new <= scope.max_sandboxes { break; }
            let (app, target, n): (String, String, i64) = (v.get("app"), v.get("target"), v.get("n"));
            match drop_target_now(scope, clusters, &Target::sandbox(&app, &target), stores).await {
                Ok(Some(names)) => { kept = kept.saturating_sub(n as u64); tracing::info!(app, version = %target, stores = ?names, "sandbox retired: the organization's sandbox databases are at their limit"); }
                Ok(None) => {}   // another replica is working on it
                Err(e) => tracing::warn!(app, version = %target, error = %e, "sandbox retirement failed"),
            }
        }
        if kept + new > scope.max_sandboxes {
            return Err(StoreError::Limit(format!("this organization keeps at most {}, and the others are in use: try again", count(scope.max_sandboxes, "sandbox database"))).into());
        }
    }
    Ok(())
}

/// `n` things: `1 store`, `3 stores`.
fn count(n: u64, one: &str) -> String { if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") } }

async fn drop_target(scope: &Scope, clusters: &Clusters, at: &Target, live: bool, stores: &Stores) -> Result<Vec<String>> {
    if at.is_live() && !live { return Err(anyhow!("the live databases are never dropped")); }
    if clusters.is_empty() { return Err(StoreError::NotConfigured.into()); }
    let lock = target_lock(&scope.db, at).await?;
    drop_locked(scope, clusters, at, stores, lock).await
}

/// `drop_target` of a sandbox no replica is working on now (None when one is): what the caps retire.
async fn drop_target_now(scope: &Scope, clusters: &Clusters, at: &Target, stores: &Stores) -> Result<Option<Vec<String>>> {
    let mut tx = scope.db.begin().await?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))").bind(target_key(&scope.db, at)).fetch_one(&mut *tx).await?;
    if !got { return Ok(None); }
    drop_locked(scope, clusters, at, stores, tx).await.map(Some)
}

/// Drops a target's databases, each on its cluster, under the target's `lock`; then the Postgres roles of each store left
/// without a database on that cluster.
async fn drop_locked(scope: &Scope, clusters: &Clusters, at: &Target, stores: &Stores, mut lock: sqlx::Transaction<'static, sqlx::Postgres>) -> Result<Vec<String>> {
    let platform = &scope.db;
    let rows: Vec<(String, String, String, String)> = sqlx::query("SELECT name, db_name, cluster, role_prefix FROM store_instances WHERE app = $1 AND target = $2 ORDER BY name").bind(&at.app).bind(&at.target).fetch_all(platform).await?
        .iter().map(|r| (r.get("name"), r.get("db_name"), r.get("cluster"), r.get("role_prefix"))).collect();
    let mut dropped = Vec::new();
    for (name, db, cluster, _) in &rows {
        let open_store = stores.borrow_mut().remove(&key(cluster, db));
        if let Some((s, _)) = open_store { s.pool.close().await; }
        cori_store::drop_database(admin(clusters, cluster)?, db).await.map_err(|e| anyhow!("store `{name}`: {e}"))?;
        // its logins went with it: so do their passwords
        if let Some(b) = &scope.broker { b.forget_prefix(platform, Some(&at.app), &format!("{}{db}.", crate::secrets::STORE_LOGIN_PREFIX)).await?; }
        dropped.push(name.clone());
    }
    sqlx::query("DELETE FROM store_instances WHERE app = $1 AND target = $2").bind(&at.app).bind(&at.target).execute(platform).await?;
    // the changes parked on the dropped databases were read from rows that are gone (a reseed brings back look-alikes)
    let prefix = format!("store:{}:{}:", at.app, at.target.trim_start_matches("sha256:"));
    sqlx::query("UPDATE approvals SET status = 'withdrawn', decided_by = 'cori', decided_at = $1 WHERE app = $2 AND status = 'pending' AND substr(id, 1, $3) = $4")
        .bind(chrono::Utc::now().to_rfc3339()).bind(&at.app).bind(prefix.len() as i32).bind(&prefix).execute(platform).await?;   // substr takes an integer on Postgres
    // a store without a database left on a cluster: its roles there go too
    for (name, _, cluster, role_prefix) in &rows {
        roles_lock(&mut lock, platform, &at.app, name).await?;
        let left = sqlx::query("SELECT 1 FROM store_instances WHERE app = $1 AND name = $2 AND cluster = $3 LIMIT 1").bind(&at.app).bind(name).bind(cluster).fetch_optional(platform).await?.is_some();
        if left { continue; }
        let role_prefix = if role_prefix.is_empty() { cori_store::policy::role_prefix(&at.app, name) } else { role_prefix.clone() };
        match cori_store::drop_roles(admin(clusters, cluster)?, &role_prefix).await {
            Ok(roles) if !roles.is_empty() => tracing::info!(app = %at.app, store = %name, cluster = %cluster, roles = roles.len(), "the store's roles dropped with its last database there"),
            Ok(_) => {}
            Err(e) => tracing::warn!(app = %at.app, store = %name, cluster = %cluster, error = %e, "the store's roles were not dropped"),
        }
    }
    lock.commit().await?;
    Ok(dropped)
}

async fn open(scope: &Scope, clusters: &Clusters, bundles: &crate::bundles::BundleStore, at: &Target, name: &str, stores: &Stores, changes: &tokio::sync::Mutex<()>) -> Result<Rc<Store>> {
    if clusters.is_empty() { return Err(StoreError::NotConfigured.into()); }
    let platform = &scope.db;
    let record = || async { sqlx::query("SELECT db_name, cluster, role_prefix, schema_yaml, rules_yaml, roles_yaml, updated_at, logins, role_order FROM store_instances WHERE app = $1 AND name = $2 AND target = $3").bind(&at.app).bind(name).bind(&at.target).fetch_optional(platform).await };
    // an open store serves while its row is the one it was opened from: another replica may have dropped or provisioned its
    // database again since (retention, the caps, a reseed, a promotion)
    let cached = |row: &Option<sqlx::postgres::PgRow>| row.as_ref().and_then(|r| {
        let k = key(&r.get::<String, _>("cluster"), &r.get::<String, _>("db_name"));
        stores.borrow().get(&k).filter(|(_, opened_at)| *opened_at == r.get::<String, _>("updated_at")).map(|(s, _)| s.clone())
    });
    if let Some(s) = cached(&record().await?) { return Ok(s); }
    // first use: one opener at a time (it may provision a retired sandbox); another job may have opened it meanwhile
    let _g = changes.lock().await;
    let mut row = record().await?;
    if let Some(s) = cached(&row) { return Ok(s); }
    let exists = match &row {
        Some(r) => at.is_live() || at.target == LOCAL || cori_store::database_exists(admin(clusters, &r.get::<String, _>("cluster"))?, &r.get::<String, _>("db_name")).await?,
        None => false,
    };
    // a sandbox that retention or the caps dropped (or that predates this server) comes back from its version's bundle, seeded again
    if !exists {
        if at.is_live() || at.target == LOCAL {
            return Err(StoreError::NotFound(if at.is_live() { format!("store `{name}` of `{}` has no live database yet: promote a version that declares it", at.app) } else { format!("unknown store `{name}` for `{}` ({})", at.app, at.target) }).into());
        }
        let bundle_dir = bundles.dir(platform.realm(), &at.target).await?;
        if !bundle_dir.join("stores").join(name).is_dir() { return Err(StoreError::NotFound(format!("unknown store `{name}` in version {} of `{}`", at.target, at.app)).into()); }
        tracing::info!(app = %at.app, store = name, version = %at.target, "sandbox provisioned on demand");
        provision(scope, clusters, at, &bundle_dir, false, stores).await?;
        row = record().await?;
    }
    let r = row.ok_or_else(|| StoreError::NotFound(format!("unknown store `{name}`")))?;
    let (db, cluster): (String, String) = (r.get("db_name"), r.get("cluster"));
    let stale = stores.borrow_mut().remove(&key(&cluster, &db));
    if let Some((s, _)) = stale { s.pool.close().await; }
    let def = StoreDef { name: name.to_string(), db_name: db.clone(), schema_yaml: r.get("schema_yaml"), rules_yaml: r.get("rules_yaml"), roles_yaml: serde_json::from_str(&r.get::<String, _>("roles_yaml")).unwrap_or_default(), role_prefix: r.get("role_prefix"), role_order: serde_json::from_str(&r.get::<String, _>("role_order")).unwrap_or_default() };
    let access = match access_of(scope, &at.app, &db, r.get::<Option<String>, _>("logins").as_deref()).await? {
        Some(a) => a,
        // its logins' secrets are gone: new passwords, set on the database before it opens
        None => {
            let a = cori_store::policy::Access::generate();
            keep_logins(scope, &at.app, &db, &a).await?;
            cori_store::policy::set_passwords(admin(clusters, &cluster)?, &db, &a).await.map_err(|e| e.context(format!("store `{name}`")))?;
            a
        }
    };
    let store = Rc::new(Store::open_with(admin(clusters, &cluster)?, &at.app, &def, &access).await.map_err(|e| e.context(format!("store `{name}`")))?);
    stores.borrow_mut().insert(key(&cluster, &db), (store.clone(), r.get("updated_at")));
    Ok(store)
}

/// A caller that is not a workflow run (a person in the console, a front calling the store) whose store role writes through
/// workflows only; store administrators keep direct access. An agent session never writes (`stores:read`): its changes
/// go through workflow runs on a preview.
fn direct_writes_refused(store: &Store, role: Option<&str>, who: &Principal) -> bool {
    who.is_agent() || (who.kind != crate::auth::PrincipalKind::Service && !who.is_store_admin() && role.map(|r| store.writes_through_workflows_only(r)).unwrap_or(false))
}

/// Which store role a principal acts as: of their roles the store defines, the first the app's cori.toml lists (never the
/// inspector, which no identity can carry), else the read-only inspector when the server allows it (`inspect`: the app's
/// owner, builders and admins).
pub fn role_for(store: &Store, who: &Principal, inspect: bool) -> Option<String> {
    store.role_of(&who.roles).or_else(|| inspect.then(|| cori_store::INSPECTOR.to_string()))
}

/// The platform approval id of a parked store change: `store:<app>:<target>:<store>:<approval_id>`, the target written
/// without the `sha256:` prefix so that the id splits on colons.
pub fn approval_id(at: &Target, store: &str, id: &str) -> String { format!("store:{}:{}:{store}:{id}", at.app, at.target.trim_start_matches("sha256:")) }
/// Parses `store:<app>:<target>:<store>:<approval_id>` back (the approval id itself may hold no colon).
pub fn parse_approval_id(id: &str) -> Option<(Target, String, String)> {
    let rest = id.strip_prefix("store:")?;
    let mut it = rest.splitn(4, ':');
    let (app, target, store, aid) = (it.next()?, it.next()?, it.next()?, it.next()?);
    let target = if target == LIVE || target == LOCAL { target.to_string() } else { format!("sha256:{target}") };
    Some((Target { app: app.into(), target }, store.into(), aid.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_failures_answer_with_their_status() {
        use axum::http::StatusCode;
        assert_eq!(status_of(&StoreError::NotFound("unknown store `x`".into()).into()), StatusCode::NOT_FOUND);
        assert_eq!(status_of(&StoreError::NotConfigured.into()), StatusCode::SERVICE_UNAVAILABLE);
        // the cause survives the context `open` adds
        assert_eq!(status_of(&anyhow::Error::new(cori_store::Unavailable("store connection: refused".into())).context("store `s`")), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(status_of(&anyhow!("the database's access policy could not be applied")), StatusCode::INTERNAL_SERVER_ERROR);
    }
    /// A bundle of app `app` declaring `stores` (each a table `items` with a row, readable by role `member`).
    fn tmp() -> PathBuf { let d = std::env::temp_dir().join(format!("cori-test-{}", uuid::Uuid::new_v4().simple())); std::fs::create_dir_all(&d).unwrap(); d }

    fn bundle(stores: &[&str]) -> PathBuf {
        let d = tmp();
        for s in stores {
            let root = d.join("stores").join(s);
            for sub in ["migrations", "seed", "roles"] { std::fs::create_dir_all(root.join(sub)).unwrap(); }
            std::fs::write(root.join("migrations/0001_init.sql"), "CREATE TABLE items (id TEXT PRIMARY KEY, title TEXT NOT NULL);").unwrap();
            std::fs::write(root.join("seed/0001_demo.sql"), "INSERT INTO items VALUES ('a', 'A');").unwrap();
            std::fs::write(root.join("rules.yaml"), "version: \"1.0.0\"\ntables:\n  items: { global: true }\n").unwrap();
            std::fs::write(root.join("roles/member.yaml"), "name: member\ntables:\n  items: { readable: [id, title] }\n").unwrap();
        }
        d
    }

    async fn job<T>(tx: &mpsc::UnboundedSender<(Scope, Job)>, scope: &Scope, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Job) -> Result<T> {
        let (rtx, rrx) = oneshot::channel();
        tx.send((scope.clone(), make(rtx))).map_err(|_| anyhow!("actor gone"))?;
        rrx.await.map_err(|_| anyhow!("no reply"))?
    }

    async fn one(admin: &str, sql: &str, bind: &str) -> Option<String> {
        use sqlx::Connection;
        let mut c = sqlx::PgConnection::connect(admin).await.unwrap();
        sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(sql)).bind(bind).fetch_optional(&mut c).await.unwrap().flatten()
    }

    /// Three organizations (each its own platform database here) with the same app and store: each gets databases and roles
    /// of its own, named with its tag and labelled as its own; an organization whose tag names another's database (a reused
    /// tag) gets another database; the caps retire the oldest sandbox and refuse what would not fit; a store's roles go with
    /// its last database on its cluster. Needs CORI_STORE_DATABASE_URL.
    #[tokio::test]
    async fn each_realm_has_store_objects_of_its_own_within_its_caps() {
        let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let (Some((pa, _)), Some((pb, _)), Some((pc, _))) = (crate::db::test_database().await, crate::db::test_database().await, crate::db::test_database().await) else { return };
        let ta = format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let tb = format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let clusters = BTreeMap::from([("main".to_string(), admin.clone()), ("second".to_string(), admin.clone())]);
        let tx = spawn_actor(clusters, Arc::new(crate::bundles::BundleStore::local(tmp())));
        let realm = |db: &crate::db::Db, id: &str, tag: &str, placement: &str| Scope { db: db.in_realm(crate::db::Realm::new(id)), tag: tag.into(), placement: placement.into(), max_stores: 0, max_sandboxes: 0, broker: None };
        let (a, b, c) = (realm(&pa, "org-a", &ta, "main"), realm(&pb, "org-b", &tb, "second"), realm(&pc, "org-c", &ta, "main"));
        let desk = bundle(&["s"]);
        let v = |n: char| format!("sha256:{}", n.to_string().repeat(64));
        let provision = |scope: &Scope, app: &str, target: String, dir: &PathBuf| { let (scope, at, dir) = (scope.clone(), Target::sandbox(app, &target), dir.clone()); let tx = tx.clone(); async move { job(&tx, &scope, |reply| Job::Provision { at, bundle_dir: dir, seed_live: false, reply }).await } };
        let drop_it = |scope: &Scope, app: &str, target: String| { let (scope, at) = (scope.clone(), Target::sandbox(app, &target)); let tx = tx.clone(); async move { job(&tx, &scope, |reply| Job::Drop { at, live: false, reply }).await } };
        let roles = |tag: &str| { let (admin, like) = (admin.clone(), format!("cs_{tag}_desk_s_")); async move { one(&admin, "SELECT COUNT(*)::text FROM pg_roles WHERE starts_with(rolname, $1)", &like).await.unwrap() } };

        // each organization's sandbox of `desk`'s store `s`: a database and roles of its own, on its cluster
        let ra = provision(&a, "desk", v('a'), &desk).await.unwrap();
        let rb = provision(&b, "desk", v('a'), &desk).await.unwrap();
        let (da, db_) = (ra[0]["database"].as_str().unwrap().to_string(), rb[0]["database"].as_str().unwrap().to_string());
        assert_eq!(da, format!("cori_store_{ta}_desk_s_aaaaaaaaaaaa"));
        assert_eq!(db_, format!("cori_store_{tb}_desk_s_aaaaaaaaaaaa"));
        assert_eq!((ra[0]["cluster"].as_str(), rb[0]["cluster"].as_str()), (Some("main"), Some("second")));
        assert_eq!(one(&admin, "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1", &da).await.as_deref(), Some(format!("cori:org-a/desk/s/{}", v('a')).as_str()));
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT cluster FROM store_instances WHERE app = 'desk'").fetch_one(&b.db).await.unwrap(), "second");
        assert_eq!((roles(&ta).await, roles(&tb).await), ("4".to_string(), "4".to_string()), "member and the inspector, a data and an applier role each");

        // an organization whose tag names A's database never takes it over: it gets one of its own
        let rc = provision(&c, "desk", v('a'), &desk).await.unwrap();
        let dc = rc[0]["database"].as_str().unwrap().to_string();
        assert!(dc != da && dc.starts_with(&da) && dc.len() <= 63, "{dc}");
        assert_eq!(one(&admin, "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1", &da).await.as_deref(), Some(format!("cori:org-a/desk/s/{}", v('a')).as_str()), "A's label stays");

        // A keeps at most one sandbox database: deploying another retires the oldest
        let a1 = Scope { max_sandboxes: 1, ..a.clone() };
        provision(&a1, "desk", v('b'), &desk).await.unwrap();
        let kept: Vec<String> = sqlx::query_scalar("SELECT target FROM store_instances WHERE app = 'desk'").fetch_all(&a.db).await.unwrap();
        assert_eq!(kept, vec![v('b')]);
        assert!(!cori_store::database_exists(&admin, &da).await.unwrap(), "the retired sandbox's database is gone");
        // a version needing more sandbox databases than the cap alone: 402
        let two = bundle(&["s", "t"]);
        let e = provision(&a1, "desk", v('c'), &two).await.unwrap_err();
        assert_eq!(status_of(&e), axum::http::StatusCode::PAYMENT_REQUIRED, "{e}");
        // A may have one store: another app's is refused, the same store's next version is not
        let a2 = Scope { max_stores: 1, ..a.clone() };
        let e = provision(&a2, "other", v('d'), &desk).await.unwrap_err();
        assert_eq!(status_of(&e), axum::http::StatusCode::PAYMENT_REQUIRED, "{e}");
        provision(&a2, "desk", v('e'), &desk).await.unwrap();

        // a store's roles go with its last database on the cluster: C's go when A's last one does (they share a tag here)
        drop_it(&c, "desk", v('a')).await.unwrap();
        assert_eq!(roles(&ta).await, "4", "A's databases still use them");
        drop_it(&a, "desk", v('b')).await.unwrap();
        assert_eq!(roles(&ta).await, "4");
        drop_it(&a, "desk", v('e')).await.unwrap();
        assert_eq!(roles(&ta).await, "0");
        drop_it(&b, "desk", v('a')).await.unwrap();
        assert_eq!(roles(&tb).await, "0");
        assert!(!cori_store::database_exists(&admin, &db_).await.unwrap() && !cori_store::database_exists(&admin, &dc).await.unwrap());
    }

    /// Each store database gets logins of its own: an owner and a runtime login with random passwords, kept as its app's
    /// credentials; the data pool connects as its runtime login, which reaches that database only, and the shared runtime
    /// login no longer does. A database made before (the shared login) gets logins of its own at its next provisioning;
    /// rotating sets new passwords that the next call picks up; dropping the database drops its logins and their
    /// secrets. Needs CORI_STORE_DATABASE_URL (a server checking passwords, to mean anything).
    #[tokio::test]
    async fn a_store_database_has_logins_of_its_own() {
        use sqlx::Connection;
        let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let Some((platform, _)) = crate::db::test_database().await else { return };
        let app = format!("lg{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let tx = spawn_actor(BTreeMap::from([("main".to_string(), admin.clone())]), Arc::new(crate::bundles::BundleStore::local(tmp())));
        let broker = crate::secrets::Broker::from_key([7; 32]);
        let own = Scope { db: platform.clone(), tag: String::new(), placement: "main".into(), max_stores: 0, max_sandboxes: 0, broker: Some(broker.clone()) };
        let shared = Scope { broker: None, ..own.clone() };
        let desk = bundle(&["s", "t"]);
        let v = format!("sha256:{}", "a".repeat(64));
        let provision = |scope: &Scope, at: Target| { let (scope, dir, tx) = (scope.clone(), desk.clone(), tx.clone()); async move { job(&tx, &scope, |reply| Job::Provision { at, bundle_dir: dir, seed_live: false, reply }).await } };
        let who = crate::auth::Principal { sub: "u1".into(), roles: vec!["member".into()], email: None, claims: json!({}), token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        let read = |scope: &Scope, at: Target, store: &str| { let (scope, tx, who, store) = (scope.clone(), tx.clone(), who.clone(), store.to_string()); async move {
            let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "query", "arguments": { "sql": "SELECT id, title FROM items" } } });
            job(&tx, &scope, |reply| Job::Rpc { at, store, who, inspect: false, req, reply }).await
        } };
        let connects = |login: String, password: String, db: String| { let admin = admin.clone(); async move {
            let o = <sqlx::postgres::PgConnectOptions as std::str::FromStr>::from_str(&admin).unwrap().username(&login).password(&password).database(&db);
            sqlx::PgConnection::connect_with(&o).await.is_ok()
        } };

        // a sandbox with logins of its own
        let r = provision(&own, Target::sandbox(&app, &v)).await.unwrap();
        let db = r[0]["database"].as_str().unwrap().to_string();
        assert_eq!(r[0]["logins"]["runtime"].as_str(), Some(format!("{db}_runtime").as_str()), "{r:?}");
        let logins: Option<String> = sqlx::query_scalar("SELECT logins FROM store_instances WHERE app = $1 AND name = 's'").bind(&app).fetch_one(&platform).await.unwrap();
        assert!(logins.unwrap().contains("_runtime"));
        let runtime_pw = broker.reveal_in(&platform, Some(&app), &format!("store-login.{db}.runtime")).await.unwrap();
        let owner_pw = broker.reveal_in(&platform, Some(&app), &format!("store-login.{db}.owner")).await.unwrap();
        assert!(broker.reveal(&platform, &format!("store-login.{db}.runtime")).await.is_err(), "an app's credential, not the deployment's");
        assert!(connects(format!("{db}_runtime"), runtime_pw.clone(), db.clone()).await, "the runtime login connects with its password");
        assert!(connects(format!("{db}_owner"), owner_pw.clone(), db.clone()).await);
        assert!(!connects(format!("{db}_runtime"), "wrong".into(), db.clone()).await, "the server checks passwords");
        let other = r[1]["database"].as_str().unwrap().to_string();
        assert!(!connects(format!("{db}_runtime"), runtime_pw.clone(), other.clone()).await, "a runtime login reaches its own database only");
        let rows = read(&own, Target::sandbox(&app, &v), "s").await.unwrap();
        assert_eq!(rows["result"]["structuredContent"]["rows"][0]["title"], "A", "{rows}");
        let shared_can = one(&admin, "SELECT CASE WHEN EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cori_runtime') THEN has_database_privilege('cori_runtime', $1, 'CONNECT')::text ELSE 'no such role' END", &db).await;
        assert!(matches!(shared_can.as_deref(), Some("false" | "no such role")), "the shared runtime login does not reach it: {shared_can:?}");

        // a live database made with the shared login moves to logins of its own at its next provisioning
        provision(&shared, Target::live(&app)).await.unwrap();
        let before: Option<String> = sqlx::query_scalar("SELECT logins FROM store_instances WHERE app = $1 AND name = 's' AND target = 'live'").bind(&app).fetch_one(&platform).await.unwrap();
        assert!(before.is_none(), "made with the shared login");
        let live = crate::names::store_database("", &app, "s", "live");
        assert_eq!(one(&admin, "SELECT has_database_privilege('cori_runtime', $1, 'CONNECT')::text", &live).await.as_deref(), Some("true"));
        let r = provision(&own, Target::live(&app)).await.unwrap();
        assert!(r[0]["logins"].is_object(), "{r:?}");
        assert_eq!(one(&admin, "SELECT has_database_privilege('cori_runtime', $1, 'CONNECT')::text", &live).await.as_deref(), Some("false"), "no longer the shared login's");
        assert!(read(&own, Target::live(&app), "s").await.unwrap()["result"]["structuredContent"]["rows"].is_array());

        // rotation: new passwords, the old ones refused, the next call reconnects with the new
        let rotated = job(&tx, &own, |reply| Job::RotateLogins { app: Some(app.clone()), reply }).await.unwrap();
        assert_eq!(rotated.len(), 4, "two stores, a sandbox and the live database each: {rotated:?}");
        assert!(!connects(format!("{db}_runtime"), runtime_pw.clone(), db.clone()).await, "the old password is refused");
        let new_pw = broker.reveal_in(&platform, Some(&app), &format!("store-login.{db}.runtime")).await.unwrap();
        assert_ne!(new_pw, runtime_pw);
        assert!(connects(format!("{db}_runtime"), new_pw, db.clone()).await);
        assert_eq!(read(&own, Target::sandbox(&app, &v), "s").await.unwrap()["result"]["structuredContent"]["rows"][0]["title"], "A");

        // dropping the sandbox drops its logins and their secrets
        job(&tx, &own, |reply| Job::Drop { at: Target::sandbox(&app, &v), live: false, reply }).await.unwrap();
        assert_eq!(one(&admin, "SELECT COUNT(*)::text FROM pg_roles WHERE rolname IN ($1 || '_runtime', $1 || '_owner')", &db).await.as_deref(), Some("0"));
        assert!(!broker.exists_in(&platform, Some(&app), &format!("store-login.{db}.runtime")).await.unwrap());
        job(&tx, &own, |reply| Job::Drop { at: Target::live(&app), live: true, reply }).await.unwrap();
    }

    #[test]
    fn store_approval_ids_round_trip() {
        let at = Target::sandbox("support-desk", "sha256:3f9a0b1c2d3e4f50");
        let id = approval_id(&at, "support_tickets", "abc-123");
        assert_eq!(id, "store:support-desk:3f9a0b1c2d3e4f50:support_tickets:abc-123");
        let (t, store, aid) = parse_approval_id(&id).unwrap();
        assert_eq!((t.app.as_str(), t.target.as_str(), store.as_str(), aid.as_str()), ("support-desk", "sha256:3f9a0b1c2d3e4f50", "support_tickets", "abc-123"));
        let (t, _, _) = parse_approval_id(&approval_id(&Target::live("a"), "s", "x")).unwrap();
        assert!(t.is_live());
    }
}

/// The role a parked change is routed to: its `approvals.group` when the store or the app has a role of that name, else
/// the group read as the plural of one (`support_leads` → `support_lead`), else the group as written.
async fn approval_role(platform: &crate::db::Db, app: &str, store: &Store, group: &str) -> String {
    let app_roles: Vec<String> = sqlx::query("SELECT roles FROM apps WHERE id = $1").bind(app).fetch_optional(platform).await.ok().flatten()
        .and_then(|r| r.get::<Option<String>, _>("roles")).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    let known = |r: &str| store.has_role(r) || app_roles.iter().any(|a| a == r);
    if known(group) { return group.to_string(); }
    match group.strip_suffix('s') { Some(one) if known(one) => one.to_string(), _ => group.to_string() }
}

async fn handle_rpc(platform: &crate::db::Db, at: &Target, store: &Rc<Store>, who: &Principal, inspect: bool, req: &Value) -> Result<Value> {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let role = role_for(store, who, inspect);
    let result: Result<Value, String> = match method {
        "initialize" => Ok(json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": format!("cori-store/{}/{}{}", at.app, store.name, if at.is_live() { String::new() } else { format!("@{}", at.target.trim_start_matches("sha256:").get(..12).unwrap_or(&at.target)) }), "version": "3" } })),
        "notifications/initialized" | "ping" => Ok(json!({})),
        "tools/list" => {
            let mut tools = store.tools_for(role.as_deref().unwrap_or(""));
            if direct_writes_refused(store, role.as_deref(), who) { tools.retain(|t| !cori_store::Store::is_write_tool(t["name"].as_str().unwrap_or(""))); }
            Ok(json!({ "tools": tools }))
        }
        // not MCP: the caller's store role and, per table, its columns and what that role may do (the console's data browser);
        // someone who may not inspect the store (an app's user) is told of what their role reaches only
        "cori/describe" => {
            let d = store.describe(role.as_deref()).await;
            let mut d = if inspect { d } else { cori_store::reached_only(d) };
            d["inspector"] = json!(role.as_deref() == Some(cori_store::INSPECTOR)); d["target"] = json!(at.target); d["you"] = json!({ "sub": who.sub, "roles": who.roles });
            Ok(d)
        }
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("").to_string();
            let args = req["params"]["arguments"].clone();
            let dry = args["dryRun"].as_bool().unwrap_or(false);
            let write = cori_store::Store::is_write_tool(&name);
            // reading live data outside every business role is on the record
            if at.is_live() && role.as_deref() == Some(cori_store::INSPECTOR) {
                let _ = crate::db::audit(platform, &who.sub, "store.inspect", &format!("{}/{}", at.app, store.name), json!({ "tool": name, "arguments": args })).await;
            }
            let out: Result<Value> = if write && direct_writes_refused(store, role.as_deref(), who) {
                if who.is_agent() { Err(anyhow!("an agent session reads stores (`stores:read`): change data through a workflow run on a preview (`POST …/versions/<hash>/wf/<workflow>/runs`), or ask a person")) }
                else { Err(anyhow!("role `{}` writes to store `{}` through workflows only (`writes: workflows` in its role file): run the workflow that makes this change", role.as_deref().unwrap_or(""), store.name)) }
            } else {
                match role { Some(ref r) => store.call(r, &who.sub, &name, args, dry).await, None => Err(anyhow!("none of your roles ({:?}) is defined on store `{}`", who.roles, store.name)) }
            };
            // the database could not be reached: the call failed, not the tool (the caller may retry)
            let out = match out { Err(e) if e.downcast_ref::<cori_store::Unavailable>().is_some() => return Err(e), other => other };
            // a change to live data outside a workflow run is on the record (a run's writes are in its own history)
            if let (Ok(v), true) = (&out, write && !dry && at.is_live() && who.kind != crate::auth::PrincipalKind::Service) {
                let parked = v.get("pending_approval").and_then(|b| b.as_bool()) == Some(true);
                let _ = crate::db::audit(platform, &who.sub, "store.write", &format!("{}/{}", at.app, store.name), json!({ "tool": name, "role": role, "arguments": args_for_display(req), "parked": parked, "approval_id": v.get("approval_id") })).await;
            }
            match out {
                Ok(mut v) if v.get("pending_approval").and_then(|b| b.as_bool()) == Some(true) => {
                    // the change waits in the approvals row, shared by every server: the caller gets its id, not the change
                    let change = v.as_object_mut().and_then(|o| o.remove("change")).unwrap_or(Value::Null);
                    let aid = approval_id(at, &store.name, v["approval_id"].as_str().unwrap_or(""));
                    let to = match v["group"].as_str() { Some(g) => approval_role(platform, &at.app, store, g).await, None => "store_admin".into() };
                    let now = chrono::Utc::now();
                    let expires = now + chrono::Duration::from_std(cori_store::PARKED_TTL).unwrap_or_default();
                    let payload = json!({ "step": name, "kind": "store_approval", "app": at.app, "target": at.target, "store": store.name, "role": role.clone().unwrap_or_default(), "approval_id": v["approval_id"], "fields": v["approval_fields"], "message": v["message"], "payload": args_for_display(req), "requested_by": who.sub, "change": change, "expires_at": expires.to_rfc3339() });
                    let inboxed = sqlx::query("INSERT INTO approvals (id, run_id, app, step, to_role, payload, status, created_at) VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)")
                        .bind(&aid).bind(format!("store:{}:{}", at.target, store.name)).bind(&at.app).bind(&name).bind(&to).bind(payload.to_string()).bind(now.to_rfc3339()).execute(platform).await;
                    match inboxed {
                        Ok(_) => Ok(json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false })),
                        // a parked change nobody can see in the inbox would never be decided: say so (nothing was changed)
                        Err(e) => Ok(json!({ "content": [{ "type": "text", "text": format!("the change needs an approval, but the approval inbox could not record it ({e}): nothing was changed; try again") }], "isError": true })),
                    }
                }
                Ok(v) => Ok(json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false })),
                Err(e) => Ok(json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true })),
            }
        }
        other => Err(format!("method `{other}` not supported")),
    };
    Ok(match result { Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }), Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": e } }) })
}

fn args_for_display(req: &Value) -> Value { req["params"]["arguments"].clone() }

/// Local mode (`cori run` with a store database): provisions the app's stores from `app_dir` into the `local` target (the
/// registry in database `cori_local` on the same server, databases `cori_store_<app>_<store>_local`, seeded), then serves `/v1/stores/{name}/mcp`
/// on a loopback port where the principal is the `x-cori-user` header (the run's user, set by the engine). Returns the
/// endpoint URL and notes for the caller to print; the server lives as long as the process. `reset` drops the app's local
/// databases first (`cori run --reset-store`), so edited migrations and seed files apply again from scratch.
pub struct LocalEndpoint { pub url: String, pub notes: Vec<String> }
pub async fn local_endpoint(admin_url: &str, app_dir: &Path, data_dir: &Path, reset: bool) -> Result<LocalEndpoint> {
    use axum::{Router, routing::post, extract::{State, Path as AxPath}, http::HeaderMap, Json};
    std::fs::create_dir_all(data_dir)?;
    let app = std::fs::read_to_string(app_dir.join("cori.toml")).ok().and_then(|t| toml::from_str::<toml::Value>(&t).ok()).and_then(|t| t.get("app")?.get("id")?.as_str().map(String::from)).ok_or_else(|| anyhow!("{} has no cori.toml with [app] id", app_dir.display()))?;
    let platform = crate::db::Db::new(crate::db::connect(&crate::db::ensure_database(admin_url, "cori_local").await?).await?);
    let tx = spawn_actor(BTreeMap::from([(crate::MAIN_STORE_CLUSTER.to_string(), admin_url.to_string())]), Arc::new(crate::bundles::BundleStore::local(data_dir.to_path_buf())));
    let at = Target { app: app.clone(), target: LOCAL.into() };
    let mut notes = Vec::new();
    if reset {
        let (rtx, rrx) = oneshot::channel();
        tx.send((Scope::local(platform.clone()), Job::Drop { at: at.clone(), live: false, reply: rtx })).map_err(|_| anyhow!("store actor gone"))?;
        let mut dropped = rrx.await.map_err(|_| anyhow!("store actor dropped the reply"))??;
        // a database the local registry lost track of (a deleted ~/.cori/local) is dropped by name
        for d in read_store_dirs(app_dir) { if !dropped.contains(&d.name) && cori_store::drop_database(admin_url, &crate::names::store_database("", &app, &d.name, LOCAL)).await.map_err(|e| anyhow!("store `{}`: {e}", d.name))? { dropped.push(d.name); } }
        notes.push(if dropped.is_empty() { "store: --reset-store: no local database to drop".to_string() } else { format!("store: --reset-store dropped the local database of {}", dropped.join(", ")) });
    }
    let (rtx, rrx) = oneshot::channel();
    tx.send((Scope::local(platform.clone()), Job::Provision { at: at.clone(), bundle_dir: app_dir.to_path_buf(), seed_live: false, reply: rtx })).map_err(|_| anyhow!("store actor gone"))?;
    let report = rrx.await.map_err(|_| anyhow!("store actor dropped the reply"))??;
    for r in &report { if let Some(n) = r["store"].as_str() { tracing::info!(store = n, database = %r["database"], migrations = ?r["migrations_applied"], seeds = ?r["seeds_applied"], "local store ready"); } }
    for r in &report {
        let store = r["store"].as_str().unwrap_or("");
        for f in r["changed"].as_array().cloned().unwrap_or_default() { notes.push(format!("warning: stores/{store}/{} changed since it was applied to the local database (not applied again); `cori run --reset-store` to rebuild", f.as_str().unwrap_or(""))); }
    }
    struct Local { tx: mpsc::UnboundedSender<(Scope, Job)>, platform: crate::db::Db, at: Target }
    async fn mcp(State(l): State<Arc<Local>>, AxPath(name): AxPath<String>, headers: HeaderMap, Json(req): Json<Value>) -> Json<Value> {
        let u: Value = headers.get("x-cori-user").and_then(|v| v.to_str().ok()).and_then(|s| serde_json::from_str(s).ok()).unwrap_or(json!({ "sub": "dev", "roles": ["it_admin"] }));
        // the only caller is the run `cori run` started: a workflow run, which writes as a hosted run does (`writes: workflows`)
        let who = Principal { sub: u["sub"].as_str().unwrap_or("dev").to_string(), roles: u["roles"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(), email: u["email"].as_str().map(String::from), claims: u, token: None, kind: crate::auth::PrincipalKind::Service, scopes: vec![], app: None, session_id: None, platform: None };
        let (rtx, rrx) = oneshot::channel();
        if l.tx.send((Scope::local(l.platform.clone()), Job::Rpc { at: l.at.clone(), store: name, who, inspect: false, req, reply: rtx })).is_err() { return Json(json!({ "error": { "code": -32000, "message": "store actor gone" } })); }
        match rrx.await { Ok(Ok(v)) => Json(v), Ok(Err(e)) => Json(json!({ "jsonrpc": "2.0", "error": { "code": -32000, "message": e.to_string() } })), Err(_) => Json(json!({ "error": { "code": -32000, "message": "store actor dropped the reply" } })) }
    }
    let router = Router::new().route("/v1/stores/{name}/mcp", post(mcp)).with_state(Arc::new(Local { tx, platform, at }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move { let _ = axum::serve(listener, router).await; });
    Ok(LocalEndpoint { url, notes })
}

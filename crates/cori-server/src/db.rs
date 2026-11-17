// SPDX-License-Identifier: Apache-2.0
//! The platform database: Postgres (the cluster's own in production, an embedded one under `cori up`), its schema as
//! numbered migrations, and the few helpers several modules share. JSON is stored as TEXT, times as RFC 3339 TEXT.
//!
//! Rules for the platform tables, so that a module can redefine their keys (a realm column first, for row-level security)
//! without touching a query:
//! - every uniqueness is a named constraint: a primary key is `<table>_pkey`, any other key `<table>_<columns>_key`;
//! - an upsert names its constraint (`ON CONFLICT ON CONSTRAINT <name>`), never a column list (a test checks the sources);
//! - every query goes through a `Db`, never through the pool.
use anyhow::{Result, anyhow};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use sqlx::postgres::{PgPoolOptions, PgQueryResult, PgRow, PgStatement, PgTypeInfo};
use sqlx::{Connection, Either, Execute, Executor, PgPool, Postgres, SqlStr};

/// An opaque realm: the part of the platform one organization sees, once a module isolates organizations (`crate::realm`).
/// The core serves a single one and never names it. Its id is 1 to 64 letters, digits, `-` and `_`, so that it reads
/// unambiguously inside every name it enters (`crate::names`) and fits a Temporal fairness key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Realm(std::sync::Arc<str>);

impl Realm {
    /// A realm the caller knows to be valid (a module's own id); panics otherwise. Ids read from a request go through `parse`.
    pub fn new(id: impl Into<std::sync::Arc<str>>) -> Self {
        let id = id.into();
        assert!(Self::valid(&id), "a realm id is 1 to 64 letters, digits, `-` and `_`: {id:?}");
        Self(id)
    }
    /// A realm named by a request (a token's claim, a header, a Slack button), None when it is not a realm id.
    pub fn parse(id: &str) -> Option<Self> { Self::valid(id).then(|| Self(id.into())) }
    fn valid(id: &str) -> bool { (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') }
    pub fn as_str(&self) -> &str { &self.0 }
}

/// The platform database as a realm sees it. The server and its modules query through a `Db`, never through the pool: it
/// is an sqlx executor (`query.fetch_one(&st.db)`). With realms enforced (`enforcing`, for a module whose row-level
/// security reads it), each query first sets `cori.realm` on its connection: the realm, or '' outside any. Without
/// (the core), nothing is set and a query costs what it did on the pool.
#[derive(Debug, Clone)]
pub struct Db { pool: PgPool, realm: Option<Realm>, enforce: bool }

impl Db {
    pub fn new(pool: PgPool) -> Self { Self { pool, realm: None, enforce: false } }

    /// Every query sets `cori.realm` first: a module that isolates organizations turns this on.
    pub fn enforcing(mut self) -> Self { self.enforce = true; self }

    /// The same database as `realm` sees it.
    pub fn in_realm(&self, realm: Realm) -> Self { Self { realm: Some(realm), ..self.clone() } }

    /// The same database outside any realm: under enforced realms it shows no realm's rows, only the tables of the module
    /// that keeps the realms (`realm::Tenancy`).
    pub fn outside(&self) -> Self { Self { realm: None, ..self.clone() } }

    pub fn realm(&self) -> Option<&Realm> { self.realm.as_ref() }

    /// A connection with the realm set, for several statements on one connection (a transaction: `conn.begin()`). The
    /// setting is session-wide on purpose: a pooled connection keeps it until its next user sets its own, as every one does.
    pub async fn acquire(&self) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        if self.enforce {
            sqlx::query("SELECT set_config('cori.realm', $1, false)").bind(self.realm.as_ref().map(Realm::as_str).unwrap_or("")).execute(&mut *conn).await?;
        }
        Ok(conn)
    }

    /// A transaction with the realm set for it alone.
    pub async fn begin(&self) -> Result<sqlx::Transaction<'static, Postgres>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        if self.enforce {
            sqlx::query("SELECT set_config('cori.realm', $1, true)").bind(self.realm.as_ref().map(Realm::as_str).unwrap_or("")).execute(&mut *tx).await?;
        }
        Ok(tx)
    }

    pub async fn close(&self) { self.pool.close().await }
}

/// As sqlx's own executor for a pool: a connection per call, here with the realm set first. Rows are collected before
/// they are streamed (the platform's queries return pages, not tables).
impl<'d> Executor<'d> for &'_ Db {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(self, query: E) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, sqlx::Error>>
    where E: 'q + Execute<'q, Postgres> {
        let db = self.clone();
        Box::pin(futures::stream::once(async move {
            let mut conn = db.acquire().await?;
            let items: Vec<_> = conn.fetch_many(query).collect().await;
            Ok::<_, sqlx::Error>(futures::stream::iter(items))
        }).try_flatten())
    }

    fn fetch_optional<'e, 'q: 'e, E>(self, query: E) -> BoxFuture<'e, Result<Option<PgRow>, sqlx::Error>>
    where E: 'q + Execute<'q, Postgres> {
        let db = self.clone();
        Box::pin(async move { db.acquire().await?.fetch_optional(query).await })
    }

    fn prepare_with<'e>(self, sql: SqlStr, parameters: &'e [PgTypeInfo]) -> BoxFuture<'e, Result<PgStatement, sqlx::Error>>
    where 'd: 'e {
        let db = self.clone();
        Box::pin(async move { db.acquire().await?.prepare_with(sql, parameters).await })
    }

    fn describe<'e>(self, sql: SqlStr) -> BoxFuture<'e, Result<sqlx::Describe<Postgres>, sqlx::Error>> {
        let db = self.clone();
        Box::pin(async move { db.acquire().await?.describe(sql).await })
    }
}

fn env_num(var: &str, default: u64) -> u64 { std::env::var(var).ok().and_then(|v| v.parse().ok()).filter(|n: &u64| *n > 0).unwrap_or(default) }

/// The platform pool: `CORI_DB_POOL_MAX` connections (default 10) per server replica; a connection waits at most 10 s for
/// a slot, is checked before use, and is replaced after 30 min, so a primary switch (Patroni behind a Consul name) heals
/// within a request or two.
pub async fn connect(url: &str) -> Result<PgPool> { connect_with(url, &[]).await }

/// `connect`, the statements of the binary's modules (`Module::schema`) run after the platform schema's, under its lock.
pub async fn connect_with(url: &str, modules: &[&'static str]) -> Result<PgPool> {
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        anyhow::bail!("the platform database is Postgres: set --database-url / CORI_DATABASE_URL to a postgres:// URL{}", if url.is_empty() { "" } else { " (SQLite is no longer supported)" });
    }
    let pool = PgPoolOptions::new()
        .max_connections(env_num("CORI_DB_POOL_MAX", 10) as u32)
        .min_connections(0)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(1800))
        .test_before_acquire(true)
        .connect(url).await.map_err(|e| anyhow!("the platform database: {e}"))?;
    migrate(&pool, modules).await?;
    Ok(pool)
}

/// Creates database `name` on the server `admin_url` connects to unless it exists, and returns its URL: `cori up`'s
/// platform database, `cori run`'s local store registry. `name` is a literal of the caller.
pub async fn ensure_database(admin_url: &str, name: &'static str) -> Result<String> {
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("{admin_url}: {e}"))?;
    if sqlx::query("SELECT 1 FROM pg_database WHERE datname = $1").bind(name).fetch_optional(&admin).await?.is_none() {
        // two processes may create it at once: the second one's `duplicate_database` is the first one's success
        if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&admin).await {
            if e.as_database_error().and_then(|d| d.code()).as_deref() != Some("42P04") { return Err(anyhow!("creating database {name}: {e}")); }
        }
    }
    admin.close().await;
    Ok(cori_store::db_url_for(admin_url, name))
}

/// The platform schema, one migration per version, each applied in its own transaction and recorded in
/// `platform_migrations`. A database a newer Cori migrated further is refused: roll forward, or restore a backup taken
/// before the upgrade.
const MIGRATIONS: &[(i64, &str, &[&str])] = &[(1, "baseline", BASELINE), (2, "natural keys", NATURAL_KEYS), (3, "usage", USAGE), (4, "store placement", STORE_PLACEMENT), (5, "clients seen", CLIENTS_SEEN), (6, "starter calls", STARTER_CALLS), (7, "app credentials", APP_CREDENTIALS), (8, "identity events", IDENTITY_EVENTS), (9, "people seen", PEOPLE_SEEN), (10, "access requests", ACCESS_REQUESTS), (11, "roles seen", ROLES_SEEN), (12, "people removed", PEOPLE_REMOVED), (13, "front codes", FRONT_CODES), (14, "store role order", STORE_ROLE_ORDER), (15, "connections", CONNECTIONS), (16, "preview roles", PREVIEW_ROLES), (17, "files", FILES), (18, "app limits", APP_LIMITS)];

/// The newest platform schema version this server knows.
pub fn latest_migration() -> i64 { MIGRATIONS.last().map(|m| m.0).unwrap_or(0) }

/// Replicas starting together migrate one after the other: a session lock, held by the connection that runs the migrations.
async fn migrate(pool: &PgPool, modules: &[&'static str]) -> Result<()> {
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtext('cori_platform_schema'))").execute(&mut *conn).await?;
    let r = migrate_locked(&mut conn, modules).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('cori_platform_schema'))").execute(&mut *conn).await;
    r
}

async fn migrate_locked(conn: &mut sqlx::PgConnection, modules: &[&'static str]) -> Result<()> {
    // the schema of the Cori versions before these migrations (a `schema_version` table) is not migrated: nothing was
    // released on it, so such a database is refused rather than half-read
    // a login that may not create Cori's tables where its search_path points: say what it lacks, once, rather than fail on
    // the first statement
    let (user, schema, create, known): (String, Option<String>, Option<bool>, bool) = sqlx::query_as("SELECT current_user::text, current_schema()::text, CASE WHEN current_schema() IS NULL THEN NULL ELSE has_schema_privilege(current_schema(), 'CREATE') END, to_regclass('platform_migrations') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !known {
        match (schema, create) {
            (None, _) => anyhow::bail!("the platform database: login `{user}` has no schema to create Cori's tables in (its search_path names none it may use): create one it owns (`CREATE SCHEMA cori AUTHORIZATION {user}`) and name it in the URL (`?options=-c%20search_path%3Dcori`)"),
            (Some(s), Some(false)) => anyhow::bail!("the platform database: login `{user}` may not create tables in schema `{s}`: `GRANT CREATE ON SCHEMA {s} TO {user}` (or make it the schema's owner), or name a schema it owns in the URL (`?options=-c%20search_path%3D<schema>`)"),
            _ => {}
        }
    }
    let legacy = sqlx::query("SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = 'schema_version'").fetch_optional(&mut *conn).await?.is_some();
    sqlx::query("CREATE TABLE IF NOT EXISTS platform_migrations (version BIGINT PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").execute(&mut *conn).await?;
    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM platform_migrations ORDER BY version").fetch_all(&mut *conn).await?;
    if legacy && applied.is_empty() {
        anyhow::bail!("the platform database holds the schema of an earlier Cori (a `schema_version` table), which this one does not migrate: start on an empty database");
    }
    let latest = MIGRATIONS.last().map(|m| m.0).unwrap_or(0);
    if let Some(v) = applied.iter().copied().max().filter(|v| *v > latest) {
        anyhow::bail!("the platform database is at schema version {v}, newer than this server's {latest}: run the newer Cori (or restore a backup taken before the upgrade)");
    }
    for (version, name, statements) in MIGRATIONS {
        if applied.contains(version) { continue; }
        let mut tx = conn.begin().await?;
        for sql in statements.iter() { sqlx::query(*sql).execute(&mut *tx).await.map_err(|e| anyhow!("platform migration {version} ({name}): {e}\n{sql}"))?; }
        sqlx::query("INSERT INTO platform_migrations (version, name, applied_at) VALUES ($1, $2, $3)").bind(version).bind(name).bind(chrono::Utc::now().to_rfc3339()).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::info!(version, name, "platform schema migrated");
    }
    // the statements of the binary's modules (literals of their crates)
    for sql in modules { sqlx::query(*sql).execute(&mut *conn).await.map_err(|e| anyhow!("a module's schema: {e}\n{sql}"))?; }
    Ok(())
}

/// Version 1: the platform tables. `apps.promoted_hash` is an app's default version and `protected` gates its promotions
/// behind an approver; `runs.store_target` is `live` or the version hash of a preview run's sandbox;
/// `store_instances` has one row per store database (target `live`, a version's sandbox, or `local`).
const BASELINE: &[&str] = &[
    "CREATE TABLE apps (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, owner TEXT, builders TEXT, name TEXT, roles TEXT, template TEXT, promoted_hash TEXT, protected BIGINT, client_id TEXT, release_seq BIGINT)",
    "CREATE TABLE versions (hash TEXT PRIMARY KEY, app TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, created_by TEXT NOT NULL, workflows TEXT NOT NULL, review TEXT NOT NULL, cori_toml TEXT NOT NULL, approved_by TEXT, git_commit TEXT, git_branch TEXT, git_dirty BIGINT, created_kind TEXT, approved_against TEXT, seq BIGINT, message TEXT)",
    "CREATE TABLE runs (id TEXT PRIMARY KEY, app TEXT NOT NULL, workflow TEXT NOT NULL, version_hash TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, user_sub TEXT NOT NULL, params TEXT NOT NULL, output TEXT, error TEXT, user_json TEXT, principal_token TEXT, idem_key TEXT, parent_run TEXT, store_target TEXT)",
    "CREATE TABLE approvals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL, app TEXT NOT NULL, step TEXT NOT NULL, to_role TEXT NOT NULL, payload TEXT NOT NULL, status TEXT NOT NULL, decided_by TEXT, decided_at TEXT, created_at TEXT NOT NULL)",
    "CREATE TABLE audit (id TEXT PRIMARY KEY, actor TEXT NOT NULL, action TEXT NOT NULL, object TEXT NOT NULL, at TEXT NOT NULL, details TEXT NOT NULL)",
    "CREATE TABLE secrets (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, version BIGINT NOT NULL, dek_nonce TEXT NOT NULL, dek_wrapped TEXT NOT NULL, nonce TEXT NOT NULL, ciphertext TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL, revoked_at TEXT)",
    "CREATE TABLE secret_use (id TEXT PRIMARY KEY, name TEXT NOT NULL, run_id TEXT NOT NULL, connector TEXT NOT NULL, worker TEXT NOT NULL, at TEXT NOT NULL, outcome TEXT NOT NULL)",
    "CREATE TABLE store_instances (app TEXT NOT NULL, name TEXT NOT NULL, target TEXT NOT NULL, db_name TEXT NOT NULL, schema_yaml TEXT NOT NULL, rules_yaml TEXT NOT NULL, roles_yaml TEXT NOT NULL, seeded BIGINT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (app, name, target))",
    // schema_valid is read as an i32, by the server and by the LLM Ops module
    "CREATE TABLE llm_calls (id TEXT PRIMARY KEY, run_id TEXT NOT NULL, app TEXT, workflow TEXT NOT NULL, step TEXT NOT NULL, tier TEXT NOT NULL, model TEXT NOT NULL, mode TEXT NOT NULL, input TEXT NOT NULL, output TEXT, schema_valid INTEGER NOT NULL, latency_ms BIGINT NOT NULL, tokens_in BIGINT NOT NULL, tokens_out BIGINT NOT NULL, cost_eur DOUBLE PRECISION NOT NULL, at TEXT NOT NULL, label TEXT)",
    "CREATE TABLE bindings (id TEXT PRIMARY KEY, app TEXT NOT NULL, workflow TEXT NOT NULL, step TEXT NOT NULL, tier TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, mode TEXT NOT NULL, updated_at TEXT NOT NULL, updated_by TEXT NOT NULL)",
    "CREATE TABLE pools (name TEXT PRIMARY KEY, key_hash TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL, last_seen TEXT, capabilities TEXT)",
    "CREATE TABLE connectors (name TEXT PRIMARY KEY, kind TEXT NOT NULL, config TEXT NOT NULL, description TEXT NOT NULL, roles TEXT NOT NULL, tools TEXT NOT NULL, tools_hash TEXT NOT NULL, captured_at TEXT, declared_by TEXT NOT NULL, declared_at TEXT NOT NULL)",
    "CREATE TABLE connector_requests (id TEXT PRIMARY KEY, url TEXT NOT NULL, reason TEXT NOT NULL, requested_by TEXT NOT NULL, status TEXT NOT NULL, at TEXT NOT NULL)",
    "CREATE TABLE webhook_deliveries (id TEXT PRIMARY KEY, app TEXT NOT NULL, event TEXT NOT NULL, url TEXT NOT NULL, status BIGINT NOT NULL, attempts BIGINT NOT NULL, at TEXT NOT NULL, error TEXT)",
    "CREATE TABLE deployments (id TEXT PRIMARY KEY, app TEXT NOT NULL, hash TEXT NOT NULL, previous_hash TEXT, by_sub TEXT NOT NULL, kind TEXT NOT NULL, at TEXT NOT NULL, migrations TEXT)",
    // a schedule records who its runs act as (the service account, verified at sync) and the version it starts
    "CREATE TABLE schedules (id TEXT PRIMARY KEY, app TEXT NOT NULL, workflow TEXT NOT NULL, cron TEXT NOT NULL, tz TEXT NOT NULL, mode TEXT NOT NULL, updated_at TEXT NOT NULL, user_json TEXT, version_hash TEXT, problem TEXT)",
    "CREATE TABLE rate_events (id TEXT PRIMARY KEY, key TEXT NOT NULL, at BIGINT NOT NULL)",
    "CREATE TABLE run_events (id TEXT PRIMARY KEY, run_id TEXT NOT NULL, kind TEXT NOT NULL, at TEXT NOT NULL, payload TEXT NOT NULL)",
    "CREATE TABLE agent_sessions (jti TEXT PRIMARY KEY, sub TEXT NOT NULL, app TEXT NOT NULL, mcp_session TEXT, issued_at TEXT NOT NULL, expires_at TEXT NOT NULL, revoked_at TEXT)",
    "CREATE TABLE release_events (id TEXT PRIMARY KEY, app TEXT NOT NULL, hash TEXT NOT NULL, kind TEXT NOT NULL, by_sub TEXT NOT NULL, at TEXT NOT NULL, details TEXT NOT NULL)",
    // the org's LLM providers (keys in the broker as `llm.<name>`), default model per tier, monthly budget per app
    "CREATE TABLE llm_providers (name TEXT PRIMARY KEY, kind TEXT NOT NULL, base_url TEXT, models TEXT NOT NULL, enabled BIGINT NOT NULL, updated_at TEXT NOT NULL, updated_by TEXT NOT NULL)",
    "CREATE TABLE llm_tiers (tier TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, updated_at TEXT NOT NULL, updated_by TEXT NOT NULL)",
    "CREATE TABLE llm_budgets (app TEXT PRIMARY KEY, monthly_eur DOUBLE PRECISION NOT NULL, updated_at TEXT NOT NULL, updated_by TEXT NOT NULL)",
    // endpoint idempotency: one row per claimed key, inserted before the run starts so concurrent requests launch once
    "CREATE TABLE idempotency_keys (idem_key TEXT PRIMARY KEY, run_id TEXT NOT NULL, params_hash TEXT NOT NULL, created_at TEXT NOT NULL)",
    // the lookups the API and the sweeps make on large tables
    "CREATE INDEX runs_app_started ON runs (app, started_at)",
    "CREATE INDEX runs_status_started ON runs (status, started_at)",
    "CREATE INDEX approvals_app_status ON approvals (app, status)",
    "CREATE INDEX approvals_run ON approvals (run_id, step)",
    "CREATE INDEX llm_calls_app_at ON llm_calls (app, at)",
    "CREATE INDEX llm_calls_run ON llm_calls (run_id)",
    "CREATE INDEX audit_at ON audit (at)",
    "CREATE INDEX secret_use_run ON secret_use (run_id)",
    "CREATE INDEX webhook_deliveries_app_at ON webhook_deliveries (app, at)",
    "CREATE INDEX versions_app_created ON versions (app, created_at)",
    "CREATE INDEX store_instances_app ON store_instances (app, target)",
    "CREATE INDEX rate_events_key_at ON rate_events (key, at)",
    "CREATE INDEX run_events_run ON run_events (run_id, at)",
    "CREATE INDEX release_events_hash ON release_events (hash, at)",
];

/// Version 2: the natural keys the code relies on, enforced.
const NATURAL_KEYS: &[&str] = &[
    // a secret's versions: two concurrent puts of one name cannot both become version n
    "ALTER TABLE secrets ADD CONSTRAINT secrets_name_version_key UNIQUE (name, version)",
    // a release number names one version of an app (`v9`)
    "ALTER TABLE versions ADD CONSTRAINT versions_app_seq_key UNIQUE (app, seq)",
    // a bearer's `azp` names one app (front tokens are confined to it): a client hook naming a taken client is refused
    "ALTER TABLE apps ADD CONSTRAINT apps_client_id_key UNIQUE (client_id)",
];

/// Version 3: the usage ledger (`crate::usage`); the LLM spend set aside for calls in flight (`crate::llm::lease`); the
/// provider of a captured LLM call, which prices it; the size of a version's bundle.
const USAGE: &[&str] = &[
    "CREATE TABLE usage (day TEXT NOT NULL, metric TEXT NOT NULL, app TEXT NOT NULL, quantity DOUBLE PRECISION NOT NULL, CONSTRAINT usage_pkey PRIMARY KEY (day, metric, app))",
    "CREATE TABLE llm_reservations (id TEXT NOT NULL, run_id TEXT NOT NULL, app TEXT NOT NULL, provider TEXT NOT NULL, eur DOUBLE PRECISION NOT NULL, expires_at TEXT NOT NULL, CONSTRAINT llm_reservations_pkey PRIMARY KEY (id))",
    "CREATE INDEX llm_reservations_run ON llm_reservations (run_id, provider)",
    "ALTER TABLE llm_calls ADD COLUMN provider TEXT",
    "ALTER TABLE versions ADD COLUMN bytes BIGINT",
];

/// Where each store database is and under which names (`stores`): the cluster it was created on, the prefix of its
/// Postgres roles (empty: the one of its app and store, as stores had before).
const STORE_PLACEMENT: &[&str] = &[
    "ALTER TABLE store_instances ADD COLUMN cluster TEXT NOT NULL DEFAULT 'main'",
    "ALTER TABLE store_instances ADD COLUMN role_prefix TEXT NOT NULL DEFAULT ''",
];

/// A realm's calls of the starter model are counted from the leases granted for it (`llm::starter_call`): by name and time.
const STARTER_CALLS: &[&str] = &["CREATE INDEX secret_use_name_at ON secret_use (name, at)"];

/// The kinds of clients that called (`api::start`): an MCP host (`mcp`) and the command line (`cli`), when each was
/// first and last seen, what it said it was (`claude-code 2.1.0`, `cori/0.1.0`) and whose call it was. The console's
/// first steps tick themselves from it.
const CLIENTS_SEEN: &[&str] = &[
    "CREATE TABLE clients_seen (kind TEXT NOT NULL, first_at TEXT NOT NULL, last_at TEXT NOT NULL, detail TEXT NOT NULL, by_sub TEXT NOT NULL, CONSTRAINT clients_seen_pkey PRIMARY KEY (kind))",
];

/// Version 7: an app's own credentials (`secrets`): a secret's scope ('' for the deployment, else the app's id), what wraps
/// its data key (`master`, or a module's key wrapper), and which app's secret a lease used; a store database's own logins
/// (`store_instances.logins`: their names as JSON; NULL, the shared runtime login of the databases made before).
/// A secret's versions count per scope and name: the key `secrets_name_version_key` keeps its columns, whatever a module
/// made of them, and gains the scope.
const APP_CREDENTIALS: &[&str] = &[
    "ALTER TABLE secrets ADD COLUMN scope TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE secrets ADD COLUMN key_by TEXT NOT NULL DEFAULT 'master'",
    r#"DO $cori$
DECLARE cols text;
BEGIN
  SELECT string_agg(quote_ident(a.attname), ', ' ORDER BY k.ord) INTO cols
    FROM pg_constraint con CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
    JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
   WHERE con.conname = 'secrets_name_version_key' AND con.conrelid = 'secrets'::regclass;
  EXECUTE format('ALTER TABLE secrets DROP CONSTRAINT secrets_name_version_key, ADD CONSTRAINT secrets_name_version_key UNIQUE (%s, scope)', cols);
END $cori$"#,
    "CREATE INDEX secrets_scope_name ON secrets (scope, name)",
    "ALTER TABLE secret_use ADD COLUMN scope TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE store_instances ADD COLUMN logins TEXT",
];

/// Version 8: what the identity provider is told (`identity_hook`). An app's sign-in client may be pending while the
/// organization's IAM creates it (`apps.client_status`, its last answer `client_note`, the front origin it was last told,
/// `client_origin`). The events to deliver, in order per app (`identity_events.seq`), with their attempts. An app's role
/// catalog as the IAM was told it (`app_roles`: pending, confirmed, deleting). The roles each version declares and uses
/// (`versions.roles`, JSON: role → description and note), from which a role no kept version declares any more is found.
const IDENTITY_EVENTS: &[&str] = &[
    "ALTER TABLE apps ADD COLUMN client_status TEXT",
    "ALTER TABLE apps ADD COLUMN client_note TEXT",
    "ALTER TABLE apps ADD COLUMN client_origin TEXT",
    "ALTER TABLE versions ADD COLUMN roles TEXT",
    "CREATE SEQUENCE identity_events_seq",
    "CREATE TABLE identity_events (id TEXT NOT NULL, seq BIGINT NOT NULL DEFAULT nextval('identity_events_seq'), app TEXT NOT NULL, event TEXT NOT NULL, body TEXT NOT NULL, status TEXT NOT NULL, attempts BIGINT NOT NULL, next_at TEXT NOT NULL, occurred_at TEXT NOT NULL, delivered_at TEXT, last_status BIGINT, last_answer TEXT, CONSTRAINT identity_events_pkey PRIMARY KEY (id))",
    "CREATE INDEX identity_events_due ON identity_events (status, next_at)",
    "CREATE INDEX identity_events_app ON identity_events (app, seq)",
    "CREATE TABLE app_roles (app TEXT NOT NULL, role TEXT NOT NULL, description TEXT, note TEXT, state TEXT NOT NULL, declared_at TEXT NOT NULL, undeclared_since TEXT, confirmed_at TEXT, updated_at TEXT NOT NULL, CONSTRAINT app_roles_pkey PRIMARY KEY (app, role))",
];

/// Version 9: the people the server has seen (`api::person_seen`), from their tokens' claims: their name and address for
/// the ids the platform keeps (an app's owner and builders, a run's user, who decided, the audit log), and when each was
/// first and last seen.
const PEOPLE_SEEN: &[&str] = &[
    "CREATE TABLE people_seen (sub TEXT NOT NULL, email TEXT, name TEXT, first_at TEXT NOT NULL, last_at TEXT NOT NULL, CONSTRAINT people_seen_pkey PRIMARY KEY (sub))",
];

/// Version 10: every change to people and access asked through Cori (`api::people`): an invitation, a member's roles, a
/// removal, a grant or revocation of an app's roles. Each is delivered to the identity provider as a hook event
/// (`client_hook`), and keeps what it came to: `awaiting_approval` (a guest's, until an admin approves), `sent`, `pending`
/// (the provider is on it), `done`, `refused`, `given_up`, `cancelled`, `ended` (a guest's access past its end date).
const ACCESS_REQUESTS: &[&str] = &[
    "CREATE TABLE access_requests (id TEXT NOT NULL, kind TEXT NOT NULL, event TEXT NOT NULL, person TEXT, email TEXT, app TEXT, roles TEXT NOT NULL, details TEXT, guest BIGINT NOT NULL DEFAULT 0, until TEXT, note TEXT, status TEXT NOT NULL, asked_by TEXT NOT NULL, asked_at TEXT NOT NULL, approved_by TEXT, decided_at TEXT, decided_note TEXT, answer TEXT, result TEXT, updated_at TEXT NOT NULL, CONSTRAINT access_requests_pkey PRIMARY KEY (id))",
    "CREATE INDEX access_requests_status ON access_requests (status, updated_at)",
    "CREATE INDEX access_requests_app ON access_requests (app, status)",
    // what the receiver told Cori (`POST /v1/identity/notify`), by id: a notification sent twice is handled once
    "CREATE TABLE identity_notifications (id TEXT NOT NULL, event TEXT NOT NULL, at TEXT NOT NULL, CONSTRAINT identity_notifications_pkey PRIMARY KEY (id))",
];

/// Version 11: the roles a person's last token carried, as the people list shows them when the identity provider lists
/// nobody: their member roles (`roles`, the platform roles claim) and their roles in apps (`app_roles`, by client), as JSON.
const ROLES_SEEN: &[&str] = &[
    "ALTER TABLE people_seen ADD COLUMN roles TEXT",
    "ALTER TABLE people_seen ADD COLUMN app_roles TEXT",
];

/// Version 12: the people who left the organization (removed, left, or gone at the identity provider), and when: a token
/// issued before is refused at once (`api::principal`), rather than within its hour.
const PEOPLE_REMOVED: &[&str] = &[
    "CREATE TABLE people_removed (sub TEXT NOT NULL, at TEXT NOT NULL, how TEXT NOT NULL, CONSTRAINT people_removed_pkey PRIMARY KEY (sub))",
];

/// Version 13: the codes a front's own host trades for a session (`api::sessions`): each good for one token, for a minute,
/// kept by the hash of the code.
const FRONT_CODES: &[&str] = &[
    "CREATE TABLE front_codes (code_hash TEXT NOT NULL, payload TEXT NOT NULL, expires_at TEXT NOT NULL, CONSTRAINT front_codes_pkey PRIMARY KEY (code_hash))",
];

/// Version 14: the app's roles in the order its cori.toml lists them, kept with each store database (JSON): a person holding
/// several of the store's roles acts as the first of them (`cori_store::Store::role_of`). Empty until the store's next
/// provisioning: then as the token lists them, as before.
const STORE_ROLE_ORDER: &[&str] = &[
    "ALTER TABLE store_instances ADD COLUMN role_order TEXT NOT NULL DEFAULT '[]'",
];

/// Version 15: connections (`connections`): an account at one of the deployment's OAuth providers, a person's
/// (`person:<sub>`) or an app's (`app:<id>`), its refresh token sealed with the realm's key, the scopes the provider
/// granted (JSON), the address it named, who connected it and when it was last used.
const CONNECTIONS: &[&str] = &[
    "CREATE TABLE connections (owner TEXT NOT NULL, provider TEXT NOT NULL, account TEXT, scopes TEXT NOT NULL, refresh_sealed TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, used_at TEXT, CONSTRAINT connections_pkey PRIMARY KEY (owner, provider))",
];

/// Version 16: the roles a person chose to try an app's releases with (`api::sessions`, `PUT /v1/apps/{app}/preview-roles`),
/// as JSON: their releases' sessions hold them in place of their own roles; the app itself never does.
const PREVIEW_ROLES: &[&str] = &[
    "CREATE TABLE preview_roles (app TEXT NOT NULL, sub TEXT NOT NULL, roles TEXT NOT NULL, updated_at TEXT NOT NULL, CONSTRAINT preview_roles_pkey PRIMARY KEY (app, sub))",
];

/// Version 17: the files runs are given (`api::files`). A file belongs to an app and to the stores its runs work on
/// (`target`: `live`, or a release's hash), and to who announced its upload (`owner`), for a workflow's parameter; it is
/// `pending` until its bytes arrive, then `ready` with its type read from them and its `sha256`. `accept` (JSON) and
/// `max_bytes` are the parameter's limits as announced. An upload no run used expires (`expires_at`); `file_refs` lists
/// what holds a file (`run:<id>`), and a file nothing holds any more goes with the next sweep.
const FILES: &[&str] = &[
    "CREATE TABLE files (id TEXT NOT NULL, app TEXT NOT NULL, target TEXT NOT NULL, owner TEXT NOT NULL, workflow TEXT NOT NULL, parameter TEXT NOT NULL, name TEXT NOT NULL, content_type TEXT NOT NULL, size BIGINT NOT NULL, sha256 TEXT, status TEXT NOT NULL, accept TEXT, max_bytes BIGINT NOT NULL, created_at TEXT NOT NULL, ready_at TEXT, expires_at TEXT, CONSTRAINT files_pkey PRIMARY KEY (id))",
    "CREATE INDEX files_app_target ON files (app, target)",
    "CREATE INDEX files_expires_at ON files (expires_at)",
    "CREATE TABLE file_refs (file_id TEXT NOT NULL, holder TEXT NOT NULL, at TEXT NOT NULL, CONSTRAINT file_refs_pkey PRIMARY KEY (file_id, holder))",
    "CREATE INDEX file_refs_holder ON file_refs (holder)",
];

/// Version 18: an app's own limits, set by the organization's admins within the organization's (`usage::AppStorageLimit`):
/// the storage it may take (NULL: none of its own).
const APP_LIMITS: &[&str] = &[
    "CREATE TABLE app_limits (app TEXT NOT NULL, storage_bytes BIGINT, updated_at TEXT NOT NULL, updated_by TEXT NOT NULL, CONSTRAINT app_limits_pkey PRIMARY KEY (app))",
];

/// The next release number of an app: one more than any it ever had (a deleted release keeps its number).
pub async fn next_release_seq(db: &Db, app: &str) -> Result<i64> {
    let max: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM versions WHERE app = $1").bind(app).fetch_one(db).await?;
    let bumped = sqlx::query("UPDATE apps SET release_seq = CASE WHEN COALESCE(release_seq, 0) > $1 THEN release_seq ELSE $1 END + 1 WHERE id = $2").bind(max).bind(app).execute(db).await?.rows_affected();
    if bumped == 0 { return Ok(max + 1); }
    Ok(sqlx::query_scalar("SELECT release_seq FROM apps WHERE id = $1").bind(app).fetch_one(db).await?)
}

/// One line of a release's history (deployed, redeployed, approved, go_live_requested, went_live, replaced, preview_reset).
pub async fn release_event(db: &Db, app: &str, hash: &str, kind: &str, by: &str, details: serde_json::Value) -> Result<()> {
    sqlx::query("INSERT INTO release_events (id, app, hash, kind, by_sub, at, details) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(app).bind(hash).bind(kind).bind(by).bind(chrono::Utc::now().to_rfc3339()).bind(details.to_string()).execute(db).await?;
    Ok(())
}

pub async fn audit(db: &Db, actor: &str, action: &str, object: &str, details: serde_json::Value) -> Result<()> {
    sqlx::query("INSERT INTO audit (id, actor, action, object, at, details) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(actor).bind(action).bind(object).bind(chrono::Utc::now().to_rfc3339()).bind(details.to_string())
        .execute(db).await?;
    Ok(())
}

/// A platform database for a test: database `cori_test_<random>` on the server `CORI_STORE_DATABASE_URL` names (an admin
/// URL, as for the store tests), migrated. None when the variable is not set: the caller skips. The database is left
/// behind (`cori_test_%`): drop them after a local run.
#[cfg(test)]
pub(crate) async fn test_db() -> Option<Db> { test_database().await.map(|(db, _)| db) }

/// `test_db`, with the database's URL (for a test that opens connections of its own).
#[cfg(test)]
pub(crate) async fn test_database() -> Option<(Db, String)> {
    let admin = std::env::var("CORI_STORE_DATABASE_URL").ok()?;
    let name = format!("cori_test_{}", uuid::Uuid::new_v4().simple());
    let a = PgPoolOptions::new().max_connections(1).connect(&admin).await.expect("CORI_STORE_DATABASE_URL");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&a).await.expect("create the test database");
    a.close().await;
    let url = cori_store::db_url_for(&admin, &name);
    Some((Db::new(connect(&url).await.expect("migrate the test database")), url))
}

#[cfg(test)]
mod tests {
    /// Every upsert of the platform names its constraint: `ON CONFLICT (col)` would pin the key to those columns.
    #[test]
    fn upserts_name_their_constraint() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut unnamed = Vec::new();
        let needle = ["ON CONFLICT", " ("].concat();   // spelled apart, so this file does not match itself
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); continue; }
                if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for (i, line) in text.lines().enumerate() { if line.contains(&needle) && !line.trim_start().starts_with("//") { unnamed.push(format!("{}:{}", p.display(), i + 1)); } }
                }
            }
        }
        assert!(unnamed.is_empty(), "upserts on a column list instead of a named constraint: {unnamed:?}");
    }

    #[tokio::test]
    async fn a_db_sets_its_realm_only_when_realms_are_enforced() {
        use sqlx::Row;
        let Some(db) = super::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        async fn realm(d: &super::Db) -> Option<String> { sqlx::query_scalar("SELECT current_setting('cori.realm', true)").fetch_one(d).await.unwrap() }
        assert!(matches!(realm(&db).await.as_deref(), None | Some("")), "the core sets nothing");
        let org = db.clone().enforcing().in_realm(super::Realm::new("org-a"));
        assert_eq!(realm(&org).await.as_deref(), Some("org-a"));
        assert_eq!(realm(&db.clone().enforcing()).await.as_deref(), Some(""), "outside any realm, never a previous user's");
        // every call shape of the platform's code goes through the handle
        sqlx::query("INSERT INTO pools (name, key_hash, created_by, created_at) VALUES ($1, 'h', 'u', 'now')").bind("p1").execute(&org).await.unwrap();
        assert_eq!(sqlx::query("SELECT name FROM pools").fetch_all(&org).await.unwrap().iter().map(|r| r.get::<String, _>("name")).collect::<Vec<_>>(), ["p1"]);
        assert!(sqlx::query("SELECT 1 FROM pools WHERE name = 'none'").fetch_optional(&org).await.unwrap().is_none());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pools").fetch_one(&db).await.unwrap(), 1);
        let mut conn = org.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT current_setting('cori.realm')").fetch_one(&mut *tx).await.unwrap(), "org-a", "a transaction on its connection sees the realm");
        tx.rollback().await.unwrap();
    }

    /// The platform database as a DBA hands it over: a database Cori does not own, and a login that owns one schema of it,
    /// named in the URL (`options=-c search_path=…`). Cori's tables land there; a login without CREATE on its schema is
    /// refused with what to grant. Needs CORI_STORE_DATABASE_URL (an admin URL; a server checking passwords).
    #[tokio::test]
    async fn the_platform_lives_in_a_schema_of_its_own_with_a_login_that_owns_only_it() {
        use sqlx::Connection;
        let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let (role, dbname, other) = (format!("cori_lp_{tag}"), format!("cori_test_lp_{tag}"), format!("cori_lp_other_{tag}"));
        let mut a = sqlx::PgConnection::connect(&admin).await.unwrap();
        for sql in [format!("CREATE ROLE {role} LOGIN PASSWORD 'pw-{tag}'"), format!("CREATE ROLE {other} LOGIN PASSWORD 'pw-{tag}'"), format!("CREATE DATABASE {dbname}")] { sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&mut a).await.unwrap(); }
        let mut inside = sqlx::PgConnection::connect(&cori_store::db_url_for(&admin, &dbname)).await.unwrap();
        for sql in [format!("REVOKE CREATE ON SCHEMA public FROM PUBLIC"), format!("CREATE SCHEMA cori AUTHORIZATION {role}"), "CREATE SCHEMA bare".to_string(), format!("GRANT USAGE ON SCHEMA bare TO {other}")] { sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&mut inside).await.unwrap(); }
        let url = |login: &str, schema: &str| {
            let base = admin.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
            let host = base.rsplit_once('@').map(|(_, h)| h).unwrap_or(&base);
            let scheme = base.split("://").next().unwrap_or("postgres");
            format!("{scheme}://{login}:pw-{tag}@{host}/{dbname}?options=-c%20search_path%3D{schema}")
        };
        let pool = super::connect(&url(&role, "cori")).await.expect("migrates into its own schema");
        let in_cori: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'cori' AND tablename IN ('apps', 'secrets', 'identity_events')").fetch_one(&pool).await.unwrap();
        let in_public: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'public'").fetch_one(&pool).await.unwrap();
        assert_eq!((in_cori, in_public), (3, 0));
        pool.close().await;
        // a login that may only use its schema: refused with the grant to make
        let e = super::connect(&url(&other, "bare")).await.unwrap_err().to_string();
        assert!(e.contains(&format!("GRANT CREATE ON SCHEMA bare TO {other}")), "{e}");
    }

    #[tokio::test]
    async fn the_schema_migrates_once_and_refuses_a_newer_one() {
        let Some(db) = super::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let pool = &db.pool;
        let url = pool.connect_options().get_database().map(String::from).unwrap();
        // a second start finds nothing to do
        super::migrate(pool, &[]).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_migrations").fetch_one(&db).await.unwrap();
        assert_eq!(n, super::MIGRATIONS.len() as i64);
        sqlx::query("INSERT INTO platform_migrations (version, name, applied_at) VALUES (999, 'future', 'x')").execute(&db).await.unwrap();
        let e = super::migrate(pool, &[]).await.unwrap_err().to_string();
        assert!(e.contains("schema version 999, newer"), "{e} ({url})");
    }
}

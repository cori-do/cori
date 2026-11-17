// SPDX-License-Identifier: Apache-2.0
//! Cori Store: a governed Postgres database per store. Runs the Store's own definitions and tool generation
//! (`cori-store-core`) and its validator, executor and approvals (`cori-store-engine`) with a role name + tenant from the
//! caller's identity. The roles and rules are also compiled into Postgres grants and row level security (`policy`),
//! and every call runs under the role's Postgres role, so the database refuses what Cori's own checks might let through.
use anyhow::{Result, anyhow};
use cori_store_core::{RoleDefinition, RulesDefinition, SchemaDefinition};
use cori_store_engine::executor::{ExecutionContext, ToolExecutor};
use cori_store_core::protocol::{CallToolOptions, ToolDefinition};
use cori_store_core::tool_generator::generate_tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool};
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;

pub mod policy;
pub mod query;

pub use cori_store_engine::ParkedChange;
/// A Postgres identifier, double-quoted: the one quoting of the store's statements, grants and policies.
pub use cori_store_engine::grants::ident;

/// How long a parked change waits for a decision.
pub const PARKED_TTL: std::time::Duration = std::time::Duration::from_secs(3 * 86400);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreDef {
    pub name: String,
    /// The Postgres database this instance lives in (`db_name(app, store, target)`).
    #[serde(default)] pub db_name: String,
    pub schema_yaml: String,
    pub rules_yaml: String,
    /// role name → YAML
    pub roles_yaml: BTreeMap<String, String>,
    /// The prefix of its Postgres roles (`policy::role_prefix`), as its record keeps it; empty, the one of its app and name.
    #[serde(default)] pub role_prefix: String,
    /// The app's roles in the order it declares them: a caller holding several of the store's roles acts as the first of
    /// them in this order (`Store::role_of`). Empty: the first the caller's token lists.
    #[serde(default)] pub role_order: Vec<String>,
}

impl StoreDef {
    /// The prefix of the store's Postgres roles: its own, else the one of `app` and its name.
    pub fn role_prefix(&self, app: &str) -> String {
        if self.role_prefix.is_empty() { policy::role_prefix(app, &self.name) } else { self.role_prefix.clone() }
    }
}

/// The live target of a store (the promoted version's database). Sandboxes are keyed by the version hash, local runs by `local`.
pub const LIVE: &str = "live";
pub const LOCAL: &str = "local";
/// The read-only role Cori adds to every store for the app's owner, builders and admins (`policy::inspector_role`). No
/// role file may take the name, and holding it as an identity role grants nothing: the server alone assigns it.
pub const INSPECTOR: &str = "cori_inspector";

/// The Postgres database of a store: `cori_store_<app>_<store>` for the live target, `cori_store_<app>_<store>_<hash12>` for a
/// version's sandbox, `cori_store_<app>_<store>_local` for `cori run`. Identifiers are lower-cased `[a-z0-9_]`; a name that
/// would exceed Postgres' 63-character limit keeps a prefix and 8 hex characters of a sha256 of the full name, so it stays
/// deterministic and distinct.
pub fn db_name(app: &str, store: &str, target: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>();
    let suffix = match target { LIVE => String::new(), LOCAL => "_local".into(), h => format!("_{}", clean(h.trim_start_matches("sha256:").get(..12).unwrap_or(h))) };
    let full = format!("cori_store_{}_{}{suffix}", clean(app), clean(store));
    if full.len() <= 63 { return full; }
    use sha2::Digest;
    let h = hex::encode(sha2::Sha256::digest(full.as_bytes()));
    let keep = 63 - 9 - suffix.len();
    format!("{}_{}{suffix}", &full[..keep], &h[..8])
}

/// `name` made distinct for `label` (the store the database is for): the name with 8 hex characters of a sha256 of the
/// label, within Postgres' 63 characters. For a store whose own name is taken by another's database (`Taken`).
pub fn db_name_for(name: &str, label: &str) -> String {
    use sha2::Digest;
    let h = &hex::encode(sha2::Sha256::digest(label.as_bytes()))[..8];
    format!("{}_{h}", &name[..name.len().min(63 - 9)])
}

/// `postgres://u:p@h:port/<anything>` → the same server, database `db_name`.
pub fn db_url_for(admin_url: &str, db_name: &str) -> String {
    let base = admin_url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let query = admin_url.find('?').map(|i| &admin_url[i..]).unwrap_or("");
    format!("{base}/{db_name}{query}")
}

/// What `provision` did: the introspected schema, the migrations and seeds applied this time, whether the database was
/// created, and the applied files whose content changed since (they are never re-applied: `cori run --reset-store`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Provisioned { pub schema_yaml: String, pub migrations_applied: Vec<String>, pub seeds_applied: Vec<String>, pub created: bool, #[serde(default)] pub changed: Vec<String> }

/// Whose a store database is: written on it when it is created (`COMMENT ON DATABASE`), so that a store never takes over
/// a database of the same name that is another's (`Taken`).
#[derive(Debug, Clone, Default)]
pub struct Claim {
    /// The store the database is for (the server writes its realm, app, store and target); empty, no label at all.
    pub label: String,
    /// A database of the name that carries no label is taken as this store's: it predates labels, or a platform database was
    /// pointed at an existing store server. Never for an organization's new store.
    pub adopt_unlabelled: bool,
}

/// The database belongs to another store: its label names another (or none, where `Claim::adopt_unlabelled` is off).
#[derive(Debug, thiserror::Error)]
#[error("database `{0}` belongs to another store ({1})")]
pub struct Taken(pub String, pub String);

/// Creates the database if needed, applies pending `migrations` (name, sql) in order, then pending `seeds` (name, sql) in
/// order, each in its own transaction and recorded with a sha256 of its content (`cori_migrations`, `cori_seeds`) so a
/// second call is a no-op. Callers pass an empty `seeds` for a live database: seeds exist for sandboxes and local runs only.
pub async fn provision(admin_url: &str, db_name: &str, migrations: &[(String, String)], seeds: &[(String, String)]) -> Result<Provisioned> {
    provision_claimed(admin_url, db_name, &Claim::default(), migrations, seeds).await
}

/// `provision` for the store `claim` names: a database of the name that is another's is refused (`Taken`) before anything
/// is done to it, and a new one is labelled as the store's.
pub async fn provision_claimed(admin_url: &str, db_name: &str, claim: &Claim, migrations: &[(String, String)], seeds: &[(String, String)]) -> Result<Provisioned> {
    provision_with(admin_url, db_name, claim, &policy::Access::Shared, migrations, seeds).await
}

/// `provision_claimed` for a database reached with the logins `access` names: with logins of its own, its owner login
/// takes the given password (set again when it does not work with it), and its runtime login comes with its policy.
pub async fn provision_with(admin_url: &str, db_name: &str, claim: &Claim, access: &policy::Access, migrations: &[(String, String)], seeds: &[(String, String)]) -> Result<Provisioned> {
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    let found: Option<(Option<String>,)> = sqlx::query_as("SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1").bind(db_name).fetch_optional(&admin).await?;
    if !claim.label.is_empty() {
        match &found {
            Some((Some(l),)) if l != &claim.label => return Err(Taken(db_name.to_string(), l.clone()).into()),
            Some((None,)) if !claim.adopt_unlabelled => return Err(Taken(db_name.to_string(), "it carries no label".into()).into()),
            _ => {}
        }
    }
    let owner = policy::ensure_owner_with(&admin, admin_url, db_name, access).await?;
    let created = found.is_none();
    if created { sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {} OWNER {}", cori_store_engine::grants::ident(db_name), cori_store_engine::grants::ident(&owner)))).execute(&admin).await?; }
    policy::adopt_database(&admin, admin_url, db_name, &owner).await?;
    if !claim.label.is_empty() && found.as_ref().is_none_or(|f| f.0.is_none()) {
        sqlx::query(AssertSqlSafe(format!("COMMENT ON DATABASE {} IS {}", cori_store_engine::grants::ident(db_name), cori_store_engine::grants::literal(&claim.label)))).execute(&admin).await.map_err(|e| anyhow!("labelling database `{db_name}`: {e}"))?;
    }
    admin.close().await;
    // builder SQL (migrations, seeds) runs as the database's owner login, never as the admin
    let url = policy::owner_url_with(admin_url, db_name, access);
    let pool = PgPoolOptions::new().max_connections(1).connect_with(policy::owner_options_with(admin_url, db_name, access)?).await.map_err(|e| anyhow!("store owner connection: {e}"))?;
    let mut out = Provisioned { created, ..Default::default() };
    let (m, mc) = apply_tracked(&pool, "cori_migrations", "migration", migrations).await?;
    let (sd, sc) = apply_tracked(&pool, "cori_seeds", "seed", seeds).await?;
    out.migrations_applied = m; out.seeds_applied = sd;
    out.changed = mc.into_iter().map(|n| format!("migrations/{n}")).chain(sc.into_iter().map(|n| format!("seed/{n}"))).collect();
    out.schema_yaml = introspect(&url).await?;
    pool.close().await;
    Ok(out)
}

/// Applies the files not yet recorded; returns (applied, changed): `changed` names recorded files whose content differs
/// from what was applied. Rows recorded before hashes existed take the current hash.
async fn apply_tracked(pool: &PgPool, table: &str, what: &str, files: &[(String, String)]) -> Result<(Vec<String>, Vec<String>)> {
    sqlx::query(AssertSqlSafe(format!("CREATE TABLE IF NOT EXISTS {table} (name TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())"))).execute(pool).await?;
    sqlx::query(AssertSqlSafe(format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS sha256 TEXT"))).execute(pool).await?;
    let mut applied = Vec::new(); let mut changed = Vec::new();
    for (name, sql) in files {
        use sha2::Digest;
        let hash = hex::encode(sha2::Sha256::digest(sql.as_bytes()));
        let done: Option<(String, Option<String>)> = sqlx::query_as(AssertSqlSafe(format!("SELECT name, sha256 FROM {table} WHERE name = $1"))).bind(name).fetch_optional(pool).await?;
        if let Some((_, recorded)) = done {
            match recorded {
                Some(h) if h != hash => changed.push(name.clone()),
                None => { sqlx::query(AssertSqlSafe(format!("UPDATE {table} SET sha256 = $1 WHERE name = $2"))).bind(&hash).bind(name).execute(pool).await?; }
                _ => {}
            }
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(AssertSqlSafe(sql.as_str())).execute(&mut *tx).await.map_err(|e| anyhow!("{what} {name}: {e}"))?;
        sqlx::query(AssertSqlSafe(format!("INSERT INTO {table} (name, sha256) VALUES ($1, $2)"))).bind(name).bind(&hash).execute(&mut *tx).await?;
        tx.commit().await?;
        applied.push(name.clone());
    }
    Ok((applied, changed))
}

/// Whether the database exists on the admin server (a sandbox may have been dropped by retention: it is recreated lazily).
pub async fn database_exists(admin_url: &str, db_name: &str) -> Result<bool> {
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1").bind(db_name).fetch_optional(&admin).await?;
    Ok(exists.is_some())
}

/// Drops a store database (a sandbox being reseeded or retired). Other sessions are terminated first. Returns whether it existed.
pub async fn drop_database(admin_url: &str, db_name: &str) -> Result<bool> {
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1").bind(db_name).fetch_optional(&admin).await?;
    if exists.is_none() { return Ok(false); }
    sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()").bind(db_name).execute(&admin).await?;
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE {}", cori_store_engine::grants::ident(db_name)))).execute(&admin).await?;
    // its own logins served only that database
    for login in [policy::owner_role(db_name), policy::runtime_role(db_name)] {
        let _ = sqlx::query(AssertSqlSafe(format!("DROP ROLE IF EXISTS {}", cori_store_engine::grants::ident(&login)))).execute(&admin).await;
    }
    Ok(true)
}

/// Drops the Postgres roles of a store (`<prefix>__…`: its data and applier roles) on the admin's server, once its databases
/// there are gone. Postgres refuses to drop a role still granted something in any database of the server, so a role
/// another database of the store still uses stays. Returns the roles dropped.
pub async fn drop_roles(admin_url: &str, prefix: &str) -> Result<Vec<String>> {
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    let roles: Vec<String> = sqlx::query_scalar("SELECT rolname::text FROM pg_roles WHERE starts_with(rolname, $1) ORDER BY 1").bind(format!("{prefix}__")).fetch_all(&admin).await?;
    let mut dropped = Vec::new();
    for r in roles {
        match sqlx::query(AssertSqlSafe(format!("DROP ROLE {}", cori_store_engine::grants::ident(&r)))).execute(&admin).await {
            Ok(_) => dropped.push(r),
            Err(e) => tracing::warn!(role = %r, error = %e, "store role kept"),
        }
    }
    admin.close().await;
    Ok(dropped)
}

pub async fn introspect(db_url: &str) -> Result<String> {
    let json = cori_store_engine::introspect::introspect_schema_json(db_url).await?;
    Ok(serde_yaml::to_string(&json)?)
}

/// rules.yaml, or an empty rule set when the file is empty.
pub(crate) fn parse_rules(yaml: &str) -> Result<RulesDefinition> {
    if yaml.trim().is_empty() { RulesDefinition::from_yaml("version: \"1.0.0\"\ntables: {}\n").map_err(|e| anyhow!("rules: {e}")) } else { RulesDefinition::from_yaml(yaml).map_err(|e| anyhow!("rules.yaml: {e}")) }
}

/// The store's database could not be reached (no connection, a pool timeout): worth trying again later, unlike a refusal.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Unavailable(pub String);

/// Why a parked change was not applied. The server answers 410, 409 and 409 for them.
#[derive(Debug, thiserror::Error)]
pub enum DecideError {
    /// Expired, or parked where it can no longer be applied.
    #[error("the change {0} is no longer pending ({1})")]
    Gone(String, String),
    #[error("the change {0} was already decided")]
    Decided(String),
    /// It no longer applies (its row changed since it was parked, a constraint refused it): it stays pending.
    #[error("{0} (the change stays pending: reject it, or approve it again once the data allows it)")]
    Conflict(String),
}

pub struct Store {
    pub name: String,
    pub db_name: String,
    pub app: String,
    /// Connections of the database's runtime login (its own, or the shared `cori_runtime`), which reads nothing until a
    /// session switches it to a data role.
    pub pool: PgPool,
    pub schema: SchemaDefinition,
    pub rules: RulesDefinition,
    pub roles: BTreeMap<String, RoleDefinition>,
    /// Cori role → its Postgres data and applier roles.
    pg: BTreeMap<String, policy::PgRoles>,
    executors: BTreeMap<String, ToolExecutor>,
    tools: BTreeMap<String, Vec<ToolDefinition>>,
    /// tool name → table, as the core generator names them (getItem → items)
    tool_tables: BTreeMap<String, String>,
    /// The definition the store was opened with (role files: `writes: workflows`).
    def: StoreDef,
    /// The hash of the access policy the database enforces, and what the compiler reported.
    pub policy_hash: String,
    pub policy_warnings: Vec<String>,
}

impl Store {
    /// Opens a store database: applies its access policy when the recorded one differs (roles, grants, row level
    /// security, constraints and triggers compiled from the roles and rules), then connects as `cori_runtime`. A policy
    /// that cannot be applied is an error: the store serves no data rather than serve it unguarded.
    pub async fn open(admin_url: &str, app: &str, def: &StoreDef) -> Result<Store> { Self::open_with(admin_url, app, def, &policy::Access::Shared).await }

    /// `open` for a database reached with the logins `access` names: the data pool connects as its runtime login.
    pub async fn open_with(admin_url: &str, app: &str, def: &StoreDef, access: &policy::Access) -> Result<Store> {
        let (schema, rules, role_defs, compiled) = policy::compile_def(app, def)?;
        policy::apply_with(admin_url, &def.db_name, &compiled, access).await.map_err(|e| anyhow!("the database's access policy could not be applied, so the store serves no data: {e}"))?;
        // per open store: at most CORI_STORE_POOL_MAX connections (default 5), none kept once idle for a minute, so the
        // server's connection count follows the stores in use, not the stores (and sandboxes) that exist
        let max = std::env::var("CORI_STORE_POOL_MAX").ok().and_then(|v| v.parse().ok()).filter(|n: &u32| *n > 0).unwrap_or(5);
        let pool = PgPoolOptions::new().max_connections(max).min_connections(0)
            .idle_timeout(std::time::Duration::from_secs(60)).max_lifetime(std::time::Duration::from_secs(1800)).acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(policy::runtime_options_with(admin_url, &def.db_name, access)?).await.map_err(|e| Unavailable(format!("store runtime connection: {e}")))?;
        let mut roles = BTreeMap::new(); let mut executors = BTreeMap::new(); let mut tools = BTreeMap::new(); let mut tool_tables = BTreeMap::new(); let mut pg = BTreeMap::new();
        for role in role_defs {
            let p = compiled.roles.iter().find(|p| p.role == role.name).cloned().unwrap_or_else(|| policy::pg_roles(&def.role_prefix(app), &role.name));
            let generated = generate_tools(&schema, &role);
            for d in &generated { if let Some((_, t)) = cori_store_core::tool_generator::tool_table(&role, &d.name) { tool_tables.insert(d.name.clone(), t.to_string()); } }
            tools.insert(role.name.clone(), generated);
            executors.insert(role.name.clone(), ToolExecutor::new(role.clone(), p.applier.clone()).with_rules(rules.clone()).with_schema(schema.clone()));
            pg.insert(role.name.clone(), p);
            roles.insert(role.name.clone(), role);
        }
        Ok(Store { name: def.name.clone(), db_name: def.db_name.clone(), app: app.to_string(), pool, schema, rules, roles, pg, executors, tools, tool_tables, def: def.clone(), policy_hash: compiled.hash, policy_warnings: compiled.warnings })
    }

    pub fn has_role(&self, role: &str) -> bool { self.roles.contains_key(role) }

    /// A query refused a column, by a role that sees every row of a table whose owner column its `readable` leaves out:
    /// the error says so. A role scoped to its own rows may always use that column (it holds the caller's own id); a role
    /// with `scope: all` reads it, the owners of every row, only when it lists it.
    fn owner_column_hint(&self, r: &RoleDefinition, e: anyhow::Error) -> anyhow::Error {
        let msg = e.to_string();
        if !msg.contains("may not do this on") { return e; }
        let mut unlisted: Vec<String> = r.tables.iter().filter(|(_, p)| p.scope == cori_store_core::RowScope::All).filter_map(|(t, _)| match self.rules.tables.get(t).and_then(|tr| tr.tenant.as_ref()) {
            Some(cori_store_core::TenantConfig::Direct(col)) if !r.can_read_column(t, col) => Some(format!("`{t}.{col}`")),
            _ => None,
        }).collect();
        if unlisted.is_empty() { return e; }
        unlisted.sort();
        anyhow!("{msg}. Note: role `{}` sees every row (`scope: all`) but reads the owner column {} only when its `readable` lists it", r.name, unlisted.join(", "))
    }

    /// The role a caller holding `roles` acts as here: of those the store defines, the first in the app's declared order
    /// (`StoreDef::role_order`), else the first `roles` lists. Never the inspector, which no identity carries.
    pub fn role_of(&self, roles: &[String]) -> Option<String> {
        let defined = |r: &&String| r.as_str() != INSPECTOR && self.has_role(r);
        self.def.role_order.iter().filter(defined).find(|r| roles.contains(r)).or_else(|| roles.iter().find(defined)).cloned()
    }

    /// Whether a role's file says `writes: workflows`: its members change data only through workflow runs (the run acts
    /// as them, after the workflow's own checks); a person or an agent calling the store directly may read but not write.
    pub fn writes_through_workflows_only(&self, role: &str) -> bool {
        self.def.roles_yaml.get(role).and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
            .and_then(|v| v.get("writes").and_then(|w| w.as_str().map(|w| w.trim().eq_ignore_ascii_case("workflows")))).unwrap_or(false)
    }

    /// Whether a tool changes data (the generated create/update/delete tools and parked-change decisions).
    pub fn is_write_tool(tool: &str) -> bool {
        ["create", "update", "delete", "approve", "reject"].iter().any(|p| tool.starts_with(p) && tool[p.len()..].chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false))
    }

    /// A transaction as Postgres role `pg_role` for the caller `tenant`: the tenant setting the row policies compare,
    /// a statement timeout and, for `query`, read-only. Everything is transaction-local, so a pooled connection never
    /// carries a previous caller's identity. The role name comes from the store's own role map, never from input.
    async fn session(&self, pg_role: &str, tenant: &str, read_only: bool) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        if tenant.contains('\0') { return Err(anyhow!("invalid caller id")); }
        let mut tx = self.pool.begin().await.map_err(|e| Unavailable(format!("store connection: {e}")))?;
        // statements still carry literals of their own (a soft delete's configured value, the `query` tool's SQL): a
        // backslash in them must stay a character even where the builder's migrations turned standard_conforming_strings off
        let sql = if read_only {
            "SELECT set_config('cori.tenant', $1, true), set_config('statement_timeout', '10s', true), set_config('standard_conforming_strings', 'on', true), set_config('transaction_read_only', 'on', true), set_config('role', $2, true)"
        } else {
            "SELECT set_config('cori.tenant', $1, true), set_config('statement_timeout', '10s', true), set_config('standard_conforming_strings', 'on', true), set_config('role', $2, true)"
        };
        sqlx::query(sql).bind(tenant).bind(pg_role).execute(&mut *tx).await.map_err(|e| anyhow!("store session: {e}"))?;
        Ok(tx)
    }

    /// MCP tool list for a role: the generated data tools and `query` (guarded SELECT). The schema changes only through
    /// the app's migrations (deploy, promote), never through a tool.
    pub fn tools_for(&self, role: &str) -> Vec<Value> {
        let mut out: Vec<Value> = self.tools.get(role).map(|ts| ts.iter().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).collect()).unwrap_or_default();
        if self.roles.contains_key(role) {
            out.push(json!({ "name": "query", "description": "Read-only SQL (SELECT) limited to the tables and columns your role can read", "inputSchema": { "type": "object", "properties": { "sql": { "type": "string" }, "params": { "type": "array" }, "max_rows": { "type": "integer" } }, "required": ["sql"] }, "annotations": { "readOnlyHint": true } }));
        }
        out
    }

    /// Which rows of `table` `role` reaches: `own` (its tenant column equals the caller), `all`, `global` (a shared
    /// table), `unscoped` (named in rules.yaml without tenancy) or `hidden` (missing from rules.yaml: no row at all).
    fn row_scope(&self, role: Option<&RoleDefinition>, table: &str) -> &'static str {
        let Some(tr) = self.rules.get_table_rules(table) else { return "hidden" };
        if tr.global.unwrap_or(false) { return "global"; }
        if tr.tenant.is_none() { return "unscoped"; }
        match role.map(|r| r.row_scope(table)) { Some(cori_store_core::RowScope::All) => "all", _ => "own" }
    }

    /// Every table with its columns and what `role` may do with each (read, create, update, approval), plus the generated
    /// tool of each operation the role has on it. The console's data browser is built from it; without a role (an admin
    /// that holds none of the store's roles) it is the schema alone, every permission false. Column rights are those the
    /// database grants the role's data role, intersected with the role file; a column the role file names but the
    /// database refuses is listed under `drift` (the policy the database enforces is not the one the files describe).
    pub async fn describe(&self, role: Option<&str>) -> Value {
        let r = role.and_then(|n| self.roles.get(n));
        let pg = role.and_then(|n| self.pg.get(n));
        let mine: Vec<&str> = role.and_then(|n| self.tools.get(n)).map(|ts| ts.iter().map(|t| t.name.as_str()).collect()).unwrap_or_default();
        // (table, column) → (select, insert, update); table → (delete, row level security)
        let mut cols_db: BTreeMap<(String, String), (bool, bool, bool)> = BTreeMap::new();
        let mut tables_db: BTreeMap<String, (bool, bool)> = BTreeMap::new();
        if let Some(p) = pg {
            let q = "SELECT c.relname::text, a.attname::text, has_column_privilege($1, c.oid, a.attnum, 'SELECT'), has_column_privilege($1, c.oid, a.attnum, 'INSERT'), has_column_privilege($1, c.oid, a.attnum, 'UPDATE'), has_table_privilege($1, c.oid, 'DELETE'), c.relrowsecurity \
                     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
                     WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p')";
            match sqlx::query_as::<_, (String, String, bool, bool, bool, bool, bool)>(q).bind(&p.data).fetch_all(&self.pool).await {
                Ok(rows) => for (t, c, s, i, u, d, rls) in rows { cols_db.insert((t.clone(), c), (s, i, u)); tables_db.insert(t, (d, rls)); },
                Err(e) => tracing::warn!(store = %self.name, error = %e, "describe: reading the database's grants failed"),
            }
        }
        let db = |t: &str, c: &str| cols_db.get(&(t.to_string(), c.to_string())).copied().unwrap_or((false, false, false));
        // the bookkeeping tables of `provision` and of the policy are not the app's data
        let tables: Vec<Value> = self.schema.tables.iter().filter(|t| !cori_store_engine::grants::BOOKKEEPING_TABLES.contains(&t.name.as_str())).map(|t| {
            let mut tools = serde_json::Map::new();
            for (tool, table) in &self.tool_tables {
                if table != &t.name || !mine.contains(&tool.as_str()) { continue; }
                if let Some(op) = ["list", "get", "create", "update", "delete"].into_iter().find(|op| tool.starts_with(op)) { tools.insert(op.into(), json!(tool)); }
            }
            let approval: Vec<&str> = r.map(|r| r.get_approval_columns(&t.name)).unwrap_or_default();
            let mut drift: Vec<String> = Vec::new();
            let columns: Vec<Value> = t.columns.iter().map(|c| {
                let fk = t.foreign_keys.iter().find(|f| f.columns.len() == 1 && f.columns[0] == c.name).map(|f| json!({ "table": f.references.table, "column": f.references.columns.first() }));
                let can = |f: fn(&RoleDefinition, &str, &str) -> bool| r.map(|r| f(r, &t.name, &c.name)).unwrap_or(false);
                let gated = approval.contains(&c.name.as_str());
                // gated per operation: the applier writes a gated column, not the data role
                let perms = r.and_then(|r| r.tables.get(&t.name));
                let gated_create = perms.and_then(|p| p.creatable.get_constraints(&c.name)).is_some_and(|k| k.requires_approval.as_ref().is_some_and(|a| a.is_required()));
                let gated_update = perms.and_then(|p| p.updatable.get_constraints(&c.name)).is_some_and(|k| k.requires_approval.as_ref().is_some_and(|a| a.is_required()));
                let (s, i, u) = db(&t.name, &c.name);
                let (read, create, update) = (can(RoleDefinition::can_read_column), can(RoleDefinition::can_create_column), can(RoleDefinition::can_update_column));
                if pg.is_some() && ((read && !s) || (create && !gated_create && !i) || (update && !gated_update && !u)) { drift.push(c.name.clone()); }
                json!({ "name": c.name, "type": c.native_type.clone().unwrap_or_else(|| format!("{:?}", c.column_type).to_lowercase()), "nullable": c.nullable, "default": c.default, "pk": t.primary_key.contains(&c.name), "fk": fk,
                        "read": read && s, "create": create && (i || gated_create), "update": update && (u || gated_update), "approval": gated })
            }).collect();
            let scope = self.row_scope(r, &t.name);
            let soft = self.rules.get_table_rules(&t.name).and_then(|x| x.soft_delete.as_ref()).map(|s| s.column.clone());
            let soft_delete = r.and_then(|r| r.tables.get(&t.name)).map(|p| p.deletable.is_soft_delete()).unwrap_or(false);
            json!({ "name": t.name, "primary_key": t.primary_key, "columns": columns, "tenant": if scope == "own" { self.tenant_label(&t.name) } else { None }, "tools": tools,
                    "read": r.map(|r| r.can_read(&t.name)).unwrap_or(false), "delete": r.map(|r| r.can_delete(&t.name)).unwrap_or(false), "max_per_page": r.and_then(|r| r.get_max_per_page(&t.name)),
                    "scope": scope, "soft_delete": soft, "delete_is_soft": soft_delete, "rls": tables_db.get(&t.name).map(|x| x.1).unwrap_or(false), "drift": drift })
        }).collect();
        json!({ "store": self.name, "role": role, "roles": self.roles.values().filter(|r| r.name != INSPECTOR).map(|r| json!({ "name": r.name, "description": r.description })).collect::<Vec<_>>(), "tables": tables,
                "policy": { "hash": self.policy_hash, "enforced_by": "postgres", "warnings": self.policy_warnings } })
    }

    /// How a tenant table is scoped, for display: its tenant column, or `<via> → <parent>` for inherited tenancy.
    fn tenant_label(&self, table: &str) -> Option<String> {
        let tr = self.rules.get_table_rules(table)?;
        if tr.global.unwrap_or(false) { return None; }
        match &tr.tenant {
            Some(cori_store_core::config::rules_definition::TenantConfig::Direct(c)) => Some(c.clone()),
            Some(cori_store_core::config::rules_definition::TenantConfig::Inherited(i)) => Some(format!("{} → {}", i.via, i.references)),
            None => None,
        }
    }

    /// Calls a tool as `role`. `tenant` is the caller's user id. Every call runs in a transaction under the role's
    /// Postgres data role with the tenant set, so the database enforces the role's columns and rows whatever Cori's own
    /// checks let through: a table whose rules declare `tenant: <column>` is scoped to rows where that column equals
    /// the caller (unless the role has `scope: all`), and a create sets the column from it.
    pub async fn call(&self, role: &str, tenant: &str, tool: &str, mut args: Value, dry_run: bool) -> Result<Value> {
        // `dryRun` is an option the caller passes among the arguments (read into `dry_run`), not a column
        if let Some(o) = args.as_object_mut() { o.remove("dryRun"); }
        let r = self.roles.get(role).ok_or_else(|| anyhow!("role `{role}` is not defined on store `{}`", self.name))?;
        let pg = self.pg.get(role).ok_or_else(|| anyhow!("role `{role}` is not defined on store `{}`", self.name))?;
        if tool == "query" {
            let sql = args.get("sql").and_then(|s| s.as_str()).ok_or_else(|| anyhow!("query needs `sql`"))?;
            let max = args.get("max_rows").and_then(|m| m.as_u64()).unwrap_or(500).min(5000);
            let params: Vec<Value> = args.get("params").and_then(|p| p.as_array()).cloned().unwrap_or_default();
            let mut tx = self.session(&pg.data, tenant, true).await?;
            let rows = query::run_guarded(&mut tx, r, sql, &params, max).await;
            tx.rollback().await.ok();
            return rows.map(|rows| json!({ "rows": rows })).map_err(|e| self.owner_column_hint(r, e));
        }
        let ex = self.executors.get(role).ok_or_else(|| anyhow!("role `{role}` is not defined on store `{}`", self.name))?;
        let def = self.tools.get(role).and_then(|ts| ts.iter().find(|t| t.name == tool)).ok_or_else(|| anyhow!("tool `{tool}` is not available to role `{role}`"))?;
        let mut tx = self.session(&pg.data, tenant, false).await?;
        let res = ex.execute_on(&mut tx, def, args.clone(), &CallToolOptions { dry_run }, &ExecutionContext { tenant_id: tenant.to_string(), role: role.to_string(), connection_id: None }).await;
        if res.success && !dry_run { tx.commit().await?; } else { tx.rollback().await.ok(); }
        if !res.success { return Err(anyhow!("{}", explain_db_error(role, &res.error.unwrap_or_else(|| "tool failed".into())))); }
        // A `requires_approval` column: the change is parked, not executed; the caller keeps it (`change`) until it is decided.
        if let Some(change) = res.parked {
            let pending = change.pending_response();
            return Ok(json!({ "pending_approval": true, "approval_id": change.id, "approval_fields": change.approval_fields, "message": pending["message"], "group": r.approvals.as_ref().map(|a| a.group.clone()), "change": change }));
        }
        // Prefer structured JSON content; fall back to the text block (usually JSON text).
        let mut out = Value::Null;
        for c in &res.content { match c { cori_store_core::protocol::ToolContent::Json { json } => { out = json.clone(); break; } cori_store_core::protocol::ToolContent::Text { text } => { out = serde_json::from_str(text).unwrap_or(Value::String(text.clone())); } } }
        if let Some(after) = res.after_state { if out.is_null() || out.is_string() { out = after; } }
        // A get of a missing row is an error on every key type (the core tool answers { data: null, message }).
        if !dry_run && tool.starts_with("get") && out.get("data").map(|d| d.is_null()).unwrap_or(false) { return Err(anyhow!("{tool}: no row with {}", args)); }
        // Single-row tools answer with the row itself (the core tool wraps it as { data, message }); list keeps { data, count, … }.
        if (tool.starts_with("get") || tool.starts_with("create") || tool.starts_with("update")) && out.get("data").map(|d| d.is_object()).unwrap_or(false) { out = out["data"].clone(); }
        Ok(out)
    }

    /// Applies an approved change under the requesting role's applier role (which holds the gated grants and is bound by
    /// the same row scope) with the requester's tenant, under the checks of a call as the role now has them. `mark`
    /// records the decision (the server: its approvals row, pending → decided, only if still pending) after the change
    /// applied and before it commits: a change that no longer applies is refused (`DecideError::Conflict`) and left
    /// undecided, and of two concurrent approvals only the one that marks it commits (`mark` fails for the other).
    pub async fn apply_parked<F, Fut>(&self, change: &ParkedChange, mark: F) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let role = change.role.as_str();
        let no_longer = |why: String| -> anyhow::Error { DecideError::Conflict(why).into() };
        let ex = self.executors.get(role).ok_or_else(|| no_longer(format!("role `{role}` is no longer defined on store `{}`", self.name)))?;
        let pg = self.pg.get(role).ok_or_else(|| no_longer(format!("role `{role}` is no longer defined on store `{}`", self.name)))?;
        let tool = self.tools.get(role).and_then(|ts| ts.iter().find(|t| t.name == change.tool_name)).ok_or_else(|| no_longer(format!("tool `{}` is no longer available to role `{role}`", change.tool_name)))?;
        let mut tx = self.session(&pg.applier, &change.tenant_id, false).await?;
        let res = ex.execute_approved_on(&mut tx, tool, change, &ExecutionContext { tenant_id: change.tenant_id.clone(), role: role.to_string(), connection_id: None }).await;
        if !res.success {
            tx.rollback().await.ok();
            return Err(no_longer(explain_db_error(role, &res.error.unwrap_or_else(|| "the approved change failed".into()))));
        }
        if let Err(e) = mark().await { tx.rollback().await.ok(); return Err(e); }
        tx.commit().await?;
        Ok(res.after_state.unwrap_or(json!({ "executed": change.id })))
    }
}

/// What `Store::describe` shows someone who does not build the app (one of its users): of the tables and columns, those
/// their role has a right on, and nothing of the others, their names included; of the store's roles, theirs; none of the
/// policy's workings (row security, drift, warnings, the tenant column). Without a role, no table at all.
pub fn reached_only(mut d: Value) -> Value {
    let role = d["role"].as_str().map(String::from);
    let right = |c: &Value| ["read", "create", "update"].iter().any(|k| c[*k].as_bool() == Some(true));
    // a table in reach: one the role reads, deletes from, has a tool on, or has a right on a column of
    let kept: Vec<Value> = if role.is_none() { vec![] } else {
        d["tables"].as_array().cloned().unwrap_or_default().into_iter().filter(|t| {
            t["read"].as_bool() == Some(true) || t["delete"].as_bool() == Some(true) || t["tools"].as_object().is_some_and(|o| !o.is_empty()) || t["columns"].as_array().is_some_and(|cs| cs.iter().any(right))
        }).collect()
    };
    let reach: BTreeMap<String, Vec<String>> = kept.iter().map(|t| {
        let cols = t["columns"].as_array().map(|cs| cs.iter().filter(|c| right(c)).filter_map(|c| c["name"].as_str().map(String::from)).collect()).unwrap_or_default();
        (t["name"].as_str().unwrap_or_default().to_string(), cols)
    }).collect();
    let in_reach = |t: &str, c: &str| reach.get(t).is_some_and(|cs| cs.iter().any(|x| x == c));
    let tables: Vec<Value> = kept.into_iter().map(|mut t| {
        let name = t["name"].as_str().unwrap_or_default().to_string();
        let columns: Vec<Value> = t["columns"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| right(c)).map(|mut c| {
            // a reference names its table and column: kept when both are in reach
            if !c["fk"]["table"].as_str().zip(c["fk"]["column"].as_str()).is_some_and(|(ft, fc)| in_reach(ft, fc)) { c["fk"] = Value::Null; }
            c
        }).collect();
        // a key with a column out of reach names no row for the role: none
        let key_in_reach = t["primary_key"].as_array().is_some_and(|ks| ks.iter().all(|k| k.as_str().is_some_and(|k| in_reach(&name, k))));
        if !key_in_reach { t["primary_key"] = json!([]); }
        if !t["soft_delete"].as_str().is_some_and(|s| in_reach(&name, s)) { t["soft_delete"] = Value::Null; }
        t["columns"] = json!(columns);
        if let Some(o) = t.as_object_mut() { for k in ["tenant", "rls", "drift"] { o.remove(k); } }
        t
    }).collect();
    d["tables"] = json!(tables);
    d["roles"] = json!(d["roles"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| role.is_some() && r["name"].as_str() == role.as_deref()).collect::<Vec<_>>());
    if let Some(o) = d.as_object_mut() { o.remove("policy"); }
    d
}

/// A Postgres refusal in the words of the role files: the database is the floor under Cori's own checks, and its
/// messages name privileges and policies rather than roles.
pub(crate) fn explain_db_error(role: &str, msg: &str) -> String {
    let m = msg.trim_start_matches("Database error: ").trim_start_matches("error returned from database: ");
    if let Some(rest) = m.strip_prefix("permission denied for table ") {
        return format!("role `{role}` may not do this on `{}`: it reads or writes a column the role files do not give it (name the columns the role reads; the Schema tab lists them)", rest.trim());
    }
    if m.starts_with("new row violates row-level security policy") {
        let table = m.split('"').nth(1).unwrap_or("the table");
        return format!("role `{role}` may not write this row of `{table}`: it is outside the role's rows, or breaks a `restrict_to` or `required` rule");
    }
    if m.starts_with("permission denied for function") { return format!("role `{role}` may not call this function in a query"); }
    msg.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn write_tools_are_recognised() {
        for t in ["createItem", "updateTicket", "deleteNote", "approveChange", "rejectChange"] { assert!(super::Store::is_write_tool(t), "{t}"); }
        for t in ["getItem", "listItems", "query", "describe", "created", "updates", "create_table"] { assert!(!super::Store::is_write_tool(t), "{t}"); }
    }

    #[test]
    fn a_user_is_described_what_their_role_reaches_only() {
        use serde_json::json;
        let col = |name: &str, read: bool, create: bool, fk: serde_json::Value| json!({ "name": name, "type": "text", "pk": name == "id", "fk": fk, "read": read, "create": create, "update": false, "approval": false });
        let d = json!({ "store": "desk", "role": "agent",
            "roles": [{ "name": "agent", "description": "Answers tickets" }, { "name": "lead", "description": "Sees every ticket" }],
            "policy": { "hash": "h", "enforced_by": "postgres", "warnings": ["lead reads salary"] },
            "tables": [
                { "name": "tickets", "read": true, "delete": false, "tools": { "list": "listTickets" }, "primary_key": ["id"], "scope": "own", "tenant": "owner", "rls": true, "drift": ["title"], "soft_delete": "deleted_at",
                  "columns": [col("id", true, false, json!(null)), col("title", true, false, json!(null)), col("owner", false, false, json!(null)), col("deleted_at", false, false, json!(null)),
                              col("customer_id", true, false, json!({ "table": "customers", "column": "id" })), col("note_id", true, false, json!({ "table": "notes", "column": "body" }))] },
                { "name": "customers", "read": false, "delete": false, "tools": {}, "primary_key": ["id"], "columns": [col("id", false, false, json!(null)), col("email", false, false, json!(null))] },
                { "name": "notes", "read": false, "delete": false, "tools": { "create": "createNote" }, "primary_key": ["id"], "columns": [col("id", false, false, json!(null)), col("body", false, true, json!(null))] },
            ] });
        let r = super::reached_only(d.clone());
        let names = |v: &serde_json::Value| v.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        // the tables the role has a right on, and of each the columns it has a right on
        assert_eq!(names(&r["tables"]), ["tickets", "notes"]);
        assert_eq!(names(&r["tables"][0]["columns"]), ["id", "title", "customer_id", "note_id"]);
        assert_eq!(names(&r["tables"][1]["columns"]), ["body"]);
        // nothing names what is out of reach: a reference to it, a key or soft-delete column out of reach, the tenant column
        assert!(r["tables"][0]["columns"][2]["fk"].is_null(), "customers is out of reach");
        assert_eq!(r["tables"][0]["columns"][3]["fk"], json!({ "table": "notes", "column": "body" }), "a column the role creates is in reach");
        assert_eq!((r["tables"][0]["primary_key"].clone(), r["tables"][1]["primary_key"].clone()), (json!(["id"]), json!([])));
        assert!(r["tables"][0]["soft_delete"].is_null());
        for k in ["tenant", "rls", "drift"] { assert!(r["tables"][0].get(k).is_none(), "{k}"); }
        assert_eq!(r["tables"][0]["scope"], "own");
        // of the roles, theirs; nothing of the policy
        assert_eq!(names(&r["roles"]), ["agent"]);
        assert!(r.get("policy").is_none());
        // without a role, nothing at all
        let none = super::reached_only(json!({ "role": null, "roles": d["roles"], "tables": d["tables"], "policy": d["policy"] }));
        assert_eq!((none["tables"].clone(), none["roles"].clone()), (json!([]), json!([])));
    }

    use super::db_name;

    #[test]
    fn names_are_per_app_store_and_target() {
        assert_eq!(db_name("my-desk", "support_tickets", super::LIVE), "cori_store_my_desk_support_tickets");
        assert_eq!(db_name("my-desk", "support_tickets", super::LOCAL), "cori_store_my_desk_support_tickets_local");
        assert_eq!(db_name("my-desk", "support_tickets", "sha256:3f9a0b1c2d3e4f5061728394a5b6c7d8"), "cori_store_my_desk_support_tickets_3f9a0b1c2d3e");
        assert_ne!(db_name("a", "tickets", super::LIVE), db_name("b", "tickets", super::LIVE));
    }

    #[test]
    fn long_names_stay_under_the_postgres_limit_and_distinct() {
        let app = "a-very-long-application-identifier-for-a-department";
        let a = db_name(app, "a_store_with_a_long_name_too", "sha256:0123456789abcdef0123");
        let b = db_name(app, "a_store_with_a_long_name_tooo", "sha256:0123456789abcdef0123");
        assert!(a.len() <= 63 && b.len() <= 63, "{a} {b}");
        assert_ne!(a, b);
        assert!(a.ends_with("_0123456789ab"));
        assert_eq!(a, db_name(app, "a_store_with_a_long_name_too", "sha256:0123456789abcdef0123"));
    }
}


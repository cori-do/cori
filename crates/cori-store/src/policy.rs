// SPDX-License-Identifier: Apache-2.0
//! Applies a store's compiled access policy (`cori_store_engine::grants`) to its database, and connects the data pool as
//! `cori_runtime`: Postgres grants and row level security enforce roles and rules under Cori's own checks (target 28).
use anyhow::{Result, anyhow};
use cori_store_core::{RoleDefinition, RulesDefinition, SchemaDefinition};
use cori_store_engine::grants::{Policy, RUNTIME_ROLE};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, PgConnection, PgPool};
use std::str::FromStr;

pub use cori_store_engine::grants::{PgRoles, pg_roles, role_prefix};

/// The logins a store database is reached with.
/// - `Shared`: the databases made before stores had logins of their own (and `cori run`'s local ones): the server-wide
///   `cori_runtime` login for queries, and an owner password derived from the admin URL. Rotating the admin's password
///   changes both.
/// - `Own`: the database's own logins, `<db>_owner` and `<db>_runtime`, with random passwords the caller keeps (the server
///   keeps them as the app's credentials). The runtime login connects to that database only and switches only to its
///   store's data roles; nothing is derived from the admin credential, which may rotate freely.
#[derive(Clone, Default)]
pub enum Access {
    #[default]
    Shared,
    Own { owner: String, runtime: String },
}

// the passwords never in a log
impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Access::Shared => f.write_str("Shared"), Access::Own { .. } => f.write_str("Own { .. }") }
    }
}

impl Access {
    /// New logins of its own for a database: two random passwords (hex, so they need no escaping anywhere).
    pub fn generate() -> Self { Access::Own { owner: hex::encode(rand::random::<[u8; 32]>()), runtime: hex::encode(rand::random::<[u8; 32]>()) } }
    pub fn is_own(&self) -> bool { matches!(self, Access::Own { .. }) }
    /// The login the data pool connects as on `db_name`.
    pub fn runtime_login(&self, db_name: &str) -> String { match self { Access::Shared => RUNTIME_ROLE.to_string(), Access::Own { .. } => runtime_role(db_name) } }
}

/// The group role every login of its own belongs to: one `pg_hba.conf` line (`host all +cori_store_logins …`) admits them.
pub const LOGIN_GROUP: &str = "cori_store_logins";

/// The runtime login's password: derived from the admin URL, so it is as secret as the admin credential and needs no
/// configuration of its own. Every Cori server that shares an admin credential agrees on it.
fn runtime_password(admin_url: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(format!("cori_runtime\u{0}{admin_url}").as_bytes()))
}

/// The runtime login of one store database (`Access::Own`): `<db>_runtime` (a hash when that would exceed 63 characters).
pub fn runtime_role(db_name: &str) -> String {
    let name = format!("{db_name}_runtime");
    if name.len() <= 63 { return name; }
    use sha2::Digest;
    format!("cori_rt_{}", &hex::encode(sha2::Sha256::digest(db_name.as_bytes()))[..24])
}

/// Connection options of the runtime login on a store database: the admin URL's host, port and TLS settings, with the
/// shared `cori_runtime` user.
pub fn runtime_options(admin_url: &str, db_name: &str) -> Result<PgConnectOptions> { runtime_options_with(admin_url, db_name, &Access::Shared) }

/// `runtime_options` for the database's logins as `access` has them.
pub fn runtime_options_with(admin_url: &str, db_name: &str, access: &Access) -> Result<PgConnectOptions> {
    let o = PgConnectOptions::from_str(admin_url).map_err(|e| anyhow!("store admin URL: {e}"))?;
    let o = match access {
        Access::Shared => o.username(RUNTIME_ROLE).password(&runtime_password(admin_url)),
        Access::Own { runtime, .. } => o.username(&runtime_role(db_name)).password(runtime),
    };
    Ok(o.database(db_name).disable_statement_logging())
}

/// The login that owns one store database and runs what its builders wrote (migrations, seeds)
/// and the table-level policy: `<db>_owner` (a hash when that would exceed 63 characters). It can create objects in
/// its own database and nothing else: no CREATEDB, no CREATEROLE, no membership in the server-wide admin, so builder
/// SQL never runs with the admin's powers.
pub fn owner_role(db_name: &str) -> String {
    let name = format!("{db_name}_owner");
    if name.len() <= 63 { return name; }
    use sha2::Digest;
    format!("cori_owner_{}", &hex::encode(sha2::Sha256::digest(db_name.as_bytes()))[..24])
}

fn owner_password(admin_url: &str, db_name: &str, access: &Access) -> String {
    match access {
        Access::Own { owner, .. } => owner.clone(),
        Access::Shared => { use sha2::Digest; hex::encode(sha2::Sha256::digest(format!("cori_owner\u{0}{admin_url}\u{0}{db_name}").as_bytes())) }
    }
}

/// Connection options of a store database's owner login (the admin URL's host, port and TLS settings).
pub fn owner_options(admin_url: &str, db_name: &str) -> Result<PgConnectOptions> { owner_options_with(admin_url, db_name, &Access::Shared) }

/// `owner_options` with the owner's password as `access` has it.
pub fn owner_options_with(admin_url: &str, db_name: &str, access: &Access) -> Result<PgConnectOptions> {
    let o = PgConnectOptions::from_str(admin_url).map_err(|e| anyhow!("store admin URL: {e}"))?;
    Ok(o.username(&owner_role(db_name)).password(&owner_password(admin_url, db_name, access)).database(db_name).disable_statement_logging())
}

/// The owner login as a URL (introspection takes one). The role name is `[a-z0-9_]` and the password hex: no escaping.
pub fn owner_url(admin_url: &str, db_name: &str) -> String { owner_url_with(admin_url, db_name, &Access::Shared) }

/// `owner_url` with the owner's password as `access` has it.
pub fn owner_url_with(admin_url: &str, db_name: &str, access: &Access) -> String {
    let url = crate::db_url_for(admin_url, db_name);
    let (scheme, rest) = url.split_once("://").unwrap_or(("postgres", url.as_str()));
    let host = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    format!("{scheme}://{}:{}@{host}", owner_role(db_name), owner_password(admin_url, db_name, access))
}

/// Makes `login` a member of the logins' group (`LOGIN_GROUP`), creating the group if needed. Best effort: a group another
/// admin credential created may refuse it, which only costs the DBA a `pg_hba.conf` line per login.
async fn join_login_group(admin: &mut PgConnection, login: &str) {
    let lit = cori_store_engine::grants::literal(LOGIN_GROUP); let id = cori_store_engine::grants::ident(LOGIN_GROUP);
    let body = format!(" BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {lit}) THEN CREATE ROLE {id} NOLOGIN; END IF; \
        EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END ");
    let created = sqlx::raw_sql(AssertSqlSafe(format!("DO {}", cori_store_engine::grants::dollar_quoted(&body)))).execute(&mut *admin).await;
    let granted = sqlx::query(AssertSqlSafe(format!("GRANT {id} TO {}", cori_store_engine::grants::ident(login)))).execute(&mut *admin).await;
    if let Some(e) = created.err().map(|e| e.to_string()).or_else(|| granted.err().map(|e| e.to_string())) { tracing::warn!(login, group = LOGIN_GROUP, error = %e, "store login left out of the logins' group"); }
}

/// Creates (or fixes) the owner login of `db_name`, and makes the admin a member so it can create the database owned by
/// it and hand it existing objects. A role of that name holding powers of its own is refused.
pub async fn ensure_owner(admin: &PgPool, admin_url: &str, db_name: &str) -> Result<String> { ensure_owner_with(admin, admin_url, db_name, &Access::Shared).await }

/// `ensure_owner` with the owner's password as `access` has it (set when the login does not work with it).
pub async fn ensure_owner_with(admin: &PgPool, admin_url: &str, db_name: &str, access: &Access) -> Result<String> {
    let owner = owner_role(db_name);
    let lit = cori_store_engine::grants::literal(&owner); let id = cori_store_engine::grants::ident(&owner);
    let body = format!(" BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {lit}) THEN CREATE ROLE {id} LOGIN NOINHERIT NOCREATEDB NOCREATEROLE; END IF; \
        EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END ");
    sqlx::raw_sql(AssertSqlSafe(format!("DO {}", cori_store_engine::grants::dollar_quoted(&body)))).execute(admin).await.map_err(|e| anyhow!("creating the store owner login (the store admin needs CREATEROLE): {e}"))?;
    let powers: Option<(bool,)> = sqlx::query_as("SELECT rolsuper OR rolbypassrls OR rolreplication OR rolcreaterole OR rolcreatedb FROM pg_roles WHERE rolname = $1").bind(&owner).fetch_optional(admin).await?;
    if powers.map(|p| p.0).unwrap_or(false) { return Err(anyhow!("role `{owner}` exists with server-level powers (superuser, createrole, createdb, bypassrls or replication): drop it or remove them")); }
    let works = match owner_options_with(admin_url, db_name, access) { Ok(o) => sqlx::PgConnection::connect_with(&o.database("postgres")).await.map(drop).is_ok(), Err(_) => false };
    if !works { sqlx::query(AssertSqlSafe(format!("ALTER ROLE {id} LOGIN PASSWORD '{}'", owner_password(admin_url, db_name, access)))).execute(admin).await.map_err(|e| anyhow!("store owner login `{owner}`: {e}"))?; }
    // membership lets the admin create the database owned by the login and move objects to it (PG 16 requires it)
    let _ = sqlx::query(AssertSqlSafe(format!("GRANT {id} TO CURRENT_USER"))).execute(admin).await;
    if access.is_own() { join_login_group(&mut *admin.acquire().await?, &owner).await; }
    Ok(owner)
}

/// Sets new passwords on a database's own logins (`Access::Own`): the admin alone may. Connections already open keep
/// working; new ones need the new passwords.
pub async fn set_passwords(admin_url: &str, db_name: &str, access: &Access) -> Result<()> {
    let Access::Own { owner, runtime } = access else { return Err(anyhow!("`{db_name}` has no logins of its own")) };
    let admin = PgPoolOptions::new().max_connections(1).connect(admin_url).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    for (login, pw) in [(owner_role(db_name), owner), (runtime_role(db_name), runtime)] {
        sqlx::query(AssertSqlSafe(format!("ALTER ROLE {} LOGIN PASSWORD '{pw}'", cori_store_engine::grants::ident(&login)))).execute(&admin).await.map_err(|e| anyhow!("store login `{login}`: {e}"))?;
    }
    admin.close().await;
    Ok(())
}

/// Hands a store database to its owner login: the database itself, the objects an earlier version created as the admin,
/// CREATE on the public schema; and closes it to PUBLIC (other stores' logins cannot even connect).
pub async fn adopt_database(admin: &PgPool, admin_url: &str, db_name: &str, owner: &str) -> Result<()> {
    let (db, own) = (cori_store_engine::grants::ident(db_name), cori_store_engine::grants::ident(owner));
    for sql in [
        format!("ALTER DATABASE {db} OWNER TO {own}"),
        format!("REVOKE CONNECT, TEMPORARY ON DATABASE {db} FROM PUBLIC"),
        format!("GRANT CONNECT, TEMPORARY ON DATABASE {db} TO {own}"),
        format!("GRANT CONNECT ON DATABASE {db} TO CURRENT_USER"),
    ] { sqlx::query(AssertSqlSafe(sql.as_str())).execute(admin).await.map_err(|e| anyhow!("{sql}: {e}"))?; }
    let inside = PgPoolOptions::new().max_connections(1).connect(&crate::db_url_for(admin_url, db_name)).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    let lit = cori_store_engine::grants::literal(owner);
    let body = format!(r#"
DECLARE r record; me oid := (SELECT oid FROM pg_roles WHERE rolname = current_user);
BEGIN
  BEGIN
    EXECUTE format('GRANT CREATE, USAGE ON SCHEMA public TO %I', {lit});
  EXCEPTION WHEN insufficient_privilege THEN RAISE NOTICE 'cori: CREATE on schema public not granted to the owner (not the schema owner)';
  END;
  FOR r IN SELECT c.relname, c.relkind FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
           WHERE n.nspname = 'public' AND c.relowner = me AND c.relkind IN ('r', 'p', 'v', 'm', 'f') LOOP
    EXECUTE format('ALTER %s public.%I OWNER TO %I', CASE r.relkind WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'MATERIALIZED VIEW' WHEN 'f' THEN 'FOREIGN TABLE' ELSE 'TABLE' END, r.relname, {lit});
  END LOOP;
  FOR r IN SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
           WHERE n.nspname = 'public' AND c.relowner = me AND c.relkind = 'S'
             AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype IN ('a', 'i')) LOOP
    EXECUTE format('ALTER SEQUENCE public.%I OWNER TO %I', r.relname, {lit});
  END LOOP;
  FOR r IN SELECT p.oid::regprocedure AS f FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'public' AND p.proowner = me LOOP
    EXECUTE format('ALTER ROUTINE %s OWNER TO %I', r.f, {lit});
  END LOOP;
  FOR r IN SELECT t.typname FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
           WHERE n.nspname = 'public' AND t.typowner = me AND t.typtype IN ('e', 'd') LOOP
    EXECUTE format('ALTER TYPE public.%I OWNER TO %I', r.typname, {lit});
  END LOOP;
END
"#);
    let r = sqlx::raw_sql(AssertSqlSafe(format!("DO {}", cori_store_engine::grants::dollar_quoted(&body)))).execute(&inside).await;
    inside.close().await;
    r.map_err(|e| anyhow!("handing the store database to its owner: {e}"))?;
    Ok(())
}

/// Compiles the policy of a store database.
pub fn compile(app: &str, store: &str, schema: &SchemaDefinition, rules: &RulesDefinition, roles: &[RoleDefinition]) -> Result<Policy> {
    cori_store_engine::grants::compile(schema, rules, roles, &role_prefix(app, store)).map_err(|e| anyhow!("{e}"))
}

/// Creates or updates the server-wide roles: the runtime login (no privilege of its own, `NOINHERIT`, a member of
/// every data role) and the policy's data and applier roles. Needs `CREATEROLE`; runs under the admin lock (`admin_part`).
async fn ensure_roles(admin: &mut PgConnection, admin_url: &str, db_name: &str, policy: &Policy, access: &Access) -> Result<()> {
    let login = access.runtime_login(db_name);
    let (lit, id) = (cori_store_engine::grants::literal(&login), cori_store_engine::grants::ident(&login));
    let create = format!(
        "DO $cori$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {lit}) THEN CREATE ROLE {id} LOGIN NOINHERIT NOCREATEDB NOCREATEROLE; END IF; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $cori$"
    );
    sqlx::raw_sql(AssertSqlSafe(create.as_str())).execute(&mut *admin).await.map_err(|e| anyhow!("creating the runtime login (the store admin needs CREATEROLE): {e}"))?;
    if access.is_own() {
        let powers: Option<(bool,)> = sqlx::query_as("SELECT rolsuper OR rolbypassrls OR rolreplication OR rolcreaterole OR rolcreatedb FROM pg_roles WHERE rolname = $1").bind(&login).fetch_optional(&mut *admin).await?;
        if powers.map(|p| p.0).unwrap_or(false) { return Err(anyhow!("role `{login}` exists with server-level powers (superuser, createrole, createdb, bypassrls or replication): drop it or remove them")); }
    }
    // set the password only when the login does not work yet: another admin credential on the same server may own the shared one
    let works = match runtime_options_with(admin_url, "postgres", access) { Ok(o) => sqlx::PgConnection::connect_with(&o.username(&login)).await.map(|c| drop(c)).is_ok(), Err(_) => false };
    if !works {
        let pw = match access { Access::Shared => runtime_password(admin_url), Access::Own { runtime, .. } => runtime.clone() };
        sqlx::query(AssertSqlSafe(format!("ALTER ROLE {id} LOGIN NOINHERIT PASSWORD '{pw}'"))).execute(&mut *admin).await.map_err(|e| anyhow!("runtime login `{login}` (created by another admin credential?): {e}"))?;
    }
    for s in cori_store_engine::grants::role_statements(policy, &login) {
        sqlx::raw_sql(AssertSqlSafe(s.as_str())).execute(&mut *admin).await.map_err(|e| anyhow!("store roles: {e}"))?;
    }
    if access.is_own() { join_login_group(admin, &login).await; }
    Ok(())
}

/// The admin's part of applying a policy (`admin_part_locked`), one admin at a time: two sessions changing one role or
/// one database's grants at once fail ("tuple concurrently updated"), as the stores a new server opens together would,
/// each setting the shared runtime login's password. A session lock on the admin URL's database (every replica of a
/// server has the same), held by the one connection that runs the statements and released before it closes.
async fn admin_part(admin_url: &str, db_name: &str, policy: &Policy, access: &Access) -> Result<()> {
    // its statements carry passwords: never logged (nor its wait for the lock, as a slow statement)
    let o = PgConnectOptions::from_str(admin_url).map_err(|e| anyhow!("store admin URL: {e}"))?.disable_statement_logging();
    let mut admin = PgConnection::connect_with(&o).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    sqlx::query("SELECT pg_advisory_lock(hashtext('cori_store_admin'))").execute(&mut admin).await.map_err(|e| anyhow!("store admin lock: {e}"))?;
    let r = admin_part_locked(&mut admin, admin_url, db_name, policy, access).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('cori_store_admin'))").execute(&mut admin).await;
    let _ = admin.close().await;
    r
}

/// The server-wide roles (`ensure_roles`), then the runtime login's CONNECT on the database (the shared login's taken
/// back when the database has logins of its own) and the catalog hardening inside it.
async fn admin_part_locked(admin: &mut PgConnection, admin_url: &str, db_name: &str, policy: &Policy, access: &Access) -> Result<()> {
    ensure_roles(admin, admin_url, db_name, policy, access).await?;
    let login = access.runtime_login(db_name);
    let dbi = cori_store_engine::grants::ident(db_name);
    sqlx::query(AssertSqlSafe(format!("GRANT CONNECT ON DATABASE {dbi} TO {}", cori_store_engine::grants::ident(&login)))).execute(&mut *admin).await.map_err(|e| anyhow!("runtime login CONNECT: {e}"))?;
    // a database with logins of its own no longer admits the shared runtime login
    if access.is_own() {
        let body = format!(" BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {}) THEN REVOKE CONNECT ON DATABASE {dbi} FROM {}; END IF; END ", cori_store_engine::grants::literal(RUNTIME_ROLE), cori_store_engine::grants::ident(RUNTIME_ROLE));
        sqlx::raw_sql(AssertSqlSafe(format!("DO {}", cori_store_engine::grants::dollar_quoted(&body)))).execute(&mut *admin).await.map_err(|e| anyhow!("closing `{db_name}` to the shared runtime login: {e}"))?;
    }
    let inside = PgPoolOptions::new().max_connections(1).connect(&crate::db_url_for(admin_url, db_name)).await.map_err(|e| anyhow!("store admin connection: {e}"))?;
    sqlx::raw_sql(AssertSqlSafe(cori_store_engine::grants::catalog_hardening(&login))).execute(&inside).await.map_err(|e| anyhow!("catalog hardening: {e}"))?;
    inside.close().await;
    Ok(())
}

/// The policy hash recorded in a store database, if any.
pub async fn stored_hash(db: &mut PgConnection) -> Result<Option<String>> {
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relname = 'cori_policy'").fetch_optional(&mut *db).await?;
    if exists.is_none() { return Ok(None); }
    Ok(sqlx::query_as::<_, (String,)>("SELECT hash FROM cori_policy WHERE id = 1").fetch_optional(&mut *db).await?.map(|r| r.0))
}

/// Applies a compiled policy to a store database unless its hash is already recorded there: the roles (server wide) and
/// the database's grants first, one admin at a time (`admin_part`), then every statement and the new hash in one
/// transaction. All of it holds the database's policy lock, so two servers opening the same database take turns: the
/// second finds the hash recorded and applies nothing. Returns whether anything was applied.
pub async fn apply(admin_url: &str, db_name: &str, policy: &Policy) -> Result<bool> { apply_with(admin_url, db_name, policy, &Access::Shared).await }

/// `apply` for the database's logins as `access` has them. A database's own runtime login enters the recorded hash, so
/// that a database moving to logins of its own has its policy applied again once, for its new login.
pub async fn apply_with(admin_url: &str, db_name: &str, policy: &Policy, access: &Access) -> Result<bool> {
    // the table-level statements run as the database's owner (it owns the tables); roles, the runtime login's CONNECT
    // and the catalog hardening need the admin
    let login = access.runtime_login(db_name);
    let hash = if access.is_own() { format!("{}+{login}", policy.hash) } else { policy.hash.clone() };
    let mut db = PgConnection::connect_with(&owner_options_with(admin_url, db_name, access)?).await.map_err(|e| anyhow!("store owner connection: {e}"))?;
    if stored_hash(&mut db).await?.as_deref() == Some(hash.as_str()) {
        let _ = db.close().await;
        return Ok(false);
    }
    // one opener at a time, from the roles to the hash: another opener's catalog hardening changes the public schema's
    // privileges as these statements do, and the second of two such changes at once fails ("tuple concurrently
    // updated"). A session lock on this database, taken before the admin lock and never while holding it
    sqlx::query("SELECT pg_advisory_lock(hashtext('cori_policy'))").execute(&mut db).await.map_err(|e| anyhow!("store policy lock: {e}"))?;
    let r = apply_locked(&mut db, admin_url, db_name, policy, access, &hash).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('cori_policy'))").execute(&mut db).await;
    let _ = db.close().await;
    r
}

/// `apply_with` holding the database's policy lock: an opener that waited for it finds the hash recorded.
async fn apply_locked(db: &mut PgConnection, admin_url: &str, db_name: &str, policy: &Policy, access: &Access, hash: &str) -> Result<bool> {
    if stored_hash(&mut *db).await?.as_deref() == Some(hash) { return Ok(false); }
    admin_part(admin_url, db_name, policy, access).await?;
    let mut tx = db.begin().await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS cori_policy (id INT PRIMARY KEY DEFAULT 1 CHECK (id = 1), hash TEXT NOT NULL, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())").execute(&mut *tx).await?;
    for s in &policy.statements {
        sqlx::raw_sql(AssertSqlSafe(s.as_str())).execute(&mut *tx).await.map_err(|e| anyhow!("access policy: {e}\n  in: {}", s.lines().next().unwrap_or("")))?;
    }
    sqlx::query("INSERT INTO cori_policy (id, hash) VALUES (1, $1) ON CONFLICT (id) DO UPDATE SET hash = $1, applied_at = now()").bind(hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

/// Parses a store definition and compiles its policy.
pub fn compile_def(app: &str, def: &crate::StoreDef) -> Result<(SchemaDefinition, RulesDefinition, Vec<RoleDefinition>, Policy)> {
    let schema = SchemaDefinition::from_yaml(&def.schema_yaml).map_err(|e| anyhow!("schema.yaml: {e}"))?;
    let rules = crate::parse_rules(&def.rules_yaml)?;
    let mut roles = Vec::new();
    for (rname, yaml) in &def.roles_yaml {
        if rname == crate::INSPECTOR { return Err(anyhow!("role `{rname}` is reserved: Cori gives it to the app's owner, builders and admins")); }
        roles.push(RoleDefinition::from_yaml(yaml).map_err(|e| anyhow!("role {rname}: {e}"))?);
    }
    roles.push(inspector_role(&schema)?);
    let policy = cori_store_engine::grants::compile(&schema, &rules, &roles, &def.role_prefix(app)).map_err(|e| anyhow!("{e}"))?;
    Ok((schema, rules, roles, policy))
}

/// The inspector role every store gets besides its own: every column of every table, every row (`scope: all`), no
/// write. The server gives it to the app's owner, builders and admins, so they can look at the data of their app without
/// holding one of its business roles; the database still enforces it (grants and row level security like any role).
/// Tables rules.yaml does not name stay hidden from it as from every role.
fn inspector_role(schema: &SchemaDefinition) -> Result<RoleDefinition> {
    let tables: serde_json::Map<String, serde_json::Value> = schema.tables.iter()
        .filter(|t| !cori_store_engine::grants::BOOKKEEPING_TABLES.contains(&t.name.as_str()))
        .map(|t| (t.name.clone(), serde_json::json!({ "readable": "*", "scope": "all" }))).collect();
    let yaml = serde_yaml::to_string(&serde_json::json!({ "name": crate::INSPECTOR, "description": "Read-only inspection by the app's owner, builders and admins", "tables": tables }))?;
    RoleDefinition::from_yaml(&yaml).map_err(|e| anyhow!("inspector role: {e}"))
}

/// Compiles and applies the policy of a store definition (`provision` and the admin tools call it). Returns the
/// compiler's warnings.
pub async fn apply_def(admin_url: &str, app: &str, def: &crate::StoreDef) -> Result<Vec<String>> { apply_def_with(admin_url, app, def, &Access::Shared).await }

/// `apply_def` for the database's logins as `access` has them.
pub async fn apply_def_with(admin_url: &str, app: &str, def: &crate::StoreDef, access: &Access) -> Result<Vec<String>> {
    let (_, _, _, policy) = compile_def(app, def)?;
    apply_with(admin_url, &def.db_name, &policy, access).await?;
    Ok(policy.warnings)
}

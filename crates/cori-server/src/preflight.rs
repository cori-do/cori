// SPDX-License-Identifier: Apache-2.0
//! `cori preflight`: what a server checks of the deployment it is configured for, before anyone relies on it. It reaches
//! every service its settings name, with the server's own credentials, and says for each what answered and, when something
//! did not, what to fix. `cori serve --preflight` runs it where the server runs, with the server's environment.
use serde::Serialize;

/// One thing checked: `ok`, `failed` (with what to do) or `skipped` (not configured, or not needed here).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status { Ok, Failed, Skipped }

impl Check {
    pub fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self { Self { name: name.into(), status: Status::Ok, detail: detail.into(), fix: None } }
    pub fn failed(name: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self { Self { name: name.into(), status: Status::Failed, detail: detail.into(), fix: Some(fix.into()) } }
    pub fn skipped(name: impl Into<String>, why: impl Into<String>) -> Self { Self { name: name.into(), status: Status::Skipped, detail: why.into(), fix: None } }
    pub fn is_failed(&self) -> bool { self.status == Status::Failed }
}

/// Every check, in the order a deployment depends on them. Nothing is changed: the platform database is read, not
/// migrated; the bucket gets one object that is deleted again; the hook hears a `ping`; Vault wraps a throwaway key.
pub async fn run(cfg: &crate::ServerConfig) -> Vec<Check> {
    let mut out = Vec::new();
    for p in cfg.production_problems() { out.push(Check::failed("settings", p, "see the setting named; `cori serve` refuses to start with it off loopback")); }
    out.push(match cori_engine::http::check() {
        Ok(0) => Check::skipped("CA bundle", "CORI_CA_FILE is not set: the system's roots only"),
        Ok(n) => Check::ok("CA bundle", format!("{n} certificate(s) from CORI_CA_FILE, besides the system's roots")),
        Err(e) => Check::failed("CA bundle", e, "CORI_CA_FILE names a PEM file of one or more certificates"),
    });
    out.push(platform_database(&cfg.database_url).await);
    let clusters = cfg.store_clusters();
    if clusters.is_empty() { out.push(Check::skipped("store server", "CORI_STORE_DATABASE_URL is not set: apps' stores are answered by their dev mocks")); }
    for (name, url) in &clusters { out.push(store_server(name, url).await); }
    out.push(issuer(cfg).await);
    out.push(hook(cfg).await);
    out.push(people_directory(cfg).await);
    out.push(access_webhook(cfg).await);
    out.push(temporal(cfg).await);
    let bundles = crate::bundles::BundleStore::from_env(cfg.data_dir.clone());
    out.push(match &bundles {
        Err(e) => Check::failed("bundle store", format!("{e:#}"), "fix the CORI_BUNDLE_STORE and CORI_S3_* settings"),
        Ok(b) => match b.check().await {
            None => Check::skipped("bundle store", "CORI_BUNDLE_STORE is not set: bundles stay in the data directory (one replica)"),
            Some(Ok(d)) => Check::ok("bundle store", d),
            Some(Err(e)) => Check::failed("bundle store", format!("{e:#}"), "the bucket's policy grants this key GetObject, PutObject and DeleteObject on the prefix; CORI_S3_ENDPOINT, CORI_S3_PATH_STYLE and CORI_S3_SSE match the server"),
        },
    });
    // the files runs are given: a bucket of their own, on the same S3 server and with the same key as the bundles'
    if let Ok(b) = &bundles {
        out.push(match crate::bundles::FileStore::from_env(cfg.data_dir.clone(), b) {
            Err(e) => Check::failed("file store", format!("{e:#}"), "fix the CORI_FILE_STORE and CORI_S3_* settings"),
            Ok(f) => match (f.unavailable(), f.check().await) {
                (Some(why), _) => Check::failed("file store", why, "set CORI_FILE_STORE=s3://<bucket>[/<prefix>]: a bucket of its own, on the S3 server CORI_S3_* names, that this key may read, write and delete in"),
                (None, None) => Check::skipped("file store", "CORI_FILE_STORE is not set: files stay in the data directory (one replica)"),
                (None, Some(Ok(d))) => Check::ok("file store", d),
                (None, Some(Err(e))) => Check::failed("file store", format!("{e:#}"), "the bucket's policy grants this key GetObject, PutObject and DeleteObject on the prefix; CORI_S3_ENDPOINT, CORI_S3_PATH_STYLE and CORI_S3_SSE match the server"),
            },
        });
    }
    out.extend(models(cfg).await);
    out.push(fronts(cfg).await);
    out.push(sandboxes().await);
    out.extend(cfg.modules.preflight().await);
    out
}

async fn platform_database(url: &str) -> Check {
    use sqlx::Connection;
    const NAME: &str = "platform database";
    if url.is_empty() { return Check::failed(NAME, "CORI_DATABASE_URL is not set", "a postgres:// URL of a login owning (or allowed to create in) the platform's schema"); }
    let mut c = match sqlx::PgConnection::connect(url).await { Ok(c) => c, Err(e) => return Check::failed(NAME, format!("{e}"), "the host, port, database, login, password and sslmode of CORI_DATABASE_URL") };
    let row: Result<(String, String, Option<String>, bool, Option<bool>), _> = sqlx::query_as("SELECT current_user::text, current_database()::text, current_schema()::text, has_database_privilege(current_database(), 'CONNECT'), CASE WHEN current_schema() IS NULL THEN NULL ELSE has_schema_privilege(current_schema(), 'CREATE') END").fetch_one(&mut c).await;
    let (user, db, schema, _, create) = match row { Ok(r) => r, Err(e) => return Check::failed(NAME, e.to_string(), "the login must be able to read the catalogs") };
    let Some(schema) = schema else { return Check::failed(NAME, format!("login `{user}` on `{db}` has no schema to work in (search_path)"), format!("CREATE SCHEMA cori AUTHORIZATION {user}; and `?options=-c%20search_path%3Dcori` in the URL")) };
    let applied: Option<i64> = sqlx::query_scalar("SELECT max(version) FROM platform_migrations").fetch_one(&mut c).await.ok().flatten();
    let latest = crate::db::latest_migration();
    match (create, applied) {
        (Some(false), a) if a.unwrap_or(0) < latest => Check::failed(NAME, format!("login `{user}` may not create tables in schema `{schema}` of `{db}`, and migrations are pending"), format!("GRANT CREATE ON SCHEMA {schema} TO {user}; (or make it the schema's owner)")),
        (_, Some(a)) if a > latest => Check::failed(NAME, format!("`{db}` is at schema version {a}, newer than this server's {latest}"), "run the newer Cori, or restore the backup taken before the upgrade"),
        (_, a) => Check::ok(NAME, format!("`{db}`, schema `{schema}`, as `{user}`; schema version {} of {latest}", a.map(|v| v.to_string()).unwrap_or_else(|| "none yet (migrated at start)".into()))),
    }
}

async fn store_server(name: &str, url: &str) -> Check {
    use sqlx::Connection;
    let label = if name == crate::MAIN_STORE_CLUSTER { "store server".to_string() } else { format!("store server `{name}`") };
    let mut c = match sqlx::PgConnection::connect(url).await { Ok(c) => c, Err(e) => return Check::failed(label, format!("{e}"), "the host, port, login, password and sslmode of the store admin URL") };
    let r: Result<(String, bool, bool, bool, String), _> = sqlx::query_as("SELECT rolname::text, rolsuper, rolcreatedb, rolcreaterole, current_setting('server_version') FROM pg_roles WHERE rolname = current_user").fetch_one(&mut c).await;
    match r {
        Ok((user, sup, db, role, version)) if sup || (db && role) => {
            let group: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)").bind(cori_store::policy::LOGIN_GROUP).fetch_one(&mut c).await.unwrap_or(false);
            Check::ok(label, format!("`{user}` ({}), Postgres {version}; each store database gets logins of its own{}", if sup { "superuser" } else { "CREATEDB, CREATEROLE" }, if group { format!(", members of `{}`", cori_store::policy::LOGIN_GROUP) } else { String::new() }))
        }
        Ok((user, ..)) => Check::failed(label, format!("`{user}` may not create databases and roles"), format!("ALTER ROLE {user} CREATEDB CREATEROLE; (or a superuser)")),
        Err(e) => Check::failed(label, e.to_string(), "the login must be able to read pg_roles"),
    }
}

async fn issuer(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "issuer";
    let crate::auth::AuthMode::Builtin { issuer, audience, subject_claim, .. } = &cfg.auth else { return Check::skipped(NAME, "auth is not builtin") };
    let app_roles = match crate::auth::AppRolesClaim::from_env() { Ok(c) => c, Err(e) => return Check::failed(NAME, e, "where your issuer's tokens carry each app's roles: CORI_AUTH_APP_ROLES_CLAIM (a dot path naming the app's client, or one claim listing every app's roles) with CORI_AUTH_APP_ROLES_PREFIX") };
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(10)).build().unwrap_or_default();
    let discovery = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let d: serde_json::Value = match http.get(&discovery).send().await { Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(), Ok(r) => return Check::failed(NAME, format!("{discovery}: http {}", r.status()), "CORI_AUTH_ISSUER is the realm's URL (Keycloak: https://<host>/realms/<realm>)"), Err(e) => return Check::failed(NAME, format!("{discovery}: {e}"), "the issuer must be reachable from the server (and its certificate trusted: CORI_CA_FILE)") };
    if d["issuer"].as_str() != Some(issuer.trim_end_matches('/')) && d["issuer"].as_str() != Some(issuer.as_str()) { return Check::failed(NAME, format!("the discovery document names issuer `{}`", d["issuer"].as_str().unwrap_or("")), "CORI_AUTH_ISSUER must be exactly the issuer tokens carry (`iss`)"); }
    let keys = match d["jwks_uri"].as_str() { Some(u) => http.get(u).send().await.ok().map(|r| r.status().is_success()).unwrap_or(false), None => false };
    if !keys { return Check::failed(NAME, "its signing keys (jwks_uri) did not load", "the issuer's keys must be reachable from the server"); }
    Check::ok(NAME, format!("{issuer}: discovery and keys; tokens must carry aud `{}`, the person in `{subject_claim}`, and app roles in {app_roles}", audience.as_deref().unwrap_or("(unchecked: CORI_AUTH_AUDIENCE is not set)")))
}

async fn hook(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "client hook";
    if !matches!(cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return Check::skipped(NAME, "auth is not builtin"); }
    let Some(url) = &cfg.client_hook else { return Check::failed(NAME, "CORI_AUTH_CLIENT_HOOK is not set", "every app gets its OIDC client from the organization's receiver of the client hook (`cori identity test-hook <url>` checks one)") };
    match crate::client_hook::ping_url(url, cfg.client_hook_secret.as_deref()).await {
        Ok(a) if (200..300).contains(&a.status) => Check::ok(NAME, format!("{url} answered the signed ping ({}){}", a.status, if cfg.identity.roles { "; it hears of roles one by one" } else { "" })),
        Ok(a) => Check::failed(NAME, format!("{url} answered {} to the ping: {}", a.status, a.note()), "the receiver checks the signature with CORI_AUTH_CLIENT_HOOK_SECRET and answers 200 to events it does not handle (`cori identity test-hook`)"),
        Err(e) => Check::failed(NAME, e, "the receiver must be reachable from the server (and its certificate trusted: CORI_CA_FILE)"),
    }
}

/// The organization's own directory of people, when it has one: a signed `people.list` answered, its first page naming
/// each person by id.
async fn people_directory(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "people directory";
    let Some(url) = &cfg.people_url else { return Check::skipped(NAME, "CORI_IDENTITY_PEOPLE_URL is not set: the people are the client hook receiver's, when it lists them, else those Cori has seen") };
    if !matches!(cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return Check::skipped(NAME, "auth is not builtin"); }
    let fix = "the webhook checks the signature with CORI_IDENTITY_PEOPLE_SECRET and answers people.list (`cori identity test-people`)";
    match crate::client_hook::ask_url(url, cfg.people_secret.as_deref(), "people.list", serde_json::json!({ "by": "cori-preflight", "cursor": null }), None).await {
        Ok(a) if (200..300).contains(&a.status) => match a.body["people"].as_array() {
            None => Check::failed(NAME, format!("{url} answered people.list without `people`: {}", a.body), fix),
            Some(people) => {
                let unnamed = people.iter().filter(|p| p["id"].as_str().is_none_or(|id| id.trim().is_empty())).count();
                if unnamed > 0 { return Check::failed(NAME, format!("{url}: {unnamed} of the {} people on its first page have no `id`", people.len()), "each person's `id` is the subject (`sub`) of their tokens (Keycloak: the user's id)"); }
                let more = if a.body["next"].as_str().is_some_and(|n| !n.is_empty()) { ", more pages follow" } else { "" };
                Check::ok(NAME, format!("{url} answered the signed people.list: {} people on its first page{more}", people.len()))
            }
        },
        Ok(a) => Check::failed(NAME, format!("{url} answered {} to people.list: {}", a.status, a.note()), fix),
        Err(e) => Check::failed(NAME, e, "the webhook must be reachable from the server (and its certificate trusted: CORI_CA_FILE)"),
    }
}

/// The organization's own webhook for apps' access, when it has one: a signed `ping` answered 2xx (it answers 200 to an
/// event it does not handle, as the hook's receiver does).
async fn access_webhook(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "access webhook";
    let Some(url) = &cfg.access_url else { return Check::skipped(NAME, "CORI_IDENTITY_ACCESS_URL is not set: apps' roles are given through the client hook's receiver, when it takes them, else in your identity provider") };
    if !matches!(cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return Check::skipped(NAME, "auth is not builtin"); }
    let message = if cfg.access_message.is_some() { "; the console shows your message where roles are given" } else { "" };
    match crate::client_hook::ping_url(url, cfg.access_secret.as_deref()).await {
        Ok(a) if (200..300).contains(&a.status) => Check::ok(NAME, format!("{url} answered the signed ping ({}): it takes access.granted and access.revoked{message}", a.status)),
        Ok(a) => Check::failed(NAME, format!("{url} answered {} to the ping: {}", a.status, a.note()), "the webhook checks the signature with CORI_IDENTITY_ACCESS_SECRET and answers 200 to events it does not handle (`cori identity test-access`)"),
        Err(e) => Check::failed(NAME, e, "the webhook must be reachable from the server (and its certificate trusted: CORI_CA_FILE)"),
    }
}

async fn temporal(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "temporal";
    let opts = match cori_engine::runtime::connection_options(&cfg.temporal, "cori-preflight") { Ok(o) => o, Err(e) => return Check::failed(NAME, format!("{e:#}"), "CORI_TEMPORAL_ADDRESS and CORI_TEMPORAL_TLS_*") };
    match tokio::time::timeout(std::time::Duration::from_secs(15), temporalio_client::Connection::connect(opts)).await {
        Ok(Ok(_)) => Check::ok(NAME, format!("{} (namespace `{}`)", cfg.temporal, cfg.namespace)),
        Ok(Err(e)) => Check::failed(NAME, format!("{}: {e}", cfg.temporal), "the frontend's address, its TLS settings and API key"),
        Err(_) => Check::failed(NAME, format!("{}: no answer within 15 s", cfg.temporal), "the frontend must be reachable from the server"),
    }
}

/// The default models: the starter model, and each tier's model of the providers file, asked one structured question.
async fn models(cfg: &crate::ServerConfig) -> Vec<Check> {
    let mut out = Vec::new();
    let egress = cfg.registry_egress();
    if let Some(s) = &cfg.llm_starter {
        out.push(match cori_engine::activities::llm_probe("openai", &s.base_url, &s.key, &s.model, egress).await {
            Ok(v) if v.get("ok").is_some() => Check::ok("default model", format!("starter `{}` at {} answers structured output", s.model, s.base_url)),
            Ok(v) => Check::failed("default model", format!("starter `{}` answered {v}", s.model), "the model must follow a JSON schema (response_format json_schema, else json_object)"),
            Err(e) => Check::failed("default model", format!("starter `{}`: {e:#}", s.model), "CORI_LLM_STARTER_BASE_URL (the API's /v1), _MODEL and _KEY; the endpoint's certificate trusted (CORI_CA_FILE)"),
        });
    }
    if let Some(path) = std::env::var("CORI_LLM_PROVIDERS_FILE").ok().filter(|p| !p.trim().is_empty()) {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_yaml::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string())) {
            Err(e) => out.push(Check::failed("LLM providers file", format!("{path}: {e}"), "a YAML or JSON file of `providers` and `tiers`")),
            Ok(f) => {
                let empty = serde_json::Map::new();
                for (tier, t) in f["tiers"].as_object().unwrap_or(&empty) {
                    let (prov, model) = (t["provider"].as_str().unwrap_or(""), t["model"].as_str().unwrap_or(""));
                    let Some(p) = f["providers"].as_array().and_then(|a| a.iter().find(|p| p["name"].as_str() == Some(prov))) else { out.push(Check::failed(format!("model ({tier})"), format!("provider `{prov}` is not declared"), "declare it under `providers`")); continue };
                    let kind = p["kind"].as_str().unwrap_or("openai");
                    let base = p["base_url"].as_str().map(String::from).unwrap_or_else(|| if kind == "anthropic" { "https://api.anthropic.com/v1".into() } else { "https://api.openai.com/v1".into() });
                    let key = p["key_env"].as_str().and_then(|v| std::env::var(v).ok()).unwrap_or_default();
                    out.push(match cori_engine::activities::llm_probe(kind, &base, &key, model, egress).await {
                        Ok(v) if v.get("ok").is_some() => Check::ok(format!("model ({tier})"), format!("`{prov}`/`{model}` answers structured output")),
                        Ok(v) => Check::failed(format!("model ({tier})"), format!("`{prov}`/`{model}` answered {v}"), "the model must follow a JSON schema"),
                        Err(e) => Check::failed(format!("model ({tier})"), format!("`{prov}`/`{model}`: {e:#}"), "its base_url, key (key_env) and model id; the endpoint's certificate trusted (CORI_CA_FILE)"),
                    });
                }
            }
        }
    }
    if out.is_empty() { out.push(Check::skipped("default model", "no starter model nor providers file: an admin adds a provider in the console before `llm` steps run")); }
    out
}

async fn fronts(cfg: &crate::ServerConfig) -> Check {
    const NAME: &str = "fronts";
    let Some(f) = &cfg.fronts_url else { return Check::failed(NAME, "CORI_FRONTS_URL is not set: every front would be served from the console's origin, where its code can read the console's and other fronts' tokens", "CORI_FRONTS_URL=https://*.apps.<domain>, with a wildcard DNS name and certificate") };
    let origin = f.origin("cori-preflight");
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
    // any answer will do: the name resolved, and the certificate is valid for it
    match http.get(format!("{origin}/healthz")).send().await {
        Ok(r) => Check::ok(NAME, format!("{} : {origin} resolves, its certificate is valid (http {})", f.pattern(), r.status().as_u16())),
        Err(e) => Check::failed(NAME, format!("{origin}: {e}"), "a wildcard DNS record and a wildcard certificate for the fronts' domain"),
    }
}

async fn sandboxes() -> Check {
    const NAME: &str = "sandboxes";
    let Some(s) = cori_engine::sandbox::SandboxConfig::from_env() else { return Check::skipped(NAME, "CORI_SANDBOX_URL is not set: builders' code runs on the workers and the server, next to their credentials") };
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap_or_default();
    let mut down = Vec::new();
    for u in &s.urls { if !http.get(format!("{}/healthz", u.trim_end_matches('/'))).send().await.map(|r| r.status().is_success()).unwrap_or(false) { down.push(u.clone()); } }
    if down.is_empty() { Check::ok(NAME, format!("{} answering", s.urls.join(", "))) } else { Check::failed(NAME, format!("not answering: {}", down.join(", ")), "the sandbox pods run (a gVisor or Kata RuntimeClass) and their service is reachable") }
}

/// The checks as a table, for a terminal.
pub fn table(checks: &[Check]) -> String {
    let mut s = String::new();
    for c in checks {
        let mark = match c.status { Status::Ok => "ok  ", Status::Failed => "FAIL", Status::Skipped => "--  " };
        s.push_str(&format!("{mark} {:<20} {}\n", c.name, c.detail));
        if let Some(f) = &c.fix { s.push_str(&format!("     {:<20} fix: {f}\n", "")); }
    }
    let failed = checks.iter().filter(|c| c.is_failed()).count();
    s.push_str(&format!("\n{} check(s), {failed} failed\n", checks.len()));
    s
}

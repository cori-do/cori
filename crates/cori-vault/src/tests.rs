// SPDX-License-Identifier: Apache-2.0
//! The module against Vault as it answers: a stand-in for Kubernetes and userpass auth, token renewal, Transit (upsert on
//! encrypt, versions, associated data, keys a policy no longer lets Cori use, deletion) and the KV engine of version 2
//! (versions, check-and-set, a policy of create and read, deleted versions, metadata deletion).
use super::*;
use crate::transit::version_of;
use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use axum::{Json, Router, extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, routing::{delete, get, post}};

#[derive(Default)]
pub(crate) struct Key { pub(crate) versions: Vec<[u8; 32]>, deletion_allowed: bool }

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) keys: HashMap<String, Key>,
    /// The KV engine's secrets: path → the field `key` of each version (None: deleted or destroyed).
    pub(crate) kv: HashMap<String, Vec<Option<String>>>,
    tokens: HashMap<String, Instant>,
    /// Cori's policy grants `create` (Transit: on the encrypt paths, a missing key is created; KV: a secret may be written
    /// where there is none).
    pub(crate) allow_create: bool,
    /// keys (Transit) and paths (KV) Cori's policy no longer lets it use
    pub(crate) denied: Vec<String>,
    ttl: u64,
    pub(crate) logins: u32,
    renewals: u32,
}

pub(crate) type Shared = Arc<Mutex<Fake>>;
type Reply = (StatusCode, Json<Value>);

fn refuse(status: StatusCode, msg: &str) -> Reply { (status, Json(json!({ "errors": [msg] }))) }

fn authed(f: &Fake, h: &HeaderMap) -> bool {
    h.get("x-vault-token").and_then(|t| t.to_str().ok()).and_then(|t| f.tokens.get(t)).is_some_and(|exp| Instant::now() < *exp)
}

fn new_token(f: &mut Fake) -> Reply {
    f.logins += 1;
    let token = format!("tok-{}", f.logins);
    let ttl = f.ttl;
    f.tokens.insert(token.clone(), Instant::now() + Duration::from_secs(ttl));
    (StatusCode::OK, Json(json!({ "auth": { "client_token": token, "lease_duration": ttl, "renewable": true } })))
}

async fn login(State(f): State<Shared>, Json(b): Json<Value>) -> Reply {
    let mut f = f.lock().unwrap();
    if b["role"] != "cori" || b["jwt"] != "sa-jwt" { return refuse(StatusCode::BAD_REQUEST, "invalid role or service account"); }
    new_token(&mut f)
}

async fn userpass(State(f): State<Shared>, Path(user): Path<String>, Json(b): Json<Value>) -> Reply {
    let mut f = f.lock().unwrap();
    if user != "cori" || b["password"] != "s3cret-pw" { return refuse(StatusCode::BAD_REQUEST, "invalid username or password"); }
    new_token(&mut f)
}

async fn renew(State(f): State<Shared>, h: HeaderMap) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    let token = h["x-vault-token"].to_str().unwrap().to_string();
    let ttl = f.ttl;
    f.renewals += 1;
    f.tokens.insert(token.clone(), Instant::now() + Duration::from_secs(ttl));
    (StatusCode::OK, Json(json!({ "auth": { "client_token": token, "lease_duration": ttl, "renewable": true } })))
}

fn seal(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let nonce: [u8; 12] = rand::random();
    let mut out = nonce.to_vec();
    out.extend(Aes256Gcm::new(key.into()).encrypt(&aes_gcm::Nonce::from(nonce), aes_gcm::aead::Payload { msg: plain, aad }).unwrap());
    out
}

async fn encrypt(State(f): State<Shared>, Path(name): Path<String>, h: HeaderMap, Json(b): Json<Value>) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) || f.denied.contains(&name) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    if !f.keys.contains_key(&name) {
        // the request needs `create` when the key does not exist yet (Transit's upsert)
        if !f.allow_create { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
        f.keys.insert(name.clone(), Key { versions: vec![rand::random()], deletion_allowed: false });
    }
    let k = &f.keys[&name];
    let (v, key) = (k.versions.len(), k.versions[k.versions.len() - 1]);
    let plain = unb64(b["plaintext"].as_str().unwrap()).unwrap();
    let aad = unb64(b["associated_data"].as_str().unwrap_or("")).unwrap();
    (StatusCode::OK, Json(json!({ "data": { "ciphertext": format!("vault:v{v}:{}", b64(&seal(&key, &plain, &aad))), "key_version": v } })))
}

async fn decrypt(State(f): State<Shared>, Path(name): Path<String>, h: HeaderMap, Json(b): Json<Value>) -> Reply {
    let f = f.lock().unwrap();
    if !authed(&f, &h) || f.denied.contains(&name) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    let Some(k) = f.keys.get(&name) else { return refuse(StatusCode::BAD_REQUEST, "encryption key not found") };
    let ct = b["ciphertext"].as_str().unwrap();
    let v = version_of(ct).unwrap() as usize;
    let raw = unb64(ct.splitn(3, ':').nth(2).unwrap()).unwrap();
    let (nonce, body) = raw.split_at(12);
    let aad = unb64(b["associated_data"].as_str().unwrap_or("")).unwrap();
    match Aes256Gcm::new((&k.versions[v - 1]).into()).decrypt(&aes_gcm::Nonce::from(<[u8; 12]>::try_from(nonce).unwrap()), aes_gcm::aead::Payload { msg: body, aad: &aad }) {
        Ok(p) => (StatusCode::OK, Json(json!({ "data": { "plaintext": b64(&p) } }))),
        Err(_) => refuse(StatusCode::BAD_REQUEST, "cipher: message authentication failed"),
    }
}

async fn config(State(f): State<Shared>, Path(name): Path<String>, h: HeaderMap, Json(b): Json<Value>) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    match f.keys.get_mut(&name) { Some(k) => { k.deletion_allowed = b["deletion_allowed"].as_bool().unwrap_or(false); (StatusCode::OK, Json(json!({}))) } None => refuse(StatusCode::NOT_FOUND, "not found") }
}

async fn kv_read(State(f): State<Shared>, Path(path): Path<String>, Query(q): Query<HashMap<String, String>>, h: HeaderMap) -> Reply {
    let f = f.lock().unwrap();
    if !authed(&f, &h) || f.denied.contains(&path) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    let Some(versions) = f.kv.get(&path) else { return (StatusCode::NOT_FOUND, Json(json!({ "errors": [] }))) };
    let v = q.get("version").and_then(|v| v.parse::<usize>().ok()).filter(|v| *v > 0).unwrap_or(versions.len());
    match versions.get(v.wrapping_sub(1)) {
        Some(Some(key)) => (StatusCode::OK, Json(json!({ "data": { "data": { "key": key }, "metadata": { "version": v, "deletion_time": "", "destroyed": false } } }))),
        _ => (StatusCode::NOT_FOUND, Json(json!({ "errors": [] }))),
    }
}

async fn kv_write(State(f): State<Shared>, Path(path): Path<String>, h: HeaderMap, Json(b): Json<Value>) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) || f.denied.contains(&path) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    let current = f.kv.get(&path).map(|v| v.len()).unwrap_or(0);
    // Cori's policy: `create` (when allowed) and `read`, never `update`: a secret that exists is not Cori's to write
    if current > 0 || !f.allow_create { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    if b["options"]["cas"].as_u64().is_some_and(|cas| cas as usize != current) { return refuse(StatusCode::BAD_REQUEST, "check-and-set parameter did not match the current version"); }
    f.kv.entry(path).or_default().push(b["data"]["key"].as_str().map(String::from));
    (StatusCode::OK, Json(json!({ "data": { "version": current + 1 } })))
}

async fn kv_destroy(State(f): State<Shared>, Path(path): Path<String>, h: HeaderMap) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) || f.denied.contains(&path) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    f.kv.remove(&path);
    (StatusCode::NO_CONTENT, Json(json!({})))
}

async fn delete_key(State(f): State<Shared>, Path(name): Path<String>, h: HeaderMap) -> Reply {
    let mut f = f.lock().unwrap();
    if !authed(&f, &h) { return refuse(StatusCode::FORBIDDEN, "permission denied"); }
    match f.keys.get(&name) {
        Some(k) if k.deletion_allowed => { f.keys.remove(&name); (StatusCode::NO_CONTENT, Json(json!({}))) }
        Some(_) => refuse(StatusCode::BAD_REQUEST, "deletion is not allowed for this key"),
        None => refuse(StatusCode::NOT_FOUND, "not found"),
    }
}

/// A Vault stand-in on a free port, and its state.
pub(crate) async fn vault(allow_create: bool, ttl: u64) -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(Fake { allow_create, ttl, ..Default::default() }));
    let app = Router::new()
        .route("/v1/auth/kubernetes/login", post(login))
        .route("/v1/auth/userpass/login/{user}", post(userpass))
        .route("/v1/auth/token/renew-self", post(renew))
        .route("/v1/secret/data/{*path}", get(kv_read).post(kv_write))
        .route("/v1/secret/metadata/{*path}", delete(kv_destroy))
        .route("/v1/transit/encrypt/{name}", post(encrypt))
        .route("/v1/transit/decrypt/{name}", post(decrypt))
        .route("/v1/transit/keys/{name}/config", post(config))
        .route("/v1/transit/keys/{name}", delete(delete_key))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

pub(crate) fn settings(addr: &str, tweak: impl FnOnce(&mut Settings)) -> Settings {
    let jwt = std::env::temp_dir().join(format!("cori-vault-sa-{}", rand::random::<u64>()));
    std::fs::write(&jwt, "sa-jwt\n").unwrap();
    let mut s = Settings {
        addr: addr.into(), namespace: None,
        auth: Auth::Kubernetes { mount: "kubernetes".into(), role: "cori".into(), token_file: jwt.display().to_string() },
        engine: Engine::Transit, mount: "transit".into(), prefix: "cori".into(), kv_mount: "secret".into(), kv_path: "cori".into(),
        create_keys: true, delete_keys: false, deployment_key: None, cache: Duration::from_secs(60),
    };
    tweak(&mut s);
    s
}

pub(crate) fn app(a: &str) -> SecretScope { SecretScope { realm: None, app: Some(a.into()) } }

#[tokio::test]
async fn each_app_has_a_key_of_its_own_and_its_rows_open_under_it_only() {
    let (addr, state) = vault(true, 3600).await;
    let t = Transit::new(settings(&addr, |_| {}));
    let key: [u8; 32] = rand::random();
    let w = t.wrap(&app("desk"), &key, "secret:@desk/crm.token").await.unwrap();
    assert_eq!(w.key_by, "vault-transit:cori-app-desk:1");
    assert!(w.wrapped.starts_with("vault:v1:"));
    assert!(state.lock().unwrap().keys.contains_key("cori-app-desk"), "created on its first use (upsert)");
    assert_eq!(t.unwrap(&app("desk"), &w.key_by, &w.wrapped, "secret:@desk/crm.token").await.unwrap(), key);
    // another app's scope never takes this key, and the associated data binds the key to its secret
    let e = t.unwrap(&app("billing"), &w.key_by, &w.wrapped, "secret:@desk/crm.token").await.unwrap_err().to_string();
    assert!(e.contains("not by `cori-app-billing`"), "{e}");
    let fresh = Transit::new(settings(&addr, |s| s.cache = Duration::ZERO));
    let e = fresh.unwrap(&app("desk"), &w.key_by, &w.wrapped, "secret:@desk/other").await.unwrap_err().to_string();
    assert!(e.contains("authentication failed"), "{e}");
    assert!(!t.covers(&SecretScope { realm: None, app: None }), "the deployment's secrets keep the master key");
    let with_deployment = Transit::new(settings(&addr, |s| s.deployment_key = Some("cori-deployment".into())));
    assert!(with_deployment.covers(&SecretScope { realm: None, app: None }));
    assert_eq!(with_deployment.key_name(&SecretScope { realm: None, app: None }).unwrap(), "cori-deployment");
    assert_eq!(t.key_name(&SecretScope { realm: Some(cori_server::db::Realm::new("org-a")), app: Some("desk".into()) }).unwrap(), "cori-realm-org-a-app-desk");
}

#[tokio::test]
async fn keys_come_with_the_app_or_from_the_organization() {
    let (addr, state) = vault(true, 3600).await;
    let t = Transit::new(settings(&addr, |_| {}));
    assert_eq!(t.prepare(&app("desk")).await.unwrap().as_deref(), Some("cori-app-desk"));
    assert!(state.lock().unwrap().keys.contains_key("cori-app-desk"), "made with the app");
    // without the right to create keys: the key is named, the organization makes it
    let (addr, state) = vault(false, 3600).await;
    let t = Transit::new(settings(&addr, |s| s.create_keys = false));
    assert_eq!(t.prepare(&app("desk")).await.unwrap().as_deref(), Some("cori-app-desk"));
    assert!(state.lock().unwrap().keys.is_empty());
    let e = t.wrap(&app("desk"), &[1; 32], "a").await.unwrap_err().to_string();
    assert!(e.contains("may not exist yet") && e.contains("app.created"), "{e}");
    state.lock().unwrap().keys.insert("cori-app-desk".into(), Key { versions: vec![rand::random()], deletion_allowed: false });
    assert!(t.wrap(&app("desk"), &[1; 32], "a").await.is_ok(), "once the organization made it");
}

#[tokio::test]
async fn a_rotated_key_is_rewrapped_and_a_withdrawn_one_stops_unwrapping() {
    let (addr, state) = vault(true, 3600).await;
    let t = Transit::new(settings(&addr, |s| s.cache = Duration::ZERO));
    let key: [u8; 32] = rand::random();
    let w = t.wrap(&app("desk"), &key, "aad").await.unwrap();
    assert!(t.rewrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap().is_none(), "already the newest version");
    state.lock().unwrap().keys.get_mut("cori-app-desk").unwrap().versions.push(rand::random());
    let r = t.rewrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap().expect("moves to version 2");
    assert_eq!(r.key_by, "vault-transit:cori-app-desk:2");
    assert_eq!(t.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap(), key);
    // the organization withdraws the key from Cori: nothing unwraps any more (no cache here)
    state.lock().unwrap().denied.push("cori-app-desk".into());
    let e = t.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap_err().to_string();
    assert!(e.contains("403"), "{e}");
    // with a cache, what was unwrapped a moment ago still opens until it expires
    state.lock().unwrap().denied.clear();
    let cached = Transit::new(settings(&addr, |s| s.cache = Duration::from_millis(300)));
    cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap();
    state.lock().unwrap().denied.push("cori-app-desk".into());
    assert_eq!(cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap(), key, "within the cache's time");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.is_err(), "and not after");
}

#[tokio::test]
async fn a_token_is_renewed_and_a_refused_one_logs_in_again() {
    let (addr, state) = vault(true, 3).await;
    let t = Transit::new(settings(&addr, |_| {}));
    t.wrap(&app("desk"), &[1; 32], "a").await.unwrap();
    assert_eq!(state.lock().unwrap().logins, 1);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    t.wrap(&app("desk"), &[1; 32], "a").await.unwrap();
    let (logins, renewals) = { let f = state.lock().unwrap(); (f.logins, f.renewals) };
    assert_eq!((logins, renewals), (1, 1), "renewed past two thirds of its life");
    state.lock().unwrap().tokens.clear();
    t.wrap(&app("desk"), &[1; 32], "a").await.unwrap();
    assert_eq!(state.lock().unwrap().logins, 2, "a revoked token: a new login");
}

#[tokio::test]
async fn an_apps_key_is_deleted_with_it_only_when_told() {
    let (addr, state) = vault(true, 3600).await;
    let keep = Transit::new(settings(&addr, |_| {}));
    keep.prepare(&app("desk")).await.unwrap();
    keep.forget(&app("desk")).await.unwrap();
    assert!(state.lock().unwrap().keys.contains_key("cori-app-desk"), "left to the organization by default");
    let delete = Transit::new(settings(&addr, |s| s.delete_keys = true));
    delete.forget(&app("desk")).await.unwrap();
    assert!(!state.lock().unwrap().keys.contains_key("cori-app-desk"));
}

#[tokio::test]
async fn the_preflight_says_what_answered_and_what_to_fix() {
    let (addr, _) = vault(true, 3600).await;
    let checks = Wrapper::new(settings(&addr, |_| {})).preflight().await;
    assert!(checks.iter().all(|c| !c.is_failed()), "{checks:?}");
    assert!(checks.iter().any(|c| c.name == "vault transit" && c.detail.contains("cori-preflight")), "{checks:?}");
    let (addr, _) = vault(false, 3600).await;
    let checks = Wrapper::new(settings(&addr, |_| {})).preflight().await;
    let failed: Vec<_> = checks.iter().filter(|c| c.is_failed()).collect();
    assert_eq!(failed.len(), 1, "{checks:?}");
    assert!(failed[0].fix.as_deref().unwrap().contains("transit/encrypt/cori-*"), "{checks:?}");
    let bad_login = Wrapper::new(settings(&addr, |s| s.auth = Auth::Kubernetes { mount: "kubernetes".into(), role: "other".into(), token_file: "/nonexistent".into() })).preflight().await;
    assert!(bad_login[0].is_failed() && bad_login.len() == 1, "{bad_login:?}");
}

#[test]
fn production_settings_are_https_and_no_static_token() {
    let s = settings("http://vault.internal:8200", |s| s.auth = Auth::Token("t".into()));
    let p = s.problems();
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(settings("https://vault.internal:8200", |_| {}).problems().is_empty());
    assert!(settings("http://127.0.0.1:8200", |_| {}).problems().is_empty(), "a laptop");
}

/// The broker with this key wrapper, end to end on a platform database: an app's secret is wrapped by the app's key, the
/// deployment's by the master key, and deleting the app forgets its key's scope. Needs CORI_STORE_DATABASE_URL.
#[tokio::test]
async fn the_broker_keeps_each_apps_credentials_under_its_vault_key() {
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let name = format!("cori_test_vault_{}", rand::random::<u32>());
    let a = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&admin).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&a).await.unwrap();
    a.close().await;
    let base = admin.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let query = admin.find('?').map(|i| &admin[i..]).unwrap_or("");
    let db = cori_server::db::Db::new(cori_server::db::connect(&format!("{base}/{name}{query}")).await.unwrap());
    let (addr, state) = vault(true, 3600).await;
    let broker = cori_server::secrets::Broker::from_key([9; 32]).with_wrapper(Some(Arc::new(Transit::new(settings(&addr, |_| {})))));
    broker.put_in(&db, Some("desk"), "crm.token", "token", "desk-value", "owner").await.unwrap();
    broker.put(&db, "llm.main", "llm_key", "deployment-value", "admin").await.unwrap();
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, key_by FROM secrets ORDER BY name").fetch_all(&db).await.unwrap();
    assert_eq!(rows, vec![("crm.token".to_string(), "vault-transit:cori-app-desk:1".to_string()), ("llm.main".to_string(), "master".to_string())]);
    assert_eq!(broker.resolve(&db, "desk", "crm.token").await.unwrap().0, "desk-value");
    // the organization rotates the app's key; the leading replica's rewrap moves the row to it
    state.lock().unwrap().keys.get_mut("cori-app-desk").unwrap().versions.push(rand::random());
    let quick = cori_server::secrets::Broker::from_key([9; 32]).with_wrapper(Some(Arc::new(Transit::new(settings(&addr, |s| s.cache = Duration::ZERO)))));
    assert_eq!(quick.rewrap(&db).await.unwrap(), 1);
    assert_eq!(quick.reveal_in(&db, Some("desk"), "crm.token").await.unwrap(), "desk-value");
    assert_eq!(broker.forget_app(&db, "desk").await.unwrap(), 1);
    assert!(state.lock().unwrap().keys.contains_key("cori-app-desk"), "the key stays for the organization");
    db.close().await;
}

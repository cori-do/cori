// SPDX-License-Identifier: Apache-2.0
//! The KV engine against the Vault stand-in (`tests`): each app's master key made once and read back, rows opening under
//! their own app's key only, rotation as new versions, a read withdrawn, keys the organization makes, an account's login,
//! and a switch of engine moving the rows.
use super::*;
use super::tests::{app, settings, vault};

fn kv_settings(addr: &str, tweak: impl FnOnce(&mut Settings)) -> Settings {
    settings(addr, |s| { s.engine = Engine::Kv; tweak(s) })
}

/// What the organization writes as a new version of a key (`vault kv put … key=…`).
fn org_puts(state: &tests::Shared, path: &str, key: &str) { state.lock().unwrap().kv.entry(path.into()).or_default().push(Some(key.into())); }

#[tokio::test]
async fn each_app_has_a_master_key_of_its_own_and_its_rows_open_under_it_only() {
    let (addr, state) = vault(true, 3600).await;
    let k = Kv::new(kv_settings(&addr, |_| {}));
    assert_eq!(k.prepare(&app("desk")).await.unwrap().as_deref(), Some("secret/cori/apps/desk"), "app.created names it as `vault kv` takes it");
    let stored = state.lock().unwrap().kv["cori/apps/desk"].clone();
    assert_eq!(stored.len(), 1);
    assert!(kv::parse_key(stored[0].as_deref().unwrap()).is_ok(), "32 random bytes, base64");
    // made once: another replica (its own login and memory) finds it and never writes another
    let other = Kv::new(kv_settings(&addr, |_| {}));
    other.prepare(&app("desk")).await.unwrap();
    assert_eq!(state.lock().unwrap().kv["cori/apps/desk"].len(), 1);
    let key: [u8; 32] = rand::random();
    let w = k.wrap(&app("desk"), &key, "secret:@desk/crm.token").await.unwrap();
    assert_eq!(w.key_by, "vault-kv:secret/cori/apps/desk:1");
    assert_eq!(other.unwrap(&app("desk"), &w.key_by, &w.wrapped, "secret:@desk/crm.token").await.unwrap(), key, "every replica opens it");
    // another app's scope never takes this key, and the associated data binds the key to its secret
    let e = k.unwrap(&app("billing"), &w.key_by, &w.wrapped, "secret:@desk/crm.token").await.unwrap_err().to_string();
    assert!(e.contains("not by `secret/cori/apps/billing`"), "{e}");
    let e = k.unwrap(&app("desk"), &w.key_by, &w.wrapped, "secret:@desk/other").await.unwrap_err().to_string();
    assert!(e.contains("does not open"), "{e}");
    // its own key per app
    let b = k.wrap(&app("billing"), &key, "a").await.unwrap();
    assert_eq!(b.key_by, "vault-kv:secret/cori/apps/billing:1");
    { let f = state.lock().unwrap(); assert_ne!(f.kv["cori/apps/billing"], f.kv["cori/apps/desk"]); }
    // where the keys are: the realm's, under a tenancy; the deployment's, when named
    assert_eq!(k.key_path(&SecretScope { realm: Some(cori_server::db::Realm::new("org-a")), app: Some("desk".into()) }).unwrap(), "cori/realms/org-a/apps/desk");
    assert!(!k.covers(&SecretScope { realm: None, app: None }), "the deployment's secrets keep the master key");
    let with_deployment = Kv::new(kv_settings(&addr, |s| { s.kv_path = "teams/platform/cori".into(); s.deployment_key = Some("teams/platform/cori/deployment".into()) }));
    assert!(with_deployment.covers(&SecretScope { realm: None, app: None }));
    assert_eq!(with_deployment.prepare(&app("desk")).await.unwrap().as_deref(), Some("secret/teams/platform/cori/apps/desk"));
}

#[tokio::test]
async fn keys_come_with_the_app_or_from_the_organization() {
    let (addr, state) = vault(false, 3600).await;
    let k = Kv::new(kv_settings(&addr, |s| s.create_keys = false));
    assert_eq!(k.prepare(&app("desk")).await.unwrap().as_deref(), Some("secret/cori/apps/desk"));
    assert!(state.lock().unwrap().kv.is_empty(), "named, not made");
    let e = k.wrap(&app("desk"), &[1; 32], "a").await.unwrap_err().to_string();
    assert!(e.contains("does not exist yet") && e.contains("app.created") && e.contains("vault kv put secret/cori/apps/desk"), "{e}");
    // the organization makes it, in hex as `openssl rand -hex 32` prints it
    org_puts(&state, "cori/apps/desk", &"ab".repeat(32));
    let w = k.wrap(&app("desk"), &[1; 32], "a").await.unwrap();
    assert_eq!(k.unwrap(&app("desk"), &w.key_by, &w.wrapped, "a").await.unwrap(), [1; 32]);
    // a value that is no key says so
    org_puts(&state, "cori/apps/crm", "not-a-key");
    let e = k.wrap(&app("crm"), &[1; 32], "a").await.unwrap_err().to_string();
    assert!(e.contains("is not a key of 32 bytes"), "{e}");
    // without the right to create keys, Cori may not make one even when told to
    let make = Kv::new(kv_settings(&addr, |_| {}));
    let e = make.prepare(&app("billing")).await.unwrap_err().to_string();
    assert!(e.contains("403") && e.contains("create and read on secret/data/cori/apps/billing"), "{e}");
}

#[tokio::test]
async fn a_new_version_is_a_rotation_and_a_withdrawn_read_cuts_the_app_off() {
    let (addr, state) = vault(true, 3600).await;
    let k = Kv::new(kv_settings(&addr, |s| s.cache = Duration::ZERO));
    let key: [u8; 32] = rand::random();
    let w = k.wrap(&app("desk"), &key, "aad").await.unwrap();
    assert!(k.rewrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap().is_none(), "already the newest version");
    // the organization rotates: a new version of the secret
    org_puts(&state, "cori/apps/desk", &b64(&rand::random::<[u8; 32]>()));
    let r = k.rewrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap().expect("moves to version 2");
    assert_eq!(r.key_by, "vault-kv:secret/cori/apps/desk:2");
    assert_eq!(k.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap(), key);
    assert_eq!(k.unwrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap(), key, "the old row opens while its version is kept");
    // once every row moved, the organization destroys the old version: what it alone wrapped no longer opens
    state.lock().unwrap().kv.get_mut("cori/apps/desk").unwrap()[0] = None;
    let e = k.unwrap(&app("desk"), &w.key_by, &w.wrapped, "aad").await.unwrap_err().to_string();
    assert!(e.contains("version 1 of `secret/cori/apps/desk` is deleted or destroyed"), "{e}");
    // the organization withdraws the read from Cori: nothing of the app opens any more (no cache here)
    state.lock().unwrap().denied.push("cori/apps/desk".into());
    let e = k.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap_err().to_string();
    assert!(e.contains("403") && e.contains("needs create and read"), "{e}");
    assert!(k.rewrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.is_err());
    // with a cache, the key read a moment ago still opens until it expires
    state.lock().unwrap().denied.clear();
    let cached = Kv::new(kv_settings(&addr, |s| s.cache = Duration::from_millis(300)));
    cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap();
    state.lock().unwrap().denied.push("cori/apps/desk".into());
    assert_eq!(cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.unwrap(), key, "within the cache's time");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(cached.unwrap(&app("desk"), &r.key_by, &r.wrapped, "aad").await.is_err(), "and not after");
}

#[tokio::test]
async fn an_account_of_vaults_logs_in() {
    let (addr, state) = vault(true, 3600).await;
    let account = |password: &str| kv_settings(&addr, |s| s.auth = Auth::Account { mount: "userpass".into(), username: "cori".into(), password: password.into() });
    let k = Kv::new(account("s3cret-pw"));
    let w = k.wrap(&app("desk"), &[7; 32], "a").await.unwrap();
    assert_eq!(k.unwrap(&app("desk"), &w.key_by, &w.wrapped, "a").await.unwrap(), [7; 32]);
    assert_eq!(state.lock().unwrap().logins, 1);
    let checks = Wrapper::new(account("wrong")).preflight().await;
    assert!(checks.len() == 1 && checks[0].is_failed() && checks[0].detail.contains("the account `cori` (userpass auth)"), "{checks:?}");
}

#[tokio::test]
async fn an_apps_key_is_deleted_with_it_only_when_told() {
    let (addr, state) = vault(true, 3600).await;
    let keep = Kv::new(kv_settings(&addr, |_| {}));
    keep.prepare(&app("desk")).await.unwrap();
    keep.forget(&app("desk")).await.unwrap();
    assert!(state.lock().unwrap().kv.contains_key("cori/apps/desk"), "left to the organization by default");
    let delete = Kv::new(kv_settings(&addr, |s| s.delete_keys = true));
    delete.forget(&app("desk")).await.unwrap();
    assert!(!state.lock().unwrap().kv.contains_key("cori/apps/desk"), "every version and its metadata");
}

#[tokio::test]
async fn the_preflight_reads_the_preflight_key() {
    let (addr, state) = vault(true, 3600).await;
    let checks = Wrapper::new(kv_settings(&addr, |_| {})).preflight().await;
    assert!(checks.iter().all(|c| !c.is_failed()) && checks.len() == 2, "{checks:?}");
    assert!(checks.iter().any(|c| c.name == "vault kv" && c.detail.contains("secret/cori/preflight")), "{checks:?}");
    assert!(state.lock().unwrap().kv.contains_key("cori/preflight"));
    let (addr, _) = vault(false, 3600).await;
    let checks = Wrapper::new(kv_settings(&addr, |s| s.create_keys = false)).preflight().await;
    let failed: Vec<_> = checks.iter().filter(|c| c.is_failed()).collect();
    assert_eq!(failed.len(), 1, "{checks:?}");
    assert!(failed[0].detail.contains("does not exist yet") && failed[0].fix.as_deref().unwrap().contains("secret/data/cori/apps/*"), "{checks:?}");
}

/// A switch of engine: rows of the other engine open, and the rewrap moves them to the engine now configured.
#[tokio::test]
async fn switching_engines_moves_the_rows_at_the_rewrap() {
    let (addr, state) = vault(true, 3600).await;
    let key: [u8; 32] = rand::random();
    let transit = Wrapper::new(settings(&addr, |_| {}));
    let t = transit.wrap(&app("desk"), &key, "aad").await.unwrap();
    assert_eq!(t.key_by, "vault-transit:cori-app-desk:1");
    // CORI_VAULT_ENGINE=kv: the Transit row opens, and moves under the app's KV key
    let kv = Wrapper::new(kv_settings(&addr, |_| {}));
    assert_eq!(kv.id(), "vault-kv");
    assert_eq!(kv.unwrap(&app("desk"), &t.key_by, &t.wrapped, "aad").await.unwrap(), key);
    let moved = kv.rewrap(&app("desk"), &t.key_by, &t.wrapped, "aad").await.unwrap().expect("moved to KV");
    assert_eq!(moved.key_by, "vault-kv:secret/cori/apps/desk:1");
    assert_eq!(kv.unwrap(&app("desk"), &moved.key_by, &moved.wrapped, "aad").await.unwrap(), key);
    assert!(kv.rewrap(&app("desk"), &moved.key_by, &moved.wrapped, "aad").await.unwrap().is_none(), "nothing left to move");
    // Transit withdrawn once every row moved: the KV rows do not need it
    state.lock().unwrap().denied.push("cori-app-desk".into());
    assert_eq!(kv.unwrap(&app("desk"), &moved.key_by, &moved.wrapped, "aad").await.unwrap(), key);
    // and back: a KV row moves under Transit again
    state.lock().unwrap().denied.clear();
    let back = transit.rewrap(&app("desk"), &moved.key_by, &moved.wrapped, "aad").await.unwrap().expect("moved to Transit");
    assert!(back.key_by.starts_with("vault-transit:cori-app-desk:"), "{}", back.key_by);
    assert!(kv.unwrap(&app("desk"), "master", "x", "aad").await.unwrap_err().to_string().contains("not a key of Vault's"));
}

#[test]
fn keys_are_read_in_base64_or_hex() {
    let k: [u8; 32] = rand::random();
    assert_eq!(kv::parse_key(&b64(&k)).unwrap(), k);
    assert_eq!(kv::parse_key(&format!(" {}\n", hex::encode(k))).unwrap(), k);
    assert!(kv::parse_key(&b64(&[1; 16])).unwrap_err().to_string().contains("16 bytes"));
    assert!(plain_path("cori") && plain_path("teams/platform/cori") && plain_path("cori.v2"));
    assert!(!plain_path("") && !plain_path("a//b") && !plain_path("../x") && !plain_path("a b") && !plain_path("/a"));
}

/// The broker with the KV engine, end to end on a platform database: an app's secret is wrapped by the app's key, the
/// deployment's by the master key, rows Transit wrapped move to KV, a rotation moves them on, and deleting the app forgets
/// its key's scope. Needs CORI_STORE_DATABASE_URL.
#[tokio::test]
async fn the_broker_keeps_each_apps_credentials_under_its_kv_key() {
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let name = format!("cori_test_vault_kv_{}", rand::random::<u32>());
    let a = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&admin).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&a).await.unwrap();
    a.close().await;
    let base = admin.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let query = admin.find('?').map(|i| &admin[i..]).unwrap_or("");
    let db = cori_server::db::Db::new(cori_server::db::connect(&format!("{base}/{name}{query}")).await.unwrap());
    let (addr, state) = vault(true, 3600).await;
    let broker = |s: Settings| cori_server::secrets::Broker::from_key([9; 32]).with_wrapper(Some(Arc::new(Wrapper::new(s))));
    // written under Transit first, then the engine switched to KV
    broker(settings(&addr, |_| {})).put_in(&db, Some("desk"), "old.token", "token", "before the switch", "owner").await.unwrap();
    let b = broker(kv_settings(&addr, |s| s.cache = Duration::ZERO));
    b.put_in(&db, Some("desk"), "crm.token", "token", "desk-value", "owner").await.unwrap();
    b.put(&db, "llm.main", "llm_key", "deployment-value", "admin").await.unwrap();
    let key_by = |n: &'static str| { let db = db.clone(); async move { sqlx::query_scalar::<_, String>("SELECT key_by FROM secrets WHERE name = $1 AND revoked_at IS NULL").bind(n).fetch_one(&db).await.unwrap() } };
    assert_eq!((key_by("crm.token").await, key_by("old.token").await, key_by("llm.main").await), ("vault-kv:secret/cori/apps/desk:1".into(), "vault-transit:cori-app-desk:1".into(), "master".into()));
    assert_eq!(b.resolve(&db, "desk", "old.token").await.unwrap().0, "before the switch");
    assert_eq!(b.rewrap(&db).await.unwrap(), 1, "the Transit row moves to KV");
    assert_eq!(key_by("old.token").await, "vault-kv:secret/cori/apps/desk:1");
    // the organization rotates the app's key; the rewrap moves both rows to version 2
    org_puts(&state, "cori/apps/desk", &b64(&rand::random::<[u8; 32]>()));
    assert_eq!(b.rewrap(&db).await.unwrap(), 2);
    assert_eq!((key_by("crm.token").await, key_by("old.token").await), ("vault-kv:secret/cori/apps/desk:2".into(), "vault-kv:secret/cori/apps/desk:2".into()));
    assert_eq!(b.reveal_in(&db, Some("desk"), "crm.token").await.unwrap(), "desk-value");
    assert_eq!(b.forget_app(&db, "desk").await.unwrap(), 2);
    assert!(state.lock().unwrap().kv.contains_key("cori/apps/desk"), "the key stays for the organization");
    db.close().await;
}

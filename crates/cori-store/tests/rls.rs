// SPDX-License-Identifier: Apache-2.0
//! Postgres enforces the store's roles and rules (target 28): the bypass corpus of the `query` tool, fail-closed
//! defaults, per-role scope, soft delete, inherited tenancy, only_when triggers and CHECK constraints, approvals applied
//! by the applier role, a later migration's table, and `describe` read from the database's grants. Most checks also run a statement
//! straight under the data role, with Cori's own checks out of the way, to show the database refuses it by itself.
//! Needs CORI_STORE_DATABASE_URL (an admin URL); skipped without it.
use cori_store::policy::{pg_roles, role_prefix, runtime_options};
use cori_store::{Store, StoreDef, drop_database, provision};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row};

const APP: &str = "rls-test";

/// The tests share one Postgres server and its `cori_runtime` login, whose password each admin credential derives
/// from its own URL: they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const STORE: &str = "desk";

const MIGRATION: &str = "
CREATE TABLE items (id TEXT PRIMARY KEY, title TEXT NOT NULL, secret TEXT);
CREATE TABLE notes (id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, body TEXT NOT NULL, author TEXT NOT NULL);
CREATE INDEX ON notes (author);
CREATE TABLE tickets (id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, title TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', owner TEXT NOT NULL, budget INT, deleted_at TIMESTAMPTZ);
CREATE INDEX ON tickets (owner);
CREATE TABLE comments (id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, ticket_id TEXT NOT NULL REFERENCES tickets(id), body TEXT NOT NULL);
CREATE TABLE stray (id TEXT PRIMARY KEY, v TEXT);
CREATE TABLE logs (id BIGSERIAL PRIMARY KEY, body TEXT NOT NULL, author TEXT NOT NULL);
CREATE TABLE links (id BIGSERIAL PRIMARY KEY, owner TEXT NOT NULL, target_id TEXT REFERENCES tickets(id));";
const SEED: &str = "
INSERT INTO items VALUES ('a', 'A', 's3cret');
INSERT INTO notes (id, body, author) VALUES ('n-alice', 'from alice', 'alice'), ('n-bob', 'from bob', 'bob');
INSERT INTO tickets (id, title, status, owner, budget) VALUES ('t-alice', 'Alice', 'open', 'alice', 10), ('t-bob', 'Bob', 'open', 'bob', 20), ('t-arch', 'Old', 'archived', 'alice', 0);
INSERT INTO comments (id, ticket_id, body) VALUES ('c-alice', 't-alice', 'hi alice'), ('c-bob', 't-bob', 'hi bob');
INSERT INTO stray VALUES ('x', 'stray');";
const RULES: &str = "
version: \"1.0.0\"
tables:
  items: { global: true }
  notes: { tenant: author }
  logs: { tenant: author }
  links: { tenant: owner }
  tickets:
    tenant: owner
    soft_delete: { column: deleted_at }
    columns:
      status: { allowed_values: [open, done, archived] }
  comments:
    tenant: { via: ticket_id, references: tickets }
";
const MEMBER: &str = "
name: member
approvals: { group: leads }
tables:
  items: { readable: [id, title] }
  notes:
    readable: [id, body, author]
    creatable: { body: { required: true } }
    updatable: { body: {} }
    deletable: true
  tickets:
    readable: [id, title, status, owner, budget]
    creatable: { title: { required: true }, status: { restrict_to: [open] } }
    updatable:
      title: {}
      status:
        only_when:
          - { old.status: open, new.status: [done] }
          - { old.status: done, new.status: [open] }
      budget: { requires_approval: true }
    deletable: { soft_delete: true }
  comments:
    readable: [id, ticket_id, body]
    creatable: { ticket_id: {}, body: { required: true } }
  stray: { readable: [id, v] }
  logs:
    readable: [id, body, author]
    creatable: { body: { required: true } }
    updatable: { body: { only_when: { new.body: { starts_with: old.body } } } }
  links:
    readable: [id, target_id]
    creatable: { target_id: { foreign_key: { verify_with: [title] } } }
    updatable: { target_id: { foreign_key: { verify_with: [title] } } }
";
const LEAD: &str = "
name: lead
tables:
  notes:
    scope: all
    readable: \"*\"
    creatable: { body: { required: true }, author: {} }
  tickets: { scope: all, readable: \"*\" }
  comments: { scope: all, readable: \"*\" }
  logs:
    scope: all
    readable: \"*\"
    creatable: { body: { required: true } }
";

async fn open(admin: &str, db: &str) -> Store { open_in(admin, db, APP).await }

/// `open` for another app: the store's Postgres roles are per app and store, so a test that must own them (another admin
/// credential) takes an app of its own.
async fn open_in(admin: &str, db: &str, app: &str) -> Store {
    drop_database(admin, db).await.expect("drop a previous run's database");
    let p = provision(admin, db, &[("0001_init.sql".into(), MIGRATION.into())], &[("0001_demo.sql".into(), SEED.into())]).await.expect("provision");
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string())].into(), ..Default::default() };
    Store::open(admin, app, &def).await.expect("open")
}

async fn call(s: &Store, role: &str, who: &str, tool: &str, args: Value) -> Result<Value, String> { s.call(role, who, tool, args, false).await.map_err(|e| e.to_string()) }
async fn query(s: &Store, who: &str, sql: &str) -> Result<Value, String> { call(s, "member", who, "query", json!({ "sql": sql })).await.map(|v| v["rows"].clone()) }

/// Runs `sql` directly on a runtime connection: as `pg_role` with `tenant` set, or without switching role at all.
async fn raw(admin: &str, db: &str, pg_role: Option<&str>, tenant: Option<&str>, sql: &str) -> Result<Vec<Value>, String> {
    let mut c = PgConnection::connect_with(&runtime_options(admin, db).unwrap()).await.map_err(|e| e.to_string())?;
    let mut tx = c.begin().await.map_err(|e| e.to_string())?;
    if let Some(t) = tenant { sqlx::query("SELECT set_config('cori.tenant', $1, true)").bind(t).execute(&mut *tx).await.map_err(|e| e.to_string())?; }
    if let Some(r) = pg_role { sqlx::query("SELECT set_config('role', $1, true)").bind(r).execute(&mut *tx).await.map_err(|e| e.to_string())?; }
    let rows = sqlx::query(AssertSqlSafe(format!("SELECT row_to_json(q)::text FROM ({sql}) q"))).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.rollback().await.ok();
    Ok(rows.iter().map(|r| serde_json::from_str(&r.get::<String, _>(0)).unwrap()).collect())
}

/// Runs a write directly as `pg_role` for `tenant` and rolls it back; Ok(rows affected) or the database's error.
async fn raw_exec(admin: &str, db: &str, pg_role: &str, tenant: &str, sql: &str) -> Result<u64, String> {
    let mut c = PgConnection::connect_with(&runtime_options(admin, db).unwrap()).await.map_err(|e| e.to_string())?;
    let mut tx = c.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("SELECT set_config('cori.tenant', $1, true), set_config('role', $2, true)").bind(tenant).bind(pg_role).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    let n = sqlx::query(AssertSqlSafe(sql)).execute(&mut *tx).await.map_err(|e| e.to_string())?.rows_affected();
    tx.rollback().await.ok();
    Ok(n)
}

/// A write as the admin, committed (what happens to a row between a change parked on it and its decision).
async fn admin_exec(admin: &str, db: &str, sql: &str) {
    let mut c = PgConnection::connect(&cori_store::db_url_for(admin, db)).await.unwrap();
    sqlx::query(AssertSqlSafe(sql)).execute(&mut c).await.unwrap();
}

async fn admin_one(admin: &str, db: &str, sql: &str) -> Value {
    let mut c = PgConnection::connect(&cori_store::db_url_for(admin, db)).await.unwrap();
    let r = sqlx::query(AssertSqlSafe(format!("SELECT row_to_json(q)::text FROM ({sql}) q"))).fetch_one(&mut c).await.unwrap();
    serde_json::from_str(&r.get::<String, _>(0)).unwrap()
}

#[tokio::test]
async fn postgres_enforces_roles_and_rules() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let db = "cori_store_rls_test";
    let s = open(&admin, db).await;
    let member = pg_roles(&role_prefix(APP, STORE), "member");

    // --- the bypass corpus: every one of these leaked or slipped through the parser guard before; each fails now
    for sql in [
        "select t.* from items t",                                        // qualified wildcard: secret is not granted
        "select i.secret from items i",                                   // aliased column
        "select i from items i join notes n on true",                     // whole-row reference in a join
        "select secret from items join notes on true",                    // bare column in a join
        "select  *  from items",                                          // spacing around *
        "with d as (delete from notes returning *) select * from d",      // data-modifying CTE
        "select * into stolen from notes",                                // SELECT INTO
        "select set_config('cori.tenant', 'bob', true)",                  // rewriting its own tenant
        // the same from FROM, before the row policies read it
        "select n.id from lateral set_config('cori.tenant', 'bob', true) s, lateral (select id from notes where s is not null) n",
        "select n.id from set_config('cori.tenant', 'bob', true) s, lateral (select id from notes where s is not null) n",
        "select * from lateral pg_ls_dir('.') f, notes",                  // a function of the server's in FROM
        "select query_to_xml('select * from notes', true, false, '')",    // SQL no one parsed
        "select id from notes for update",                                // locks
        "select relname from pg_class",                                   // catalogs are not the store's tables
        "reset role",
        "set role cori_runtime",
        "select 1; select 2",
    ] {
        let r = query(&s, "alice", sql).await;
        assert!(r.is_err(), "`{sql}` should fail: {r:?}");
    }
    assert_eq!(admin_one(&admin, db, "SELECT count(*) AS n FROM notes").await["n"], 2, "the data-modifying CTE deleted nothing");

    // --- the same shapes straight under the data role, with the guard out of the way: refused by grants
    for sql in ["select t.* from items t", "select i.secret from items i", "select i from items i join notes n on true", "table items"] {
        let r = raw(&admin, db, Some(&member.data), Some("alice"), sql).await;
        assert!(r.as_ref().err().map(|e| e.contains("permission denied")).unwrap_or(false), "`{sql}` under the data role: {r:?}");
    }
    // rows: a WHERE on someone else's tenant finds nothing, schema-qualified or not
    assert_eq!(query(&s, "alice", "select id from notes").await.unwrap(), json!([{ "id": "n-alice" }]));
    assert_eq!(query(&s, "alice", "select id from public.notes where author = 'bob'").await.unwrap(), json!([]));
    assert_eq!(raw(&admin, db, Some(&member.data), Some("alice"), "select id from notes where author = 'bob'").await.unwrap(), Vec::<Value>::new());
    // CTEs and window functions now work, scoped like any other read
    assert_eq!(query(&s, "alice", "with mine as (select id, row_number() over (order by id) as n from notes) select id, n from mine").await.unwrap(), json!([{ "id": "n-alice", "n": 1 }]));
    // so do the allow-list's set-returning functions in FROM (a calendar to join on), still over the caller's rows only
    assert_eq!(query(&s, "alice", "select count(n.id) as n from generate_series(1, 3) g cross join notes n").await.unwrap(), json!([{ "n": 3 }]));

    // --- fail closed: no role switch reads nothing, a missing tenant matches nothing, a table without rules is dark
    // (without USAGE on the schema the runtime login does not even see the table)
    assert!(raw(&admin, db, None, None, "select id from notes").await.is_err());
    assert_eq!(raw(&admin, db, Some(&member.data), None, "select id from notes").await.unwrap(), Vec::<Value>::new());
    assert_eq!(query(&s, "alice", "select id from stray").await.unwrap(), json!([]), "stray is not in rules.yaml");
    let d = s.describe(Some("member")).await;
    let stray = d["tables"].as_array().unwrap().iter().find(|t| t["name"] == "stray").unwrap();
    assert_eq!(stray["scope"], "hidden");
    assert!(d["policy"]["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("stray")), "{}", d["policy"]);

    // --- per-role scope: the lead sees every note and may write one for someone else; the member only its own
    assert_eq!(call(&s, "lead", "lead", "listNotes", json!({})).await.unwrap()["data"].as_array().unwrap().len(), 2);
    assert_eq!(call(&s, "member", "alice", "listNotes", json!({})).await.unwrap()["data"].as_array().unwrap().len(), 1);
    let n = call(&s, "lead", "lead", "createNote", json!({ "body": "for bob", "author": "bob" })).await.unwrap();
    assert_eq!(n["author"], "bob", "{n}");
    let n = call(&s, "member", "alice", "createNote", json!({ "body": "mine", "author": "bob" })).await.unwrap();
    assert_eq!(n["author"], "alice", "a member's create takes the caller's id: {n}");
    // a dry run: the server passes the arguments as the caller sent them, reserved `dryRun` key included; nothing is written
    let before = query(&s, "alice", "select count(*) as n from notes").await.unwrap();
    let dry = s.call("member", "alice", "createNote", json!({ "body": "not written", "dryRun": true }), true).await.map_err(|e| e.to_string()).unwrap();
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(query(&s, "alice", "select count(*) as n from notes").await.unwrap(), before);
    // a SERIAL key: the inserting role draws from the table's sequence
    let l = call(&s, "member", "alice", "createLog", json!({ "body": "first" })).await.unwrap();
    assert_eq!((l["id"].as_i64(), l["author"].as_str()), (Some(1), Some("alice")), "{l}");
    assert_eq!(raw_exec(&admin, db, &member.data, "alice", "INSERT INTO logs (body, author) VALUES ('rolled back', 'alice')").await.unwrap(), 1);
    call(&s, "member", "alice", "createLog", json!({ "body": "second" })).await.unwrap();
    // lists page in key order: row 1, rewritten after the second row on disk, still comes first
    admin_exec(&admin, db, "UPDATE logs SET body = 'first, edited' WHERE id = 1").await;
    let page = call(&s, "member", "alice", "listLogs", json!({ "limit": 1 })).await.unwrap();
    assert_eq!(page["data"][0]["id"], 1, "{page}");
    // an only_when that compares with the current row without an `old.*` key: appending passes, rewriting does not
    let l = call(&s, "member", "alice", "updateLog", json!({ "id": 1, "body": "first, edited, appended" })).await.unwrap();
    assert_eq!(l["body"], "first, edited, appended", "{l}");
    assert!(call(&s, "member", "alice", "updateLog", json!({ "id": 1, "body": "rewritten" })).await.is_err());
    // a lead that may not name the owner (the owner column is not creatable to it) files rows of its own, also straight
    // under its data role
    let l = call(&s, "lead", "lead", "createLog", json!({ "body": "the lead's" })).await.unwrap();
    assert_eq!(l["author"], "lead", "{l}");
    assert!(call(&s, "lead", "lead", "createLog", json!({ "body": "for bob", "author": "bob" })).await.is_err());
    let lead = pg_roles(&role_prefix(APP, STORE), "lead");
    assert_eq!(raw_exec(&admin, db, &lead.data, "lead", "INSERT INTO logs (body, author) VALUES ('x', 'lead')").await.unwrap(), 1);
    let e = raw_exec(&admin, db, &lead.data, "lead", "INSERT INTO logs (body, author) VALUES ('x', 'bob')").await.unwrap_err();
    assert!(e.contains("row-level security"), "{e}");

    // --- a foreign key not named after its table: the verification input follows the key (`target_id` → `target_title`)
    let link = call(&s, "member", "alice", "createLink", json!({ "target_id": "t-alice", "target_title": "Alice" })).await.unwrap();
    assert_eq!(link["target_id"], "t-alice", "{link}");
    let e = call(&s, "member", "alice", "createLink", json!({ "target_id": "t-alice", "target_title": "Bob" })).await.unwrap_err();
    assert!(e.contains("Referenced target not found with title matching"), "{e}");
    // an update checks the key it sets like a create: another tenant's row is not a target
    let e = call(&s, "member", "alice", "updateLink", json!({ "id": link["id"], "target_id": "t-bob" })).await.unwrap_err();
    assert!(e.contains("Referenced target not found"), "{e}");
    let moved = call(&s, "member", "alice", "updateLink", json!({ "id": link["id"], "target_id": "t-arch", "target_title": "Old" })).await.unwrap();
    assert_eq!(moved["target_id"], "t-arch", "{moved}");
    // straight under the data role: writing a row for someone else violates the scope policy
    let e = raw_exec(&admin, db, &member.data, "alice", "INSERT INTO notes (body, author) VALUES ('x', 'bob')").await.unwrap_err();
    assert!(e.contains("row-level security"), "{e}");
    // a text key of someone else's row: not found, not updated
    assert!(call(&s, "member", "alice", "getNote", json!({ "id": "n-bob" })).await.is_err());
    assert!(call(&s, "member", "alice", "updateNote", json!({ "id": "n-bob", "body": "hijack" })).await.is_err());
    assert_eq!(raw_exec(&admin, db, &member.data, "alice", "UPDATE notes SET body = 'hijack' WHERE id = 'n-bob'").await.unwrap(), 0);

    // --- inherited tenancy: comments follow their ticket's owner, in reads and in writes
    assert_eq!(query(&s, "alice", "select id from comments").await.unwrap(), json!([{ "id": "c-alice" }]));
    assert_eq!(call(&s, "member", "bob", "listComments", json!({})).await.unwrap()["data"].as_array().unwrap().len(), 1);
    assert!(call(&s, "member", "bob", "createComment", json!({ "ticket_id": "t-alice", "body": "sneaky" })).await.is_err());
    assert!(call(&s, "member", "bob", "createComment", json!({ "ticket_id": "t-bob", "body": "mine" })).await.is_ok());
    assert_eq!(call(&s, "lead", "lead", "listComments", json!({})).await.unwrap()["data"].as_array().unwrap().len(), 3);

    // --- only_when: the validator refuses, and so does the trigger when the validator is out of the way
    assert!(call(&s, "member", "alice", "updateTicket", json!({ "id": "t-arch", "status": "open" })).await.is_err());
    let e = raw_exec(&admin, db, &member.data, "alice", "UPDATE tickets SET status = 'open' WHERE id = 't-arch'").await.unwrap_err();
    assert!(e.contains("only_when"), "{e}");
    assert_eq!(call(&s, "member", "alice", "updateTicket", json!({ "id": "t-alice", "status": "done" })).await.unwrap()["status"], "done");
    // allowed_values as a CHECK constraint, restrict_to as an insert policy
    let e = raw_exec(&admin, db, &member.data, "alice", "UPDATE tickets SET title = 'x', status = 'banana' WHERE id = 't-alice'").await.unwrap_err();
    assert!(e.contains("cori_values_tickets_status") || e.contains("only_when"), "{e}");
    let e = raw_exec(&admin, db, &member.data, "alice", "INSERT INTO tickets (title, status, owner) VALUES ('t', 'done', 'alice')").await.unwrap_err();
    assert!(e.contains("row-level security"), "{e}");
    assert!(call(&s, "member", "alice", "createTicket", json!({ "title": "New", "status": "done" })).await.is_err());
    let t = call(&s, "member", "alice", "createTicket", json!({ "title": "New" })).await.unwrap();
    assert_eq!((t["owner"].as_str(), t["status"].as_str()), (Some("alice"), Some("open")), "{t}");
    assert!(t.get("deleted_at").is_none(), "create returns only the readable columns: {t}");

    // --- approvals: the gated column parks, the data role cannot write it, the applier applies the decision
    let p = call(&s, "member", "alice", "updateTicket", json!({ "id": "t-alice", "budget": 99 })).await.unwrap();
    assert_eq!(p["pending_approval"], true, "{p}");
    let change: cori_store::ParkedChange = serde_json::from_value(p["change"].clone()).expect("the parked change");
    assert_eq!((change.id.as_str(), change.tenant_id.as_str(), change.target_table.as_deref()), (p["approval_id"].as_str().unwrap(), "alice", Some("tickets")));
    assert_eq!(admin_one(&admin, db, "SELECT budget FROM tickets WHERE id = 't-alice'").await["budget"], 10, "nothing is written while it waits");
    let e = raw_exec(&admin, db, &member.data, "alice", "UPDATE tickets SET budget = 99 WHERE id = 't-alice'").await.unwrap_err();
    assert!(e.contains("permission denied"), "{e}");
    s.apply_parked(&change, || async { Ok(()) }).await.expect("approve");
    assert_eq!(admin_one(&admin, db, "SELECT budget FROM tickets WHERE id = 't-alice'").await["budget"], 99);
    // a change that no longer applies (its row changed since it was parked) is refused before the decision is recorded
    let q = call(&s, "member", "alice", "updateTicket", json!({ "id": t["id"], "budget": 7 })).await.unwrap();
    let stale: cori_store::ParkedChange = serde_json::from_value(q["change"].clone()).unwrap();
    admin_exec(&admin, db, &format!("UPDATE tickets SET title = 'changed meanwhile' WHERE id = '{}'", t["id"].as_str().unwrap())).await;
    let marked = std::cell::Cell::new(false);
    let e = s.apply_parked(&stale, || async { marked.set(true); Ok(()) }).await.unwrap_err();
    assert!(matches!(e.downcast_ref::<cori_store::DecideError>(), Some(cori_store::DecideError::Conflict(_))), "{e}");
    assert!(!e.to_string().contains("changed meanwhile"), "the refusal does not echo the row: {e}");
    assert!(!marked.get(), "a refused change is not marked decided");
    // the decision is recorded before the commit: a change another decision already took is rolled back
    let r = call(&s, "member", "alice", "updateTicket", json!({ "id": t["id"], "budget": 5 })).await.unwrap();
    let taken: cori_store::ParkedChange = serde_json::from_value(r["change"].clone()).unwrap();
    let e = s.apply_parked(&taken, || async { Err(cori_store::DecideError::Decided(taken.id.clone()).into()) }).await.unwrap_err();
    assert!(matches!(e.downcast_ref::<cori_store::DecideError>(), Some(cori_store::DecideError::Decided(_))), "{e}");
    assert_eq!(admin_one(&admin, db, &format!("SELECT budget FROM tickets WHERE id = '{}'", t["id"].as_str().unwrap())).await["budget"], Value::Null, "rolled back");

    // --- soft delete: the row stays, marked; the member no longer sees it; the data role cannot hard-delete it
    assert!(raw_exec(&admin, db, &member.data, "alice", "DELETE FROM tickets WHERE id = 't-alice'").await.unwrap_err().contains("permission denied"));
    call(&s, "member", "alice", "deleteTicket", json!({ "id": "t-alice" })).await.expect("soft delete");
    assert_eq!(admin_one(&admin, db, "SELECT deleted_at IS NOT NULL AS gone FROM tickets WHERE id = 't-alice'").await["gone"], true);
    assert!(call(&s, "member", "alice", "getTicket", json!({ "id": "t-alice" })).await.is_err());
    assert_eq!(query(&s, "alice", "select count(*) as n from tickets").await.unwrap(), json!([{ "n": 2 }]), "t-arch and the new ticket");
    // the data role cannot resurrect or re-mark a soft-deleted row either: it does not see it
    assert_eq!(raw_exec(&admin, db, &member.data, "alice", "UPDATE tickets SET title = 'back' WHERE id = 't-alice'").await.unwrap(), 0);

    // --- describe reads the grants: no drift between the role file and the database
    let d = s.describe(Some("member")).await;
    for t in d["tables"].as_array().unwrap() { assert_eq!(t["drift"], json!([]), "{}", t["name"]); }
    let tickets = d["tables"].as_array().unwrap().iter().find(|t| t["name"] == "tickets").unwrap();
    assert_eq!((tickets["scope"].as_str(), tickets["rls"].as_bool(), tickets["delete_is_soft"].as_bool()), (Some("own"), Some(true), Some(true)));
    let budget = tickets["columns"].as_array().unwrap().iter().find(|c| c["name"] == "budget").unwrap();
    assert_eq!((budget["update"].as_bool(), budget["approval"].as_bool()), (Some(true), Some(true)));

    // --- a later migration (what a promotion applies): its new table is behind RLS and granted to no one until the files name it
    let p = provision(&admin, db, &[("0001_init.sql".into(), MIGRATION.into()), ("0002_later.sql".into(), "CREATE TABLE later (id TEXT PRIMARY KEY, v TEXT); INSERT INTO later VALUES ('l', 'x');".into())], &[]).await.expect("migrate");
    let later = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string())].into(), ..Default::default() };
    cori_store::policy::apply_def(&admin, APP, &later).await.expect("policy of the migrated schema");
    assert_eq!(admin_one(&admin, db, "SELECT relrowsecurity AS rls FROM pg_class WHERE relname = 'later'").await["rls"], true);
    assert!(raw(&admin, db, Some(&member.data), Some("alice"), "select id from later").await.unwrap_err().contains("permission denied"));

    // --- the policy is applied once per definition: reopening changes nothing
    let (_, _, _, compiled) = cori_store::policy::compile_def(APP, &StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: cori_store::introspect(&cori_store::db_url_for(&admin, db)).await.unwrap(), rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string())].into(), ..Default::default() }).unwrap();
    assert!(!cori_store::policy::apply(&admin, db, &compiled).await.unwrap(), "same hash: nothing to apply");

    s.pool.close().await;
    drop_database(&admin, db).await.ok();
}

/// Managed Postgres gives a CREATEDB/CREATEROLE admin, not a superuser: the policy applies all the same, minus the
/// hardening that needs ownership of catalog functions.
#[tokio::test]
async fn a_non_superuser_admin_is_enough() {
    let _serial = SERIAL.lock().await;
    let Ok(su) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let mut c = PgConnection::connect(&su).await.unwrap();
    let is_super: bool = sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = current_user").fetch_one(&mut c).await.unwrap();
    if !is_super { eprintln!("the test admin is not a superuser: skipped"); return; }
    sqlx::raw_sql("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cori_test_admin') THEN CREATE ROLE cori_test_admin LOGIN CREATEDB CREATEROLE PASSWORD 'cori_test_admin'; END IF; END $$").execute(&mut c).await.unwrap();
    // the runtime login exists already (created by the superuser in another test): on a real server one admin
    // credential manages it; here the test admin is given the right to do so
    sqlx::raw_sql("DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cori_runtime') THEN GRANT cori_runtime TO cori_test_admin WITH ADMIN OPTION; END IF; END $$").execute(&mut c).await.unwrap();
    let base = su.split('?').next().unwrap();
    let host = base.rsplit_once('@').map(|(_, h)| h).unwrap_or("127.0.0.1:5432/postgres");
    let admin = format!("postgres://cori_test_admin:cori_test_admin@{host}");
    let db = "cori_store_rls_nosuper";
    let s = open_in(&admin, db, "rls-test-nosuper").await;
    assert_eq!(query(&s, "alice", "select id from notes").await.unwrap(), json!([{ "id": "n-alice" }]));
    assert!(query(&s, "alice", "select t.* from items t").await.is_err());
    assert_eq!(call(&s, "member", "alice", "createNote", json!({ "body": "x" })).await.unwrap()["author"], "alice");
    s.pool.close().await;
    drop_database(&admin, db).await.ok();
}

/// A migration can turn standard_conforming_strings off for its database: a value with a backslash and a quote must
/// still be data, not SQL, in the statements Cori writes (a trailing `\'` would otherwise end the literal early).
#[tokio::test]
async fn backslashes_stay_data_when_the_database_turns_standard_strings_off() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let db = "cori_store_rls_scs";
    drop_database(&admin, db).await.expect("drop a previous run's database");
    let off = "DO $$ BEGIN EXECUTE format('ALTER DATABASE %I SET standard_conforming_strings = off', current_database()); END $$;";
    let p = provision(&admin, db, &[("0001_init.sql".into(), MIGRATION.into()), ("0002_off.sql".into(), off.into())], &[("0001_demo.sql".into(), SEED.into())]).await.expect("provision");
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string())].into(), ..Default::default() };
    let s = Store::open(&admin, "rls-test-scs", &def).await.expect("open");
    let rows = call(&s, "member", "alice", "listNotes", json!({ "body": "\\' OR true --" })).await.unwrap();
    assert_eq!(rows["data"], json!([]), "the filter value was read as SQL: {rows}");
    s.pool.close().await;
    drop_database(&admin, db).await.ok();
}

/// Values are bound and cast to their column's type, and identifiers are quoted: a uuid key, an enum, jsonb, date and
/// char columns, and columns named after keywords (`user` as the tenant column, `order`) work through every tool kind,
/// and a value is never SQL.
#[tokio::test]
async fn values_are_bound_and_identifiers_quoted() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let db = "cori_store_rls_types";
    drop_database(&admin, db).await.expect("drop a previous run's database");
    let migration = "CREATE TYPE mood AS ENUM ('calm', 'busy');
CREATE TABLE tasks (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), \"user\" TEXT NOT NULL, \"order\" INT NOT NULL DEFAULT 0, mood mood NOT NULL DEFAULT 'calm', meta JSONB, due DATE, code CHAR(4));";
    let rules = "version: \"1.0.0\"\ntables:\n  tasks: { tenant: user }\n";
    let role = "name: member\ntables:\n  tasks:\n    readable: [id, user, order, mood, meta, due, code]\n    creatable: { order: {}, mood: {}, meta: {}, due: {}, code: {} }\n    updatable: { order: {}, mood: {}, meta: {} }\n    deletable: true\n";
    let p = provision(&admin, db, &[("0001_init.sql".into(), migration.into())], &[]).await.expect("provision");
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: rules.into(), roles_yaml: [("member".to_string(), role.to_string())].into(), ..Default::default() };
    let s = Store::open(&admin, "rls-test-types", &def).await.expect("open");

    let meta = json!({ "note": "it's \\' OR true --", "tags": ["a"] });
    let row = call(&s, "member", "alice", "createTask", json!({ "order": 2, "mood": "busy", "meta": meta, "due": "2025-10-01", "code": "AB" })).await.unwrap();
    let id = row["id"].as_str().expect("a uuid key").to_string();
    assert_eq!((&row["user"], &row["order"], &row["mood"], &row["meta"], &row["due"], &row["code"]), (&json!("alice"), &json!(2), &json!("busy"), &meta, &json!("2025-10-01"), &json!("AB  ")), "{row}");
    assert_eq!(call(&s, "member", "alice", "getTask", json!({ "id": id })).await.unwrap()["meta"], meta);
    // equality filters on keyword and enum columns; a value is data
    for (filter, n) in [(json!({ "order": 2, "mood": "busy" }), 1), (json!({ "mood": "calm" }), 0), (json!({ "code": "AB" }), 1), (json!({ "user": "alice' OR '1'='1" }), 0)] {
        assert_eq!(call(&s, "member", "alice", "listTasks", filter.clone()).await.unwrap()["count"], json!(n), "{filter}");
    }
    assert!(call(&s, "member", "alice", "getTask", json!({ "id": "not-a-uuid" })).await.is_err());
    // another tenant does not reach the row
    assert!(call(&s, "member", "bob", "getTask", json!({ "id": id })).await.is_err());
    let row = call(&s, "member", "alice", "updateTask", json!({ "id": id, "order": 3, "mood": "calm", "meta": null })).await.unwrap();
    assert_eq!((&row["order"], &row["mood"], &row["meta"]), (&json!(3), &json!("calm"), &Value::Null), "{row}");
    let gone = call(&s, "member", "alice", "deleteTask", json!({ "id": id })).await.unwrap();
    assert_eq!(gone["deleted"]["id"], json!(id), "{gone}");
    s.pool.close().await;
    drop_database(&admin, db).await.ok();
}

/// Builder SQL (migrations, seeds) runs as the database's owner login, not as the server-wide admin: it cannot create
/// logins, change the runtime login, or reach another store's database; and other stores' logins cannot connect to it.
#[tokio::test]
async fn builder_sql_runs_as_the_owner() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let (a, b) = ("cori_store_rls_owner_a", "cori_store_rls_owner_b");
    for db in [a, b] { drop_database(&admin, db).await.expect("drop a previous run's database"); }
    let who = provision(&admin, a, &[("0001.sql".into(), "CREATE TABLE who (u TEXT); INSERT INTO who SELECT current_user;".into())], &[]).await.expect("provision a");
    assert!(who.migrations_applied.contains(&"0001.sql".to_string()));
    let owner = cori_store::policy::owner_role(a);
    assert_eq!(admin_one(&admin, a, "SELECT u FROM who").await["u"], json!(owner));
    for (name, sql) in [
        ("0002_login.sql", "CREATE ROLE cori_evil LOGIN PASSWORD 'x'"),
        ("0002_runtime.sql", "ALTER ROLE cori_runtime PASSWORD 'x'"),
        ("0002_grant.sql", "GRANT pg_read_all_data TO CURRENT_USER"),
        ("0002_program.sql", "COPY (SELECT 1) TO PROGRAM 'true'"),
        ("0002_reset.sql", "RESET ROLE; CREATE ROLE cori_evil2 LOGIN"),
    ] {
        let e = provision(&admin, a, &[(name.into(), sql.into())], &[]).await.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("permission denied") || e.contains("must be") || e.contains("not allowed"), "{name} was not refused: {e:?}");
    }
    provision(&admin, b, &[("0001.sql".into(), "CREATE TABLE secret (v TEXT); INSERT INTO secret VALUES ('b');".into())], &[]).await.expect("provision b");
    // a's owner cannot connect to b
    let other = cori_store::policy::owner_options(&admin, a).unwrap().database(b);
    let e = PgConnection::connect_with(&other).await.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("permission denied") || e.contains("not permitted"), "a's owner reached b: {e:?}");
    for db in [a, b] { drop_database(&admin, db).await.ok(); }
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_roles WHERE rolname = $1").bind(&owner).fetch_one(&mut PgConnection::connect(&admin).await.unwrap()).await.unwrap();
    assert_eq!(left, 0, "the owner login is dropped with its database");
}

/// A caller holding several of the store's roles acts as the first of them the app lists (`[auth] roles`), whatever order
/// their token gives them in; without a declared order, as the first their token lists.
#[tokio::test]
async fn a_caller_with_several_roles_acts_as_the_first_the_app_lists() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let db = "cori_store_rls_role_order";
    drop_database(&admin, db).await.expect("drop a previous run's database");
    let p = provision(&admin, db, &[("0001_init.sql".into(), MIGRATION.into())], &[]).await.expect("provision");
    let auditor = "name: auditor\ntables:\n  notes: { scope: all, readable: [id, body] }\n";
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string()), ("auditor".to_string(), auditor.to_string())].into(), ..Default::default() };
    let both = ["member".to_string(), "lead".to_string()];
    let unordered = Store::open(&admin, APP, &def).await.expect("open");
    assert_eq!(unordered.role_of(&both).as_deref(), Some("member"), "no declared order: the token's first");
    let ordered = Store::open(&admin, APP, &StoreDef { role_order: vec!["viewer".into(), "lead".into(), "member".into()], ..def.clone() }).await.expect("open");
    assert_eq!(ordered.role_of(&both).as_deref(), Some("lead"));
    assert_eq!(ordered.role_of(&["member".to_string()]).as_deref(), Some("member"));
    assert_eq!(ordered.role_of(&["viewer".to_string(), cori_store::INSPECTOR.to_string()]), None, "a role the store does not define, and the inspector, are no store roles");
    // a role that sees every row reads their owners only when it lists the column: the refusal says so
    let e = call(&ordered, "auditor", "alice", "query", json!({ "sql": "select author, count(*) as n from notes group by author" })).await.unwrap_err();
    assert!(e.contains("role `auditor` sees every row (`scope: all`) but reads the owner column `notes.author` only when its `readable` lists it"), "{e}");
    assert_eq!(call(&ordered, "auditor", "alice", "query", json!({ "sql": "select count(*) as n from notes" })).await.unwrap()["rows"], json!([{ "n": 0 }]));
    for s in [unordered, ordered] { s.pool.close().await; }
    drop_database(&admin, db).await.ok();
}

/// A new server opens its first stores together (its replicas, or tests running at once) before its runtime login logs
/// in, so each store's admin sets the login's password; replicas first opening one store together all change its roles
/// and its database's grants. Two sessions changing one role or grant at once fail ("tuple concurrently updated"): the
/// admins take turns, and every store opens. The replicas of one store take turns with its policy too: one applies it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stores_opened_at_once_take_turns_with_the_roles() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    const N: usize = 6;
    // the shared runtime login as on a new server: there, not logging in yet
    let mut c = PgConnection::connect(&admin).await.unwrap();
    sqlx::raw_sql("DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cori_runtime') THEN ALTER ROLE cori_runtime NOLOGIN; END IF; END $$").execute(&mut c).await.unwrap();

    // stores of their own, provisioned and then opened at once: they share the runtime login
    let go = std::sync::Arc::new(tokio::sync::Barrier::new(N));
    let mut stores = tokio::task::JoinSet::new();
    for i in 0..N {
        let (admin, go) = (admin.clone(), go.clone());
        stores.spawn(async move {
            let db = format!("cori_store_rls_race_{i}");
            drop_database(&admin, &db).await.map_err(|e| e.to_string())?;
            let p = provision(&admin, &db, &[("0001_init.sql".into(), MIGRATION.into())], &[("0001_demo.sql".into(), SEED.into())]).await;
            go.wait().await;
            let p = p.map_err(|e| format!("provision {db}: {e}"))?;
            let def = StoreDef { name: STORE.into(), db_name: db.clone(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string())].into(), ..Default::default() };
            let s = Store::open(&admin, &format!("rls-test-race-{i}"), &def).await.map_err(|e| format!("open {db}: {e}"))?;
            let rows = query(&s, "alice", "select id from notes").await;
            s.pool.close().await;
            drop_database(&admin, &db).await.ok();
            rows
        });
    }
    for r in stores.join_all().await { assert_eq!(r, Ok(json!([{ "id": "n-alice" }]))); }

    // one store first opened by several replicas at once, with logins of its own: the same roles, logins and grants
    let db = "cori_store_rls_race";
    drop_database(&admin, db).await.expect("drop a previous run's database");
    let access = cori_store::policy::Access::generate();
    let p = cori_store::provision_with(&admin, db, &cori_store::Claim::default(), &access, &[("0001_init.sql".into(), MIGRATION.into())], &[("0001_demo.sql".into(), SEED.into())]).await.expect("provision");
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string()), ("lead".to_string(), LEAD.to_string())].into(), ..Default::default() };
    let mut replicas = tokio::task::JoinSet::new();
    for _ in 0..N {
        let (admin, def, access) = (admin.clone(), def.clone(), access.clone());
        replicas.spawn(async move {
            let s = Store::open_with(&admin, "rls-test-race", &def, &access).await.map_err(|e| e.to_string())?;
            let rows = query(&s, "alice", "select id from notes").await;
            s.pool.close().await;
            rows
        });
    }
    for r in replicas.join_all().await { assert_eq!(r, Ok(json!([{ "id": "n-alice" }]))); }

    // a policy that changed, applied by every replica at once: one applies it, the others wait and find its hash
    let def = StoreDef { roles_yaml: [("member".to_string(), MEMBER.to_string())].into(), ..def };
    let (_, _, _, policy) = cori_store::policy::compile_def("rls-test-race", &def).unwrap();
    let mut applying = tokio::task::JoinSet::new();
    for _ in 0..N {
        let (admin, policy, access) = (admin.clone(), policy.clone(), access.clone());
        applying.spawn(async move { cori_store::policy::apply_with(&admin, db, &policy, &access).await.map_err(|e| e.to_string()) });
    }
    let applied = applying.join_all().await;
    assert!(applied.iter().all(Result::is_ok), "{applied:?}");
    assert_eq!(applied.iter().filter(|r| **r == Ok(true)).count(), 1, "{applied:?}");
    drop_database(&admin, db).await.ok();
}

/// Another opener in the middle of the policy's statements, holding the database's policy lock: they change the public
/// schema's privileges, as an opener's catalog hardening does, and the second of two such changes at once fails ("tuple
/// concurrently updated"). The opener waits for the lock before its roles and hardening, then opens.
#[tokio::test]
async fn an_opener_waits_for_the_statements_another_is_applying() {
    let _serial = SERIAL.lock().await;
    let Ok(admin) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    let db = "cori_store_rls_wait";
    drop_database(&admin, db).await.expect("drop a previous run's database");
    let p = provision(&admin, db, &[("0001_init.sql".into(), MIGRATION.into())], &[("0001_demo.sql".into(), SEED.into())]).await.expect("provision");
    let def = StoreDef { name: STORE.into(), db_name: db.into(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), MEMBER.to_string())].into(), ..Default::default() };
    // the statements run as the database's owner
    let mut other = PgConnection::connect_with(&cori_store::policy::owner_options(&admin, db).unwrap()).await.unwrap();
    sqlx::raw_sql("BEGIN; SELECT pg_advisory_xact_lock(hashtext('cori_policy')); GRANT USAGE ON SCHEMA public TO PUBLIC").execute(&mut other).await.unwrap();
    let opening = tokio::spawn({
        let (admin, def) = (admin.clone(), def.clone());
        async move {
            let s = Store::open(&admin, "rls-test-wait", &def).await.map_err(|e| e.to_string())?;
            let rows = query(&s, "alice", "select id from notes").await;
            s.pool.close().await;
            rows
        }
    });
    // the other commits once the opener waits on a lock in the database (for the schema's row when it does not wait for
    // the policy lock)
    let mut watch = PgConnection::connect(&admin).await.unwrap();
    for _ in 0..200 {
        let (waiting,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a USING (pid) WHERE NOT l.granted AND a.datname = $1").bind(db).fetch_one(&mut watch).await.unwrap();
        if waiting > 0 { break; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    sqlx::raw_sql("COMMIT").execute(&mut other).await.unwrap();
    other.close().await.ok();
    assert_eq!(opening.await.unwrap(), Ok(json!([{ "id": "n-alice" }])));
    drop_database(&admin, db).await.ok();
}

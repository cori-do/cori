// SPDX-License-Identifier: Apache-2.0
//! Store policy on a real Postgres: rules and role constraints hold on text keys, NULL clears a column, list filters stay
//! on readable columns, and `tenant: <column>` scopes every path to the caller. Needs CORI_STORE_DATABASE_URL (an admin
//! URL); skipped without it.
use cori_store::{Store, StoreDef, drop_database, provision};
use serde_json::{Value, json};

const MIGRATION: &str = "
CREATE TABLE items (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', secret TEXT, priority TEXT, completed_at TIMESTAMPTZ);
CREATE TABLE notes (id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, body TEXT NOT NULL, author TEXT NOT NULL);";
const SEED: &str = "
INSERT INTO items (id, title, status, secret, completed_at) VALUES ('a', 'A', 'open', 's3cret', NULL), ('b', 'B', 'done', NULL, now()), ('z', 'Z', 'archived', NULL, NULL);
INSERT INTO notes (id, body, author) VALUES ('n-alice', 'from alice', 'alice'), ('n-bob', 'from bob', 'bob');";
const RULES: &str = "
version: \"1.0.0\"
tables:
  items:
    global: true
    columns:
      status: { allowed_values: [open, done, archived] }
      priority: { allowed_values: [low, high] }
  notes:
    tenant: author
";
const ROLE: &str = "
name: member
tables:
  items:
    readable: [id, title, status, priority, completed_at]
    creatable:
      id: { required: true }
      title: { required: true }
      status: { restrict_to: [open] }
    updatable:
      title: {}
      priority: {}
      completed_at: {}
      status:
        only_when:
          - { old.status: open, new.status: [done] }
          - { old.status: done, new.status: [open] }
    deletable: true
  notes:
    readable: [id, body, author]
    creatable:
      body: { required: true }
    updatable:
      body: {}
    deletable: true
";

async fn store() -> Option<(String, String, Store)> {
    let admin = std::env::var("CORI_STORE_DATABASE_URL").ok()?;
    let db = "cori_store_policy_test".to_string();
    drop_database(&admin, &db).await.expect("drop a previous run's database");
    let p = provision(&admin, &db, &[("0001_init.sql".into(), MIGRATION.into())], &[("0001_demo.sql".into(), SEED.into())]).await.expect("provision");
    let def = StoreDef { name: "todos".into(), db_name: db.clone(), schema_yaml: p.schema_yaml, rules_yaml: RULES.into(), roles_yaml: [("member".to_string(), ROLE.to_string())].into(), ..Default::default() };
    let s = Store::open(&admin, "policy-test", &def).await.expect("open");
    Some((admin, db, s))
}

async fn call(s: &Store, who: &str, tool: &str, args: Value) -> Result<Value, String> { s.call("member", who, tool, args, false).await.map_err(|e| e.to_string()) }

#[tokio::test]
async fn policy_holds_on_text_keys_filters_and_tenancy() {
    let Some((admin, db, s)) = store().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    {
        // rules.yaml allowed_values and only_when on a text key
        let e = call(&s, "alice", "updateItem", json!({ "id": "a", "priority": "banana" })).await.unwrap_err();
        assert!(e.contains("banana") || e.to_lowercase().contains("allowed"), "{e}");
        assert_eq!(call(&s, "alice", "updateItem", json!({ "id": "a", "priority": "high" })).await.unwrap()["priority"], "high");
        let e = call(&s, "alice", "updateItem", json!({ "id": "a", "status": "banana" })).await.unwrap_err();
        assert!(e.contains("status"), "{e}");
        let e = call(&s, "alice", "updateItem", json!({ "id": "z", "status": "open" })).await.unwrap_err();
        assert!(e.contains("old.status"), "{e}");
        let row = call(&s, "alice", "updateItem", json!({ "id": "a", "status": "done", "completed_at": "2025-09-16T10:00:00Z" })).await.unwrap();
        assert_eq!(row["status"], "done");
        // NULL clears a column
        let row = call(&s, "alice", "updateItem", json!({ "id": "a", "status": "open", "completed_at": null })).await.unwrap();
        assert_eq!(row["completed_at"], Value::Null);
        // a column the role cannot update, and restrict_to on create
        assert!(call(&s, "alice", "updateItem", json!({ "id": "a", "secret": "x" })).await.is_err());
        assert!(call(&s, "alice", "createItem", json!({ "id": "c", "title": "C", "status": "done" })).await.is_err());
        // get: the row, a missing row is an error, unreadable columns are left out
        let row = call(&s, "alice", "getItem", json!({ "id": "a" })).await.unwrap();
        assert!(row.get("secret").is_none() && row["title"] == "A", "{row}");
        assert!(call(&s, "alice", "getItem", json!({ "id": "nope" })).await.is_err());
        // list filters: readable columns only, unknown keys and non-scalars fail
        let page = call(&s, "alice", "listItems", json!({ "status": "done" })).await.unwrap();
        assert_eq!(page["data"].as_array().unwrap().len(), 1, "{page}");
        assert!(call(&s, "alice", "listItems", json!({ "secret": "s3cret" })).await.is_err());
        assert!(call(&s, "alice", "listItems", json!({ "where": { "status": "open" } })).await.is_err());
        // tenant: author scopes list, get, update, delete and query to the caller; create sets the column
        let page = call(&s, "alice", "listNotes", json!({})).await.unwrap();
        assert_eq!(page["data"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["n-alice"]);
        assert!(call(&s, "alice", "getNote", json!({ "id": "n-bob" })).await.is_err());
        assert!(call(&s, "alice", "updateNote", json!({ "id": "n-bob", "body": "hijack" })).await.is_err());
        assert!(call(&s, "alice", "updateNote", json!({ "id": "n-alice", "author": "bob" })).await.is_err());
        let del = call(&s, "alice", "deleteNote", json!({ "id": "n-bob" })).await;
        assert!(del.is_err() || del.as_ref().unwrap()["deleted"] == 0, "{del:?}");
        let created = call(&s, "alice", "createNote", json!({ "body": "new", "author": "bob" })).await.unwrap();
        assert_eq!(created["author"], "alice", "{created}");
        let rows = call(&s, "bob", "query", json!({ "sql": "select n.id from notes n join items i on true where i.id = $1", "params": ["a"] })).await.unwrap();
        assert_eq!(rows["rows"], json!([{ "id": "n-bob" }]));
        // a schema-qualified or ONLY reference is still the caller's rows: the database scopes the table itself
        assert_eq!(call(&s, "bob", "query", json!({ "sql": "select id from public.notes" })).await.unwrap()["rows"], json!([{ "id": "n-bob" }]));
        // (the SQL parser reads `only notes` as a table named `only`: refused, or the caller's rows)
        let only = call(&s, "bob", "query", json!({ "sql": "select id from only notes" })).await;
        assert!(only.as_ref().map(|r| r["rows"] == json!([{ "id": "n-bob" }])).unwrap_or(true), "{only:?}");
    }
    s.pool.close().await;
    drop_database(&admin, &db).await.ok();   // a failed run leaves it for inspection; the next run drops it first
}

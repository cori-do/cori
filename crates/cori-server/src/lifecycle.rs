// SPDX-License-Identifier: Apache-2.0
//! A realm's data as a whole: its export (`export`, `GET /v1/export`: what an organization takes with it) and its removal
//! (`purge`, which a module deleting an organization calls). Both act on one realm's state (`AppState::in_realm`, or
//! `AppState::in_realm_unadmitted` for a realm no longer admitted): every query sees that realm's rows only.
//!
//! An export is a `.tar.zst`:
//! - `manifest.json`: when, which realm, and what is inside;
//! - `tables/<table>.jsonl`: every row of every platform table (a module's included), one JSON object a line, without what
//!   only the platform can use (`WITHHELD`: sealed secrets and tokens, a pool's key hash);
//! - `bundles/<hash>.tar.zst`: each version's archive, as it was deployed;
//! - `files/<id>`: each file runs were given, as it was uploaded (its record in `tables/files.jsonl`);
//! - `stores/<app>/<store>/<target>/<schema>.<table>.jsonl`: every row of every store database, read through the store
//!   admin.
//!
//! Temporal keeps the histories of runs until its namespace's retention ends them: an export does not hold them, and a
//! purge does not delete them.
use crate::api::{ApiError, Caller, InRealm, Power, require_human, require_power};
use crate::state::AppState;
use anyhow::{Context, Result};
use axum::response::{IntoResponse, Response};
use futures::TryStreamExt;
use serde_json::{Value, json};
use sqlx::Row;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Columns an export leaves out: what only the platform can open or check (sealed secrets and run tokens, a pool's key
/// hash), useless to anyone else and not the organization's to carry around.
const WITHHELD: &[(&str, &str)] = &[("secrets", "dek_nonce"), ("secrets", "dek_wrapped"), ("secrets", "nonce"), ("secrets", "ciphertext"), ("runs", "principal_token"), ("pools", "key_hash"), ("connections", "refresh_sealed")];

/// The store tables Cori keeps for itself in each store database.
const STORE_OWN: &[&str] = &["cori_migrations", "cori_seeds", "cori_policy"];

fn quote(ident: &str) -> String { format!("\"{}\"", ident.replace('"', "\"\"")) }

/// The realm's platform tables: every table of the schema but the migrations' record. Under a tenancy, only the tables
/// whose rows the database itself keeps to the connection's realm (row-level security, enabled and forced): a module's
/// own tables outside realms (its organizations' record) are never a realm's, whatever their columns.
async fn tables(st: &AppState) -> Result<Vec<String>> {
    let sql = if st.tenancy.is_some() {
        anyhow::ensure!(st.realm().is_some(), "a realm's tables are read and deleted in that realm only");
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') AND c.relrowsecurity AND c.relforcerowsecurity ORDER BY 1"
    } else {
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' AND table_name <> 'platform_migrations' ORDER BY 1"
    };
    Ok(sqlx::query_scalar(sql).fetch_all(&st.db).await?)
}

/// Writes each text of `rows` as a line of `path`; returns how many.
async fn dump<S>(mut rows: S, path: &Path) -> Result<u64> where S: futures::Stream<Item = Result<String, sqlx::Error>> + Unpin {
    use tokio::io::AsyncWriteExt;
    let mut f = tokio::io::BufWriter::new(tokio::fs::File::create(path).await?);
    let mut n = 0;
    while let Some(line) = rows.try_next().await? { f.write_all(line.as_bytes()).await?; f.write_all(b"\n").await?; n += 1; }
    f.flush().await?;
    Ok(n)
}

/// Exports the realm into a `.tar.zst` under the data dir (`exports/`), and returns its path: the caller streams it, then
/// it is deleted (`file_stream`).
pub async fn export(st: &AppState) -> Result<PathBuf> {
    let dir = st.cfg.data_dir.join("exports");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let work = dir.join(format!(".{id}"));
    tokio::fs::create_dir_all(&work).await?;
    let r = export_into(st, &work, &dir.join(format!("{id}.tar.zst"))).await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    r
}

async fn export_into(st: &AppState, work: &Path, out: &Path) -> Result<PathBuf> {
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    let mut counts = serde_json::Map::new();
    for t in tables(st).await? {
        let minus: String = std::iter::once("realm").chain(WITHHELD.iter().filter(|(tb, _)| *tb == t).map(|(_, c)| *c)).map(|c| format!(" - '{c}'")).collect();
        let sql = format!("SELECT (to_jsonb(t){minus})::text FROM {} t", quote(&t));
        let path = work.join(format!("{t}.jsonl"));
        let n = dump(sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql)).fetch(&st.db), &path).await.with_context(|| format!("table {t}"))?;
        counts.insert(t.clone(), json!(n));
        parts.push((format!("tables/{t}.jsonl"), path));
    }
    let hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM versions ORDER BY created_at").fetch_all(&st.db).await?;
    let mut bundles = Vec::new();
    for h in &hashes {
        let name = h.replace(':', "_");
        match st.bundles.archive(st.db.realm(), h).await {
            Ok(bytes) => { let p = work.join(format!("{name}.tar.zst")); tokio::fs::write(&p, bytes).await?; parts.push((format!("bundles/{name}.tar.zst"), p)); bundles.push(h.clone()); }
            Err(e) => tracing::warn!(hash = %h, error = %e, "export: a version's archive is missing"),
        }
    }
    // the files runs were given, as uploaded (`files/<id>`; their records are in tables/files.jsonl)
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM files WHERE status = 'ready' ORDER BY created_at").fetch_all(&st.db).await?;
    let mut files = Vec::new();
    for id in &ids {
        match st.files.get(st.db.realm(), id).await {
            Ok(Some(bytes)) => { let p = work.join(format!("file-{id}")); tokio::fs::write(&p, bytes).await?; parts.push((format!("files/{id}"), p)); files.push(id.clone()); }
            Ok(None) => tracing::warn!(file = %id, "export: a file's bytes are missing"),
            Err(e) => tracing::warn!(file = %id, error = %e, "export: a file's bytes could not be read"),
        }
    }
    let mut stores = Vec::new();
    let clusters = st.cfg.store_clusters();
    if !clusters.is_empty() {
        let rows = sqlx::query("SELECT app, name, target, db_name, cluster FROM store_instances ORDER BY app, name, target").fetch_all(&st.db).await?;
        for r in rows {
            let (app, name, target, db_name, cluster): (String, String, String, String, String) = (r.get("app"), r.get("name"), r.get("target"), r.get("db_name"), r.get("cluster"));
            let Some(admin) = clusters.get(&cluster) else { tracing::warn!(db = %db_name, cluster = %cluster, "export: a store database's cluster is not configured"); continue };
            use sqlx::Connection;
            let mut c = match sqlx::PgConnection::connect(&cori_store::db_url_for(admin, &db_name)).await {
                Ok(c) => c,
                Err(e) => { tracing::warn!(db = %db_name, error = %e, "export: a store database is unreachable"); continue }
            };
            let own: Vec<String> = STORE_OWN.iter().map(|s| s.to_string()).collect();
            let tabs: Vec<(String, String)> = sqlx::query("SELECT schemaname::text AS s, tablename::text AS t FROM pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema') AND NOT (tablename = ANY($1)) ORDER BY 1, 2")
                .bind(&own).fetch_all(&mut c).await?.iter().map(|x| (x.get("s"), x.get("t"))).collect();
            let mut n_rows = serde_json::Map::new();
            for (schema, table) in tabs {
                let path = work.join(format!("{}.{}.{}.jsonl", db_name, schema, table));
                let sql = format!("SELECT row_to_json(t)::text FROM {}.{} t", quote(&schema), quote(&table));
                let n = dump(sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql)).fetch(&mut c), &path).await.with_context(|| format!("store {db_name}: {schema}.{table}"))?;
                n_rows.insert(format!("{schema}.{table}"), json!(n));
                parts.push((format!("stores/{app}/{name}/{}/{schema}.{table}.jsonl", target.replace(':', "_")), path));
            }
            let _ = c.close().await;
            stores.push(json!({ "app": app, "store": name, "target": target, "rows": n_rows }));
        }
    }
    let manifest = json!({ "format": "cori-export/1", "realm": st.realm().map(|r| r.as_str()), "exported_at": chrono::Utc::now().to_rfc3339(), "tables": counts, "bundles": bundles, "files": files, "stores": stores,
        "withheld": WITHHELD.iter().map(|(t, c)| format!("{t}.{c}")).collect::<Vec<_>>(), "not_included": "the histories of runs Temporal keeps until its retention ends them" });
    let out = out.to_path_buf();
    let o = out.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let f = std::fs::File::create(&o)?;
        let mut enc = zstd::Encoder::new(f, 3)?;
        {
            let mut tar = tar::Builder::new(&mut enc);
            let m = serde_json::to_vec_pretty(&manifest)?;
            let mut h = tar::Header::new_gnu(); h.set_size(m.len() as u64); h.set_mode(0o644); h.set_mtime(chrono::Utc::now().timestamp() as u64);
            tar.append_data(&mut h, "manifest.json", m.as_slice())?;
            for (name, path) in &parts { tar.append_path_with_name(path, name)?; }
            tar.finish()?;
        }
        enc.finish()?;
        Ok(())
    }).await??;
    Ok(out)
}

/// A file as a response body, deleted once sent (or once the client went away).
pub fn file_stream(path: PathBuf) -> impl futures::Stream<Item = std::io::Result<axum::body::Bytes>> + Send + 'static {
    struct Gone(PathBuf);
    impl Drop for Gone { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
    async_stream::stream! {
        let _gone = Gone(path.clone());
        let mut f = match tokio::fs::File::open(&path).await { Ok(f) => f, Err(e) => { yield Err(e); return; } };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut f, &mut buf).await {
                Ok(0) => break,
                Ok(n) => yield Ok(axum::body::Bytes::copy_from_slice(&buf[..n])),
                Err(e) => { yield Err(e); break; }
            }
        }
    }
}

/// An export as a response: `cori-export-<date>.tar.zst`.
pub fn export_response(path: PathBuf) -> Response {
    let name = format!("cori-export-{}.tar.zst", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    ([(axum::http::header::CONTENT_TYPE, "application/zstd".to_string()), (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], axum::body::Body::from_stream(file_stream(path))).into_response()
}

/// `GET /v1/export`: the realm's data (`export`), for its admins, in person.
pub(crate) async fn export_route(InRealm(st): InRealm, Caller(who): Caller) -> Result<Response, ApiError> {
    require_human(&who, "an export")?;
    require_power(&st, &who, &[Power::Admin], "an export")?;
    let path = export(&st).await.map_err(|e| ApiError(axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("export: {e:#}")))?;
    crate::db::audit(&st.db, &who.sub, "realm.export", "", json!({})).await?;
    Ok(export_response(path))
}

/// Removes everything of the realm: runs still going are terminated, its Temporal schedules deleted, its store databases
/// dropped, its bundles and files removed (the bundle and file stores' included), and every row of its tables deleted. What a module keeps
/// outside realms (an organization's record) is the module's to remove. Each step is idempotent: a purge cut short is
/// run again. Returns what was removed.
pub async fn purge(st: &Arc<AppState>) -> Result<Value> {
    let running: Vec<String> = sqlx::query_scalar("SELECT id FROM runs WHERE status = 'running'").fetch_all(&st.db).await?;
    crate::api::terminate_runs(st, &running, "the organization is deleted").await;
    let schedules: Vec<String> = sqlx::query_scalar("SELECT id FROM schedules").fetch_all(&st.db).await?;
    for id in &schedules {
        let _ = st.temporal.get_schedule_handle(id.as_str()).delete(temporalio_client::schedules::DeleteScheduleOptions::builder().build()).await;
    }
    let mut dropped = Vec::new();
    if !st.cfg.store_clusters().is_empty() {
        let targets: Vec<(String, String)> = sqlx::query("SELECT DISTINCT app, target FROM store_instances").fetch_all(&st.db).await?.iter().map(|r| (r.get("app"), r.get("target"))).collect();
        for (app, target) in targets {
            let r = if target == cori_store::LIVE { crate::stores::drop_live(st, &app).await } else { crate::stores::drop_sandbox(st, crate::stores::Target::sandbox(&app, &target)).await };
            dropped.extend(r.with_context(|| format!("the stores of `{app}` ({target})"))?);
        }
    }
    let hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM versions").fetch_all(&st.db).await?;
    for h in &hashes { crate::api::remove_version_files(st, h).await; }
    // the files runs were given: their bytes (the file store's bucket included), their records with the tables below
    let files = crate::api::remove_all_file_bytes(st).await.context("the files' bytes")?;
    // and whatever else of the realm this replica keeps (its folder of the data dir): another replica's goes with its pod
    if let Some(r) = st.db.realm() { let _ = std::fs::remove_dir_all(st.cfg.data_dir.join(crate::names::realm_dir(r))); }
    let mut rows = serde_json::Map::new();
    for t in tables(st).await? {
        let n = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {}", quote(&t)))).execute(&st.db).await.with_context(|| format!("table {t}"))?.rows_affected();
        if n > 0 { rows.insert(t, json!(n)); }
    }
    Ok(json!({ "runs_terminated": running.len(), "schedules": schedules.len(), "store_databases": dropped, "bundles": hashes.len(), "files": files, "rows": rows }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(super::quote("runs"), "\"runs\"");
        assert_eq!(super::quote("a\"b"), "\"a\"\"b\"");
    }
}

// SPDX-License-Identifier: Apache-2.0
//! Bundles = the app folder (cori.toml, workflows/, stores/, fixtures/, dist/) uploaded as a `tar.zst`, content-addressed
//! by the sorted manifest `(path, sha256(bytes))` so the hash does not depend on the transport. The server keeps
//! the archive (`bundles/<hash>.tar.zst`, what `cori pull` gets back) and the extracted tree (`bundles/<hash>/`, what the
//! compiler, the front and the workers read), in the data dir and, with a bundle store, in S3 (`BundleStore`); a realm's
//! under its own folder (`realms/<realm>/bundles/`), never shared with another realm deploying the same bundle. Compiled
//! server-side; never trusted from the client.
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use crate::db::Realm;

fn root(data_dir: &Path, realm: Option<&Realm>) -> PathBuf { data_dir.join(crate::names::realm_folder(realm, "bundles")) }
pub fn bundle_dir(data_dir: &Path, realm: Option<&Realm>, hash: &str) -> PathBuf { root(data_dir, realm).join(hash.replace(':', "_")) }
pub fn archive_path(data_dir: &Path, realm: Option<&Realm>, hash: &str) -> PathBuf { root(data_dir, realm).join(format!("{}.tar.zst", hash.replace(':', "_"))) }

/// The generated and tooling folders a bundle leaves out.
pub fn skip_dir(name: &str) -> bool { matches!(name, ".cori" | ".git" | "node_modules" | "target") }

/// A bundle's files: relative path → bytes, sorted.
pub type Files = BTreeMap<String, Vec<u8>>;

/// A credential file (`.env`, `.env.*`, `.npmrc`, `.netrc`, a key or certificate bundle): never part of a bundle, which
/// is kept for good, pulled by every builder and agent session of the app, and read by its workers.
pub fn credential_file(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name == ".env" || name.starts_with(".env.") || matches!(name, ".npmrc" | ".netrc")
        || [".pem", ".key", ".p12", ".pfx"].iter().any(|ext| name.ends_with(ext))
}

/// Every file of a tree (relative path → bytes) a bundle carries, minus the generated and tooling folders and `.DS_Store`,
/// and the credential files it leaves out (`cori deploy` names them). The CLI and the server both bundle with this.
pub fn collect(dir: &Path) -> Result<(Files, Vec<String>)> {
    let (mut files, mut credentials) = (BTreeMap::new(), Vec::new());
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| !(e.file_type().is_dir() && e.file_name().to_str().map(skip_dir).unwrap_or(false))).filter_map(|e| e.ok()) {
        if !e.file_type().is_file() { continue; }
        let rel = e.path().strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        if rel.ends_with(".DS_Store") { continue; }
        if credential_file(&rel) { credentials.push(rel); continue; }
        files.insert(rel, std::fs::read(e.path())?);
    }
    credentials.sort();
    Ok((files, credentials))
}

/// Every file of a tree a bundle carries (`collect` without the list of credential files left out).
pub fn tree_files(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> { Ok(collect(dir)?.0) }

/// `sha256:<hex>` over the sorted manifest; identical to the CLI's computation.
pub fn manifest_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut h = Sha256::new();
    for (p, b) in files { if p.starts_with(".cori/") || p.contains("/.cori/") { continue; } h.update(p.as_bytes()); h.update([0]); h.update(Sha256::digest(b)); h.update([0]); }
    format!("sha256:{}", hex::encode(h.finalize()))
}

/// Extracts a `tar.zst` into `dir`, refusing paths that escape it. Returns the files written.
pub fn extract(bytes: &[u8], dir: &Path) -> Result<usize> {
    let dec = zstd::Decoder::new(bytes)?;
    let mut ar = tar::Archive::new(dec);
    let mut n = 0;
    for entry in ar.entries()? {
        let mut e = entry?;
        let p = e.path()?.to_path_buf();
        if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) { bail!("bundle has an unsafe path: {}", p.display()); }
        if !e.header().entry_type().is_file() { continue; }
        let out = dir.join(&p);
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        let mut buf = Vec::new(); e.read_to_end(&mut buf)?; std::fs::write(&out, buf)?; n += 1;
    }
    Ok(n)
}

/// Stores an uploaded archive: extracted under a temporary folder, hashed, moved to `bundles/<hash>/` (the realm's), the
/// archive kept next to it. Returns the bundle dir and the hash.
pub fn store_archive(data_dir: &Path, realm: Option<&Realm>, bytes: &[u8]) -> Result<(PathBuf, String)> {
    let root = root(data_dir, realm); std::fs::create_dir_all(&root)?;
    let tmp = root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let res = (|| -> Result<(PathBuf, String)> {
        let n = extract(bytes, &tmp)?;
        if n == 0 { bail!("empty bundle"); }
        if !tmp.join("cori.toml").exists() { bail!("bundle has no cori.toml at its root"); }
        let (files, credentials) = collect(&tmp)?;
        if !credentials.is_empty() { bail!("the bundle carries credential files ({}): a bundle is kept and served to the app's builders; delete them and deploy again (the cli leaves them out)", credentials.join(", ")); }
        let hash = manifest_hash(&files);
        let dir = bundle_dir(data_dir, realm, &hash);
        if dir.exists() { std::fs::remove_dir_all(&tmp)?; } else { std::fs::rename(&tmp, &dir)?; }
        std::fs::write(archive_path(data_dir, realm, &hash), bytes)?;
        Ok((dir, hash))
    })();
    if res.is_err() { let _ = std::fs::remove_dir_all(&tmp); }
    res
}

/// Where version bundles live. With `CORI_BUNDLE_STORE=s3://<bucket>[/<prefix>]` (any S3-compatible server:
/// `CORI_S3_ENDPOINT`, else AWS in `CORI_S3_REGION`; keys `CORI_S3_ACCESS_KEY_ID`, `CORI_S3_SECRET_ACCESS_KEY`), a
/// version's archive is written to the bucket before the version is recorded, and the data dir only caches extracted
/// trees: a replica that lacks one fetches the archive and checks its content hash first. Without it, the data dir holds
/// everything, which suits one replica.
pub struct BundleStore { data_dir: PathBuf, s3: Option<S3> }

/// A bucket a store keeps its objects in, and what that store is (`bundle store`, `file store`: its errors say so).
struct S3 { bucket: rusty_s3::Bucket, name: String, credentials: rusty_s3::Credentials, prefix: String, http: reqwest::Client, url: String, sse: Option<(String, Option<String>)>, what: &'static str }

/// An environment variable as the stores read it: trimmed, none when empty.
fn env_var(k: &str) -> Option<String> { std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) }

/// Where a bundle store is: `endpoint` for a server other than AWS's (path-style addressing unless `path_style` says
/// otherwise), the bucket, a key prefix inside it, the access key (with a session token for temporary credentials), and
/// the server-side encryption uploads ask for when the bucket's policy requires it (`AES256`, `aws:kms` with a key).
#[derive(Default)]
pub struct S3Settings { pub endpoint: Option<String>, pub region: String, pub bucket: String, pub prefix: String, pub key: String, pub secret: String,
    pub path_style: Option<bool>, pub session_token: Option<String>, pub sse: Option<String>, pub sse_kms_key_id: Option<String> }

impl S3Settings {
    /// The bucket and prefix `var` names (`s3://<bucket>[/<prefix>]`: `CORI_BUNDLE_STORE`, `CORI_FILE_STORE`), on the S3
    /// server and with the credentials and the encryption of the `CORI_S3_*` settings; None when `var` is not set.
    pub fn named(var: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let Some(spec) = env(var) else { return Ok(None) };
        let rest = spec.strip_prefix("s3://").ok_or_else(|| anyhow!("{var}: expected s3://<bucket>[/<prefix>], got `{spec}`"))?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() { bail!("{var}: expected s3://<bucket>[/<prefix>], got `{spec}`"); }
        let (Some(key), Some(secret)) = (env("CORI_S3_ACCESS_KEY_ID"), env("CORI_S3_SECRET_ACCESS_KEY")) else { bail!("{var}: set CORI_S3_ACCESS_KEY_ID and CORI_S3_SECRET_ACCESS_KEY") };
        let path_style = match env("CORI_S3_PATH_STYLE").map(|v| v.to_ascii_lowercase()) {
            None => None,
            Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => Some(true),
            Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => Some(false),
            Some(v) => bail!("CORI_S3_PATH_STYLE: true or false, not `{v}`"),
        };
        let sse = env("CORI_S3_SSE");
        if let Some(v) = &sse && !matches!(v.as_str(), "AES256" | "aws:kms" | "aws:kms:dsse") { bail!("CORI_S3_SSE: AES256, aws:kms or aws:kms:dsse, not `{v}`"); }
        Ok(Some(S3Settings { endpoint: env("CORI_S3_ENDPOINT"), region: env("CORI_S3_REGION").unwrap_or_else(|| "us-east-1".into()), bucket: bucket.to_string(), prefix: prefix.to_string(), key, secret,
            path_style, session_token: env("CORI_S3_SESSION_TOKEN"), sse, sse_kms_key_id: env("CORI_S3_SSE_KMS_KEY_ID") }))
    }
}

/// How long a signed S3 request stays valid: one call.
const SIGNED_FOR: std::time::Duration = std::time::Duration::from_secs(300);

impl BundleStore {
    pub fn local(data_dir: PathBuf) -> Self { Self { data_dir, s3: None } }

    /// The store `CORI_BUNDLE_STORE` and the `CORI_S3_*` settings name, else the data dir alone.
    pub fn from_env(data_dir: PathBuf) -> Result<Self> {
        match S3Settings::named("CORI_BUNDLE_STORE", &env_var)? { Some(s) => Self::with_s3(data_dir, s), None => Ok(Self::local(data_dir)) }
    }

    /// Archives in the S3 bucket `settings` names, trees cached in `data_dir`.
    pub fn with_s3(data_dir: PathBuf, settings: S3Settings) -> Result<Self> {
        Ok(Self { data_dir, s3: Some(S3::new(settings, "CORI_BUNDLE_STORE", "bundle store")?) })
    }

    /// `s3://<bucket>/<prefix>` when the archives live in S3 (`cori doctor`).
    pub fn remote(&self) -> Option<&str> { self.s3.as_ref().map(|s| s.url.as_str()) }

    /// `cori preflight`: an object written, read back and deleted under the prefix (`preflight/<random>`), with the server's
    /// own credentials. None without a bucket.
    pub async fn check(&self) -> Option<Result<String>> { Some(self.s3.as_ref()?.round_trip().await) }

    pub fn data_dir(&self) -> &Path { &self.data_dir }

    /// Where a version's tree is (or will be) on this replica, not fetched: a root to read the app config against.
    pub fn path(&self, realm: Option<&Realm>, hash: &str) -> PathBuf { bundle_dir(&self.data_dir, realm, hash) }

    /// A stored version's extracted tree. With S3, a replica that lacks it fetches the archive and checks that it hashes
    /// to `hash` before extracting it. An unknown version gives a path that does not exist, as the data dir alone does.
    pub async fn dir(&self, realm: Option<&Realm>, hash: &str) -> Result<PathBuf> {
        let dir = bundle_dir(&self.data_dir, realm, hash);
        if dir.is_dir() { return Ok(dir); }
        let Some(s3) = &self.s3 else { return Ok(dir) };
        let Some(bytes) = s3.get(&crate::names::bundle_key(realm, &s3.prefix, hash)).await? else { return Ok(dir) };
        let (got_dir, got) = store_archive(&self.data_dir, realm, &bytes)?;
        if got != hash {
            let _ = std::fs::remove_dir_all(&got_dir); let _ = std::fs::remove_file(archive_path(&self.data_dir, realm, &got));
            bail!("the bundle store's archive of {hash} hashes to {got}: refused");
        }
        Ok(dir)
    }

    /// After `store_archive` accepted an upload as `hash`: its archive goes to S3, before the caller records the version.
    pub async fn upload(&self, realm: Option<&Realm>, hash: &str, bytes: &[u8]) -> Result<()> {
        match &self.s3 { Some(s3) => s3.put(&crate::names::bundle_key(realm, &s3.prefix, hash), bytes).await, None => Ok(()) }
    }

    /// The archive of a stored version (`cori pull`): this replica's copy, else S3's, else rebuilt from the tree.
    pub async fn archive(&self, realm: Option<&Realm>, hash: &str) -> Result<Vec<u8>> {
        let p = archive_path(&self.data_dir, realm, hash);
        if p.exists() { return Ok(std::fs::read(p)?); }
        if let Some(s3) = &self.s3 { if let Some(b) = s3.get(&crate::names::bundle_key(realm, &s3.prefix, hash)).await? { return Ok(b); } }
        archive_of(&self.data_dir, realm, hash)
    }

    /// Removes a version's archive and tree: on this replica, and in S3 (the other replicas' caches fill again only from it).
    pub async fn remove(&self, realm: Option<&Realm>, hash: &str) {
        let _ = std::fs::remove_dir_all(bundle_dir(&self.data_dir, realm, hash));
        let _ = std::fs::remove_file(archive_path(&self.data_dir, realm, hash));
        if let Some(s3) = &self.s3 {
            if let Err(e) = s3.delete(&crate::names::bundle_key(realm, &s3.prefix, hash)).await { tracing::warn!(hash, error = %e, "bundle store: delete failed"); }
        }
    }
}

/// Where the files runs are given are kept (`api::files`): a bucket of their own, never the bundles'
/// (`CORI_FILE_STORE=s3://<bucket>[/<prefix>]`, on the S3 server and with the credentials and the encryption of the
/// `CORI_S3_*` settings), else each realm's `files/` folder of the data dir, when bundles are kept in the data dir too (one
/// replica). Bundles in S3 and no file store: uploads are refused, as one replica's disk would keep files the other
/// replicas cannot read. No replica caches a file: each read is the store's.
pub struct FileStore { data_dir: PathBuf, place: FilePlace }

enum FilePlace { DataDir, S3(S3), Unset }

/// Why uploads are refused (`FileStore::unavailable`).
const NO_FILE_STORE: &str = "file uploads need a bucket of their own: set CORI_FILE_STORE=s3://<bucket>[/<prefix>] (bundles are in S3, and one replica's disk would keep files the other replicas cannot read)";

/// The bucket of an `s3://<bucket>/<prefix>` address.
fn bucket_of(url: &str) -> Option<&str> { url.strip_prefix("s3://")?.split('/').next() }

impl FileStore {
    /// Files in the data dir: one replica (`cori up`, tests).
    pub fn local(data_dir: PathBuf) -> Self { Self { data_dir, place: FilePlace::DataDir } }

    /// The bucket `CORI_FILE_STORE` names, else the data dir when `bundles` keeps its archives there too, else none.
    pub fn from_env(data_dir: PathBuf, bundles: &BundleStore) -> Result<Self> { Self::from_lookup(data_dir, bundles.remote(), &env_var) }

    fn from_lookup(data_dir: PathBuf, bundles: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        match S3Settings::named("CORI_FILE_STORE", env)? {
            Some(settings) => {
                let store = Self::with_s3(data_dir, settings)?;
                // allowed, but files and bundles live and are read differently: their own bucket keeps them apart
                if let FilePlace::S3(s3) = &store.place && bundles.and_then(bucket_of) == Some(s3.name.as_str()) { tracing::warn!(bucket = %s3.name, "files and bundles share a bucket: give files a bucket of their own (CORI_FILE_STORE)"); }
                Ok(store)
            }
            None if bundles.is_some() => Ok(Self { data_dir, place: FilePlace::Unset }),
            None => Ok(Self::local(data_dir)),
        }
    }

    /// Files in the S3 bucket `settings` names.
    pub fn with_s3(data_dir: PathBuf, settings: S3Settings) -> Result<Self> { Ok(Self { data_dir, place: FilePlace::S3(S3::new(settings, "CORI_FILE_STORE", "file store")?) }) }

    /// `s3://<bucket>/<prefix>` when files live in S3 (`cori doctor`).
    pub fn remote(&self) -> Option<&str> { match &self.place { FilePlace::S3(s3) => Some(s3.url.as_str()), _ => None } }

    /// Why files cannot be kept, when they cannot: bundles in S3 and no bucket for files.
    pub fn unavailable(&self) -> Option<&'static str> { matches!(self.place, FilePlace::Unset).then_some(NO_FILE_STORE) }

    /// `cori preflight`: an object written, read back and deleted in the bucket (`preflight/<random>`). None without one.
    pub async fn check(&self) -> Option<Result<String>> { match &self.place { FilePlace::S3(s3) => Some(s3.round_trip().await), _ => None } }

    fn path(&self, realm: Option<&Realm>, id: &str) -> PathBuf { self.data_dir.join(crate::names::realm_folder(realm, "files")).join(id) }

    /// Keeps a file's bytes under its id (written whole: a reader never sees half of it).
    pub async fn put(&self, realm: Option<&Realm>, id: &str, bytes: &[u8]) -> Result<()> {
        if !cori_compiler::files::valid_id(id) { bail!("not a file id: `{id}`"); }
        match &self.place {
            FilePlace::S3(s3) => s3.put(&crate::names::file_key(realm, &s3.prefix, id), bytes).await,
            FilePlace::DataDir => {
                let p = self.path(realm, id);
                if let Some(d) = p.parent() { std::fs::create_dir_all(d)?; }
                let tmp = p.with_extension(format!("part-{}", uuid::Uuid::new_v4().simple()));
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &p)?;
                Ok(())
            }
            FilePlace::Unset => bail!("{NO_FILE_STORE}"),
        }
    }

    /// A file's bytes, None when the store has none under that id.
    pub async fn get(&self, realm: Option<&Realm>, id: &str) -> Result<Option<Vec<u8>>> {
        if !cori_compiler::files::valid_id(id) { return Ok(None); }
        match &self.place {
            FilePlace::S3(s3) => s3.get(&crate::names::file_key(realm, &s3.prefix, id)).await,
            FilePlace::DataDir => match std::fs::read(self.path(realm, id)) { Ok(b) => Ok(Some(b)), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None), Err(e) => Err(e.into()) },
            FilePlace::Unset => Ok(None),
        }
    }

    /// Removes a file's bytes (none there is no failure).
    pub async fn remove(&self, realm: Option<&Realm>, id: &str) -> Result<()> {
        if !cori_compiler::files::valid_id(id) { return Ok(()); }
        match &self.place {
            FilePlace::S3(s3) => s3.delete(&crate::names::file_key(realm, &s3.prefix, id)).await,
            FilePlace::DataDir => match std::fs::remove_file(self.path(realm, id)) { Ok(()) => Ok(()), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), Err(e) => Err(e.into()) },
            FilePlace::Unset => Ok(()),
        }
    }
}

impl S3 {
    /// A client of the bucket `settings` names; `var` names the setting in errors, `what` the store.
    fn new(settings: S3Settings, var: &str, what: &'static str) -> Result<Self> {
        let S3Settings { endpoint, region, bucket, prefix, key, secret, path_style, session_token, sse, sse_kms_key_id } = settings;
        // AWS addresses a bucket by host; other S3 servers (MinIO, R2, Ceph) by path, unless CORI_S3_PATH_STYLE says
        let style = if path_style.unwrap_or(endpoint.is_some()) { rusty_s3::UrlStyle::Path } else { rusty_s3::UrlStyle::VirtualHost };
        let endpoint = endpoint.unwrap_or_else(|| format!("https://s3.{region}.amazonaws.com"));
        let b = rusty_s3::Bucket::new(endpoint.parse().map_err(|e| anyhow!("CORI_S3_ENDPOINT `{endpoint}`: {e}"))?, style, bucket.clone(), region).map_err(|e| anyhow!("{var}: {e}"))?;
        let prefix = prefix.trim_matches('/').to_string();
        let url = format!("s3://{bucket}/{prefix}");
        let credentials = match session_token { Some(t) => rusty_s3::Credentials::new_with_token(key, secret, t), None => rusty_s3::Credentials::new(key, secret) };
        Ok(S3 {
            bucket: b, name: bucket, credentials, prefix: if prefix.is_empty() { prefix } else { format!("{prefix}/") },
            http: cori_engine::http::builder().timeout(std::time::Duration::from_secs(300)).build()?, url, sse: sse.map(|m| (m, sse_kms_key_id)), what,
        })
    }

    /// An object written, read back and deleted under the prefix (`preflight/<random>`): what the bucket's policy lets
    /// the server's key do.
    async fn round_trip(&self) -> Result<String> {
        let key = format!("{}preflight/{}", self.prefix, uuid::Uuid::new_v4().simple());
        let body = b"cori preflight".to_vec();
        self.put(&key, &body).await?;
        let back = self.get(&key).await?;
        self.delete(&key).await?;
        if back.as_deref() != Some(body.as_slice()) { bail!("{}: the object read back is not the one written", self.what); }
        Ok(format!("{} (put, get, delete{})", self.url, if self.sse.is_some() { ", with server-side encryption" } else { "" }))
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        use rusty_s3::S3Action;
        let r = self.http.get(self.bucket.get_object(Some(&self.credentials), key).sign(SIGNED_FOR)).send().await.map_err(|e| anyhow!("{}: GET {key}: {e}", self.what))?;
        if r.status().as_u16() == 404 { return Ok(None); }
        if !r.status().is_success() { bail!("{}: GET {key}: {}", self.what, r.status()); }
        Ok(Some(r.bytes().await?.to_vec()))
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        use rusty_s3::S3Action;
        let mut action = self.bucket.put_object(Some(&self.credentials), key);
        // the encryption a bucket policy may require of uploads: signed, and sent as signed
        let mut headers: Vec<(&str, String)> = Vec::new();
        if let Some((mode, kms_key)) = &self.sse {
            headers.push(("x-amz-server-side-encryption", mode.clone()));
            if let Some(k) = kms_key { headers.push(("x-amz-server-side-encryption-aws-kms-key-id", k.clone())); }
        }
        for (k, v) in &headers { action.headers_mut().insert(*k, v.clone()); }
        let mut req = self.http.put(action.sign(SIGNED_FOR)).body(bytes.to_vec());
        for (k, v) in &headers { req = req.header(*k, v); }
        let r = req.send().await.map_err(|e| anyhow!("{}: PUT {key}: {e}", self.what))?;
        if !r.status().is_success() { let status = r.status(); bail!("{}: PUT {key}: {status} {}", self.what, r.text().await.unwrap_or_default().chars().take(300).collect::<String>()); }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        use rusty_s3::S3Action;
        let r = self.http.delete(self.bucket.delete_object(Some(&self.credentials), key).sign(SIGNED_FOR)).send().await.map_err(|e| anyhow!("{}: DELETE {key}: {e}", self.what))?;
        if !r.status().is_success() && r.status().as_u16() != 404 { bail!("{}: DELETE {key}: {}", self.what, r.status()); }
        Ok(())
    }
}

/// The archive of a stored version: the uploaded one, or rebuilt from the tree for bundles stored before archives were kept.
pub fn archive_of(data_dir: &Path, realm: Option<&Realm>, hash: &str) -> Result<Vec<u8>> {
    let p = archive_path(data_dir, realm, hash);
    if p.exists() { return Ok(std::fs::read(p)?); }
    let dir = bundle_dir(data_dir, realm, hash);
    if !dir.exists() { bail!("unknown bundle {hash}"); }
    archive_from_files(&tree_files(&dir)?)
}

/// A `tar.zst` from a file map (deterministic: sorted paths, zero mtime and ids), the same the CLI builds.
pub fn archive_from_files(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let mut enc = zstd::Encoder::new(Vec::new(), 3)?;
    { let mut tar = tar::Builder::new(&mut enc); for (rel, bytes) in files { let mut h = tar::Header::new_gnu(); h.set_size(bytes.len() as u64); h.set_mode(0o644); h.set_mtime(0); h.set_uid(0); h.set_gid(0); tar.append_data(&mut h, rel, bytes.as_slice())?; } tar.finish()?; }
    Ok(enc.finish()?)
}

/// Text files of a bundle for the workers (`GET /v1/bundles/{hash}/files`): sources and fixtures, not the built front.
pub fn read_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !e.file_type().is_file() { continue; }
        let rel = e.path().strip_prefix(dir)?.to_string_lossy().to_string();
        if rel.contains("/.cori/") || rel.starts_with(".cori/") || rel.contains("node_modules/") || rel.starts_with("dist/") || credential_file(&rel) { continue; }
        if let Ok(t) = std::fs::read_to_string(e.path()) { out.insert(rel, t); }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEntry { pub id: String, pub dir: String, pub compiled: cori_compiler::Compiled }

/// Compiles every workflow folder (`workflows/<id>/` with a workflow.yaml) in the bundle, and checks its stores: every
/// error of every workflow, prefixed with its folder.
pub fn compile_all(dir: &Path) -> Result<Vec<WorkflowEntry>, Vec<String>> { compile_all_with(dir, vec![], None) }

/// `registry` rows (`GET /v1/connectors` shape) type the connector tools in the generated context; `deno` runs the Deno
/// passes, which load the steps' modules (a sandbox: `cori_engine::sandbox::SandboxChecks`), else this machine does.
pub fn compile_all_with(dir: &Path, registry: Vec<serde_json::Value>, deno: Option<std::sync::Arc<dyn cori_compiler::DenoRunner>>) -> Result<Vec<WorkflowEntry>, Vec<String>> {
    if !dir.join("cori.toml").exists() { return Err(vec![format!("{}: no cori.toml at the app root", dir.display())]); }
    if !dir.join("workflows").is_dir() { return Err(vec![format!("{}: no workflows/ folder (a workflow is workflows/<id>/workflow.yaml)", dir.display())]); }
    let app = cori_compiler::compile_app(dir, &cori_compiler::Options { run_deno: true, sdk_path: None, connectors: registry, deno });
    // stores: role files and rules against the migrations, bindings against the folders (errors block the version)
    let mut errors: Vec<String> = app.stores.errors.into_iter().map(|e| format!("stores: {e}")).collect();
    let mut out = Vec::new();
    for (d, r) in app.workflows {
        // `dir` is relative to the bundle root: it is the workflow's `root` in the worker's bundle cache
        let rel = format!("workflows/{}", d.file_name().unwrap().to_string_lossy());
        match r {
            Ok(c) => out.push(WorkflowEntry { id: c.id.clone(), dir: rel, compiled: c }),
            Err(es) => errors.extend(es.into_iter().map(|e| format!("{rel}: {e}"))),
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

/// The change review: per workflow, per external step: connector target, effect, lineage; plus the delta against the deployed version.
pub fn review(workflows: &[WorkflowEntry], deployed: Option<&[WorkflowEntry]>) -> Value {
    let effects = |ws: &[WorkflowEntry]| -> BTreeMap<String, BTreeMap<String, String>> {
        ws.iter().map(|w| (w.id.clone(), w.compiled.dag.nodes.iter().filter(|n| n.effect.external && !cori_compiler::is_container_kind(&n.kind)).map(|n| (n.name.clone(), format!("{} {:?}", n.effect.target, n.effect.access))).collect())).collect()
    };
    let now = effects(workflows);
    let before = deployed.map(effects).unwrap_or_default();
    let mut new_writes = Vec::new(); let mut removed = Vec::new(); let mut added_workflows = Vec::new();
    for (wf, steps) in &now {
        if !before.contains_key(wf) { added_workflows.push(wf.clone()); }
        for (step, eff) in steps { if before.get(wf).and_then(|b| b.get(step)) != Some(eff) && eff.contains("MayWrite") { new_writes.push(format!("{wf}.{step}: {eff}")); } }
    }
    for wf in before.keys() { if !now.contains_key(wf) { removed.push(wf.clone()); } }
    json!({
        "workflows": workflows.iter().map(|w| json!({
            "id": w.id, "hash": w.compiled.content_hash,
            "endpoint": w.compiled.manifest.endpoint, "schedule": w.compiled.manifest.schedule,
            "steps": w.compiled.dag.nodes.iter().filter(|n| !cori_compiler::is_container_kind(&n.kind)).filter(|n| n.nested_in.is_none() || n.effect.external).map(|n| json!({ "name": n.name, "kind": n.kind, "target": n.effect.target, "access": n.effect.access, "external": n.effect.external, "lineage": w.compiled.lineage.get(&n.name) })).collect::<Vec<_>>(),
            "schedule_levels": w.compiled.dag.schedule,
        })).collect::<Vec<_>>(),
        "delta": { "new_or_changed_writes": new_writes, "removed_workflows": removed, "added_workflows": added_workflows },
        "needs_approval": !new_writes.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_files_stay_out_of_bundles() {
        for f in [".env", "front/.env.local", ".npmrc", ".netrc", "keys/server.pem", "tls.key", "a/b.p12", "c.pfx"] { assert!(credential_file(f), "{f}"); }
        for f in ["cori.toml", "front/src/env.ts", "fixtures/keys.json", "environment.md", ".environment", "monkey"] { assert!(!credential_file(f), "{f}"); }
    }

    /// A bundle store on a minimal S3 server (path-style PUT, GET, DELETE; signatures not checked): an upload lands in
    /// the bucket, a replica with an empty cache gets the tree back, a tampered archive is refused, a removal deletes it.
    /// Against a real S3 server (`CORI_E2E_S3_ENDPOINT`, `_BUCKET`, `_KEY`, `_SECRET`; `scripts/e2e-s3.sh` starts one):
    /// signed requests are taken, an archive goes in and comes back checked, the preflight's round trip passes, and a wrong
    /// secret is refused (the server checks signatures, unlike the stand-in above).
    #[tokio::test]
    async fn a_real_s3_server_takes_signed_requests_only() {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(endpoint), Some(bucket), Some(key), Some(secret)) = (env("CORI_E2E_S3_ENDPOINT"), env("CORI_E2E_S3_BUCKET"), env("CORI_E2E_S3_KEY"), env("CORI_E2E_S3_SECRET")) else { eprintln!("CORI_E2E_S3_ENDPOINT not set: skipped"); return };
        let settings = |secret: &str, sse: Option<&str>| S3Settings { endpoint: Some(endpoint.clone()), region: "us-east-1".into(), bucket: bucket.clone(), prefix: format!("cori-test-{}", uuid::Uuid::new_v4().simple()), key: key.clone(), secret: secret.into(), sse: sse.map(String::from), ..Default::default() };
        let dir = std::env::temp_dir().join(format!("cori-s3-{}", uuid::Uuid::new_v4().simple()));
        let store = BundleStore::with_s3(dir.clone(), settings(&secret, None)).unwrap();
        let mut files = BTreeMap::new(); files.insert("cori.toml".to_string(), b"[app]\nid = \"s3\"\n".to_vec());
        let archive = archive_from_files(&files).unwrap();
        let hash = manifest_hash(&files);
        store.upload(None, &hash, &archive).await.expect("a signed PUT");
        assert_eq!(store.archive(None, &hash).await.unwrap(), archive, "a signed GET");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(store.dir(None, &hash).await.unwrap().join("cori.toml").is_file(), "the tree rebuilt from the bucket");
        assert!(store.check().await.unwrap().is_ok(), "the preflight's round trip");
        store.remove(None, &hash).await;
        let forged = BundleStore::with_s3(dir.join("forged"), settings("not-the-secret", None)).unwrap();
        let e = forged.upload(None, &hash, &archive).await.unwrap_err().to_string();
        assert!(e.contains("403"), "a wrong signature is refused: {e}");
        // server-side encryption asked for: signed with the request; a server that does not do it says so
        let sse = BundleStore::with_s3(dir.join("sse"), settings(&secret, Some("AES256"))).unwrap();
        match sse.upload(None, &hash, &archive).await { Ok(()) => { sse.remove(None, &hash).await; } Err(e) => eprintln!("this S3 server refuses SSE-S3 (fine for the test): {e}") }
    }

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a { move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()) }

    #[test]
    fn files_get_a_bucket_of_their_own() {
        let dir = std::env::temp_dir().join("cori-file-store-test");
        let s3 = [("CORI_S3_ACCESS_KEY_ID", "k"), ("CORI_S3_SECRET_ACCESS_KEY", "s"), ("CORI_S3_ENDPOINT", "http://127.0.0.1:9")];
        // bundles in the data dir and no file store: files there too (one replica)
        let f = FileStore::from_lookup(dir.clone(), None, &lookup(&s3)).unwrap();
        assert!(f.unavailable().is_none() && f.remote().is_none());
        // bundles in S3 and no file store: uploads are refused, never kept on one replica's disk
        let f = FileStore::from_lookup(dir.clone(), Some("s3://bundles/cori"), &lookup(&s3)).unwrap();
        assert!(f.unavailable().is_some_and(|why| why.contains("CORI_FILE_STORE")));
        // a bucket of their own, on the same S3 server and with the same key
        let with = [s3[0], s3[1], s3[2], ("CORI_FILE_STORE", "s3://files/cori")];
        let f = FileStore::from_lookup(dir.clone(), Some("s3://bundles/cori"), &lookup(&with)).unwrap();
        assert_eq!((f.remote(), f.unavailable()), (Some("s3://files/cori"), None));
        // no bucket, no s3://, no credentials: refused at start
        for bad in [[("CORI_FILE_STORE", "s3:///x")], [("CORI_FILE_STORE", "files")], [("CORI_FILE_STORE", "s3://files")]] {
            assert!(FileStore::from_lookup(dir.clone(), None, &lookup(&bad)).is_err(), "{bad:?}");
        }
        assert_eq!(bucket_of("s3://bundles/cori/x"), Some("bundles"));
    }

    #[tokio::test]
    async fn files_go_to_their_own_bucket_never_the_bundles() {
        use axum::{Router, extract::{Path as AxPath, State}, routing::put, body::Bytes, http::StatusCode};
        type Objects = std::sync::Arc<std::sync::Mutex<BTreeMap<String, Vec<u8>>>>;
        let objects: Objects = Default::default();
        async fn put_o(State(o): State<Objects>, AxPath((b, k)): AxPath<(String, String)>, body: Bytes) -> StatusCode { o.lock().unwrap().insert(format!("{b}/{k}"), body.to_vec()); StatusCode::OK }
        async fn get_o(State(o): State<Objects>, AxPath((b, k)): AxPath<(String, String)>) -> Result<Vec<u8>, StatusCode> { o.lock().unwrap().get(&format!("{b}/{k}")).cloned().ok_or(StatusCode::NOT_FOUND) }
        async fn del_o(State(o): State<Objects>, AxPath((b, k)): AxPath<(String, String)>) -> StatusCode { o.lock().unwrap().remove(&format!("{b}/{k}")); StatusCode::NO_CONTENT }
        let app = Router::new().route("/{bucket}/{*key}", put(put_o).get(get_o).delete(del_o)).with_state(objects.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let settings = |bucket: &str| S3Settings { endpoint: Some(format!("http://{addr}")), region: "us-east-1".into(), bucket: bucket.into(), prefix: "cori".into(), key: "k".into(), secret: "s".into(), ..Default::default() };
        let root = std::env::temp_dir().join(format!("cori-file-store-{}", uuid::Uuid::new_v4().simple()));
        let bundles = BundleStore::with_s3(root.join("b"), settings("bundles")).unwrap();
        let files = FileStore::with_s3(root.join("f"), settings("files")).unwrap();
        let realm = Realm::new("acme");
        files.put(Some(&realm), "file_1", b"%PDF-1.7").await.unwrap();
        files.put(None, "file_2", b"a,b").await.unwrap();
        let keys: Vec<String> = objects.lock().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["files/cori/file_2".to_string(), "files/cori/realms/acme/file_1".to_string()], "the file store's bucket, by realm");
        assert_eq!(files.get(Some(&realm), "file_1").await.unwrap().as_deref(), Some(&b"%PDF-1.7"[..]));
        assert!(files.get(None, "file_1").await.unwrap().is_none(), "another realm's key");
        assert!(files.get(None, "../bundles/cori/x").await.unwrap().is_none(), "only file ids are read");
        files.remove(Some(&realm), "file_1").await.unwrap();
        assert!(files.get(Some(&realm), "file_1").await.unwrap().is_none());
        assert!(files.check().await.unwrap().is_ok(), "the preflight's round trip in the files' bucket");
        assert!(bundles.remote() == Some("s3://bundles/cori") && files.remote() == Some("s3://files/cori"));
        assert!(!objects.lock().unwrap().keys().any(|k| k.starts_with("bundles/")), "nothing of the files in the bundles' bucket");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The file store against a real S3 server (`scripts/e2e-s3.sh`: `CORI_E2E_S3_FILES_BUCKET`): signed requests in a
    /// bucket of its own.
    #[tokio::test]
    async fn a_real_s3_server_keeps_files_in_their_own_bucket() {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(endpoint), Some(bucket), Some(key), Some(secret)) = (env("CORI_E2E_S3_ENDPOINT"), env("CORI_E2E_S3_FILES_BUCKET"), env("CORI_E2E_S3_KEY"), env("CORI_E2E_S3_SECRET")) else { eprintln!("CORI_E2E_S3_FILES_BUCKET not set: skipped"); return };
        let settings = |secret: &str| S3Settings { endpoint: Some(endpoint.clone()), region: "us-east-1".into(), bucket: bucket.clone(), prefix: format!("cori-test-{}", uuid::Uuid::new_v4().simple()), key: key.clone(), secret: secret.into(), ..Default::default() };
        let dir = std::env::temp_dir().join(format!("cori-s3-files-{}", uuid::Uuid::new_v4().simple()));
        let files = FileStore::with_s3(dir.clone(), settings(&secret)).unwrap();
        files.put(None, "file_e2e", b"%PDF-1.7").await.expect("a signed PUT");
        assert_eq!(files.get(None, "file_e2e").await.unwrap().as_deref(), Some(&b"%PDF-1.7"[..]), "a signed GET");
        assert!(files.check().await.unwrap().is_ok(), "the preflight's round trip");
        files.remove(None, "file_e2e").await.unwrap();
        assert!(files.get(None, "file_e2e").await.unwrap().is_none());
        let forged = FileStore::with_s3(dir.clone(), settings("not-the-secret")).unwrap();
        let e = forged.put(None, "file_e2e", b"x").await.unwrap_err().to_string();
        assert!(e.contains("file store") && e.contains("403"), "a wrong signature is refused, as the file store's: {e}");
    }

    #[tokio::test]
    async fn a_replica_rebuilds_a_version_from_the_bundle_store() {
        use axum::{Router, extract::{Path as AxPath, State}, routing::put, body::Bytes, http::StatusCode};
        type Objects = std::sync::Arc<std::sync::Mutex<BTreeMap<String, Vec<u8>>>>;
        let objects: Objects = Default::default();
        async fn put_o(State(o): State<Objects>, AxPath(k): AxPath<String>, b: Bytes) -> StatusCode { o.lock().unwrap().insert(k, b.to_vec()); StatusCode::OK }
        async fn get_o(State(o): State<Objects>, AxPath(k): AxPath<String>) -> Result<Vec<u8>, StatusCode> { o.lock().unwrap().get(&k).cloned().ok_or(StatusCode::NOT_FOUND) }
        async fn del_o(State(o): State<Objects>, AxPath(k): AxPath<String>) -> StatusCode { o.lock().unwrap().remove(&k); StatusCode::NO_CONTENT }
        let app = Router::new().route("/bundles-test/{*key}", put(put_o).get(get_o).delete(del_o)).with_state(objects.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = |dir: &Path| BundleStore::with_s3(dir.to_path_buf(), S3Settings { endpoint: Some(format!("http://{addr}")), region: "us-east-1".into(), bucket: "bundles-test".into(), prefix: "cori".into(), key: "k".into(), secret: "s".into(), ..Default::default() }).unwrap();
        let root = std::env::temp_dir().join(format!("cori-bundle-store-{}", uuid::Uuid::new_v4().simple()));
        let (a, b) = (store(&root.join("a")), store(&root.join("b")));
        let mut files = BTreeMap::new();
        files.insert("cori.toml".to_string(), b"[app]\nid = \"t\"\n".to_vec());
        let bytes = archive_from_files(&files).unwrap();
        // replica a: the upload
        let (_, hash) = store_archive(a.data_dir(), None, &bytes).unwrap();
        a.upload(None, &hash, &bytes).await.unwrap();
        assert!(objects.lock().unwrap().contains_key(&crate::names::bundle_key(None, "cori/", &hash)));
        // replica b: nothing cached, the tree comes back from the bucket
        let dir = b.dir(None, &hash).await.unwrap();
        assert!(dir.join("cori.toml").is_file());
        assert_eq!(b.archive(None, &hash).await.unwrap(), bytes);
        // an archive that does not hash to its key is refused
        files.insert("evil.ts".to_string(), b"x".to_vec());
        let other = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        objects.lock().unwrap().insert(crate::names::bundle_key(None, "cori/", other).trim_start_matches('/').to_string(), archive_from_files(&files).unwrap());
        let e = b.dir(None, other).await.unwrap_err().to_string();
        assert!(e.contains("hashes to"), "{e}");
        // an unknown version is a path that does not exist, as on the data dir alone
        assert!(!b.dir(None, "sha256:ffff").await.unwrap().exists());
        a.remove(None, &hash).await;
        assert!(!objects.lock().unwrap().contains_key(&crate::names::bundle_key(None, "cori/", &hash)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_carrying_a_credential_file_is_refused() {
        let data = std::env::temp_dir().join(format!("cori-bundles-test-{}", std::process::id()));
        let mut files = BTreeMap::new();
        files.insert("cori.toml".to_string(), b"[app]\nid = \"t\"\n".to_vec());
        files.insert("front/.env.local".to_string(), b"KEY=v".to_vec());
        let err = store_archive(&data, None, &archive_from_files(&files).unwrap()).unwrap_err().to_string();
        assert!(err.contains("credential files (front/.env.local)"), "{err}");
        files.remove("front/.env.local");
        let (dir, hash) = store_archive(&data, None, &archive_from_files(&files).unwrap()).unwrap();
        assert_eq!(hash, manifest_hash(&files));
        assert!(dir.join("cori.toml").is_file());
        let _ = std::fs::remove_dir_all(&data);
    }
}

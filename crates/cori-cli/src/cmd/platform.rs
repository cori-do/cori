// SPDX-License-Identifier: Apache-2.0
//! Running the platform: `cori serve` (the server), `cori work` (a worker), `cori sandbox serve` (where builders' code
//! runs), `cori up` (everything local).
use crate::cli::*;
use crate::embedded_pg;

pub async fn run(cmd: Cmd, modules: &cori_server::module::Modules) -> anyhow::Result<()> {
    // a CA bundle that does not read would leave every call to an internal service failing later, one by one
    cori_engine::http::check().map_err(|e| anyhow::anyhow!("{e}"))?;
    match cmd {
        Cmd::Serve { listen, database_url, data_dir, temporal, namespace, task_queue, auth, dev_mocks, public_url, internal_url, approver_roles, ingest_token, store_database_url, no_mock, allow_insecure, preflight, json } => {
            let data_dir = data_dir.unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cori/server"));
            let mut cfg = cori_server::ServerConfig::local(data_dir);
            cfg.listen = listen.clone(); cfg.public_url = public_url.unwrap_or_else(|| format!("http://{listen}")); cfg.temporal = temporal; cfg.namespace = namespace; cfg.task_queue = task_queue;
            cfg.dev_mocks = dev_mocks; cfg.ingest_token = ingest_token; if store_database_url.is_some() { cfg.store_database_url = store_database_url; } cfg.no_mocks = no_mock;
            cfg.allow_insecure = allow_insecure; cfg.internal_url = internal_url; cfg.modules = modules.clone();
            let approver_roles: Vec<String> = approver_roles.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
            if !approver_roles.is_empty() { cfg.approver_roles = approver_roles; }
            if let Some(u) = database_url { cfg.database_url = u; }
            cfg.auth = match auth.as_str() { "off" => cori_server::auth::AuthMode::Off, "dev" => cori_server::auth::AuthMode::Dev, "builtin" => {
                let issuer = std::env::var("CORI_AUTH_ISSUER").map_err(|_| anyhow::anyhow!("--auth builtin: set CORI_AUTH_ISSUER (and CORI_AUTH_AUDIENCE, CORI_AUTH_ROLES_CLAIM, CORI_AUTH_SUBJECT_CLAIM)"))?;
                cori_server::auth::AuthMode::Builtin { issuer, audience: std::env::var("CORI_AUTH_AUDIENCE").ok().filter(|a| !a.is_empty()), roles_claim: std::env::var("CORI_AUTH_ROLES_CLAIM").unwrap_or_else(|_| "roles".into()),
                    subject_claim: std::env::var("CORI_AUTH_SUBJECT_CLAIM").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "sub".into()) } }
                other => anyhow::bail!("--auth {other}: expected dev, builtin or off") };
            if preflight {
                let checks = cori_server::preflight::run(&cfg).await;
                if json { println!("{}", serde_json::to_string_pretty(&checks)?); } else { print!("{}", cori_server::preflight::table(&checks)); }
                let failed = checks.iter().filter(|c| c.is_failed()).count();
                if failed > 0 { anyhow::bail!("preflight: {failed} check(s) failed"); }
                return Ok(());
            }
            // the server never starts Temporal: `cori up` runs a dev server for the laptop
            cori_server::serve(cfg).await?;
        }
        Cmd::Work { queue, temporal, namespace, pool, key, server, shards } => {
            let shards = if pool.is_some() { 1 } else { shards };
            let queue = match (pool, key) {
                (Some(p), Some(k)) => {
                    let caps = serde_json::json!({ "binaries": ["gws", "deno"], "os": std::env::consts::OS });
                    let r = cori_engine::http::client().post(format!("{}/v1/pools/{p}/heartbeat", server.trim_end_matches('/'))).header("x-cori-pool-key", &k).json(&serde_json::json!({ "capabilities": caps })).send().await?;
                    if !r.status().is_success() { anyhow::bail!("pool heartbeat failed: {}", r.text().await.unwrap_or_default()); }
                    let v: serde_json::Value = r.json().await?;
                    // the pool's own key authenticates its calls to the server (events, leases, store calls)
                    unsafe { std::env::set_var("CORI_WORKER_POOL", &p); std::env::set_var("CORI_POOL_KEY", &k); std::env::remove_var("CORI_WORKER_TOKEN"); }
                    let q = v["task_queue"].as_str().unwrap_or(&queue).to_string(); println!("pool {p} · polling {q}"); q
                }
                (Some(_), None) => anyhow::bail!("--pool needs --key (or CORI_POOL_KEY)"),
                _ => queue,
            };
            cori_engine::run::serve_worker_shards(&temporal, &namespace, &queue, shards).await?;
        }
        Cmd::Up { listen, data_dir, store_database_url, no_mock, no_store } => {
            let data_dir = data_dir.unwrap_or_else(|| dirs::home_dir().unwrap().join(".cori/up"));
            let mut cfg = cori_server::ServerConfig::local(data_dir.clone());
            cfg.listen = listen.clone(); cfg.public_url = format!("http://{listen}"); if store_database_url.is_some() { cfg.store_database_url = store_database_url; } cfg.no_mocks = no_mock;
            // `cori up` is the laptop server: on another address it refuses like `cori serve`, unless CORI_ALLOW_INSECURE=true
            cfg.allow_insecure = cori_server::env_flag("CORI_ALLOW_INSECURE", false);
            // its in-process worker polls the one shared queue: sharding would start runs no worker polls
            cfg.shards = 1;
            cfg.modules = modules.clone();
            // embedded Postgres, a cluster under the data dir run by the machine's `pg_ctl` (Postgres.app, Homebrew, PATH): the
            // platform database `cori` unless CORI_DATABASE_URL names one, and the stores unless --store-database-url or
            // --no-store. Started before Temporal, so a missing Postgres leaves nothing running.
            let mut pg: Option<embedded_pg::Embedded> = None;
            if cfg.database_url.is_empty() || (cfg.store_database_url.is_none() && !no_store) {
                let e = embedded_pg::start(&data_dir.join("pg")).await.map_err(|e| anyhow::anyhow!("cori up runs on Postgres and found none ({e}): install it (Postgres.app, or `brew install postgresql@17`), or set CORI_DATABASE_URL"))?;
                if cfg.database_url.is_empty() { cfg.database_url = cori_server::db::ensure_database(&e.url, "cori").await?; }
                if cfg.store_database_url.is_none() && !no_store { cfg.store_database_url = Some(e.url.clone()); }
                println!("cori up · embedded postgres on port {} ({})", e.port, e.bin.display());
                pg = Some(e);
            }
            let temporal_child = match cori_engine::run::ensure_local_temporal().await { Ok(c) => c, Err(err) => { if let Some(e) = &pg { e.stop(); } return Err(err); } };
            let temporal = cfg.temporal.clone(); let queue = cfg.task_queue.clone();
            // a dev box has no gVisor pool: the in-process worker also serves the sandbox pool's queue
            let sandbox = cori_server::names::sandbox_queue(&cfg);
            let mocks = if cfg.store_database_url.is_some() { "connector mocks on · stores on postgres (fixtures ignored)" } else { "dev mocks on (stores mocked too)" };
            println!("cori up · server http://{listen} · temporal {temporal} · worker on {queue} · {mocks} · auth dev (Authorization: Bearer dev:<user>:<roles>)");
            // The Temporal worker future is !Send: it gets its own OS thread and single-threaded runtime (as the desktop app did).
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("worker runtime");
                rt.block_on(async move { if let Err(e) = cori_engine::run::serve_worker_queues(&temporal, "default", vec![queue, sandbox]).await { eprintln!("worker: {e}"); } });
            });
            // Second Ctrl+C while shutting down: leave immediately.
            tokio::spawn(async { let _ = tokio::signal::ctrl_c().await; let _ = tokio::signal::ctrl_c().await; eprintln!("forced exit"); std::process::exit(130); });
            // the embedded Postgres and the dev Temporal stop however the server ends (signal, busy port, refused settings)
            let served = cori_server::serve(cfg).await;
            if let Some(e) = pg { e.stop(); }
            if let Some(mut c) = temporal_child { let _ = c.kill(); let _ = c.wait(); println!("cori up · stopped (server, worker, dev Temporal)"); } else { println!("cori up · stopped (server, worker); Temporal was already running and is left as is"); }
            served?;
            std::process::exit(0);
        }
        Cmd::Sandbox { what: SandboxCmd::Serve { listen, work, contained } } => {
            let opts = cori_engine::sandbox_server::SandboxOptions::from_env(listen, work, contained);
            std::fs::create_dir_all(&opts.work)?;
            let work = opts.work.canonicalize()?;
            // its home, temporary and cache folders (Cori's runtime files) are in the work folder: what a call writes there
            // is emptied with it. Deno's cache too, where Deno keeps the modules it transpiles; its packages stay in the
            // cache Deno had (in a cluster, the image's, read-only), linked from there (`sandbox_server::deno_cache`).
            let packages = std::env::var_os("DENO_DIR").filter(|v| !v.is_empty()).map(std::path::PathBuf::from).or_else(|| dirs::cache_dir().map(|d| d.join("deno")));
            // without symbolic links (not unix), Deno keeps the cache it had
            let deno_packages = if cfg!(unix) { packages.clone() } else { None };
            let deno_dir = if deno_packages.is_some() { Some(work.join("deno")) } else { packages };
            unsafe {
                std::env::set_var("HOME", work.join("home"));
                std::env::set_var("TMPDIR", work.join("tmp"));
                std::env::set_var("CORI_CACHE_DIR", work.join("cache"));
                if let Some(d) = &deno_dir { std::env::set_var("DENO_DIR", d); }
            }
            cori_engine::sandbox_server::serve(cori_engine::sandbox_server::SandboxOptions { work, deno_packages, ..opts }).await?;
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}

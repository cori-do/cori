// SPDX-License-Identifier: Apache-2.0
//! Embedded Postgres for `cori up`: initialises and runs a private cluster under the data dir with the machine's
//! own `pg_ctl` (Postgres.app, Homebrew, or PATH). Stores get a real database on a laptop with nothing to configure.
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

pub struct Embedded { pub url: String, pub port: u16, pub bin: PathBuf, data: PathBuf }

fn find_bin() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("CORI_PG_BIN") { candidates.push(PathBuf::from(p)); }
    for v in ["latest", "17", "16", "15"] { candidates.push(PathBuf::from(format!("/Applications/Postgres.app/Contents/Versions/{v}/bin"))); }
    for v in ["postgresql@17", "postgresql@16", "postgresql@15", "postgresql"] { candidates.push(PathBuf::from(format!("/opt/homebrew/opt/{v}/bin"))); candidates.push(PathBuf::from(format!("/usr/local/opt/{v}/bin"))); }
    candidates.push(PathBuf::from("/usr/lib/postgresql/16/bin")); candidates.push(PathBuf::from("/usr/lib/postgresql/17/bin"));
    for c in candidates { if c.join("pg_ctl").exists() && c.join("initdb").exists() { return Some(c); } }
    // PATH
    let out = std::process::Command::new("sh").args(["-c", "command -v pg_ctl"]).output().ok()?;
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() { None } else { Path::new(&p).parent().map(|d| d.to_path_buf()) }
}

fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(55499) }

pub async fn start(data: &Path) -> Result<Embedded> {
    let bin = find_bin().ok_or_else(|| anyhow!("pg_ctl not found"))?;
    std::fs::create_dir_all(data)?;
    let cluster = data.join("cluster");
    if !cluster.join("PG_VERSION").exists() {
        let out = std::process::Command::new(bin.join("initdb")).args(["-D", &cluster.display().to_string(), "-U", "cori", "--auth=trust", "-E", "UTF8", "--no-locale"]).output().context("initdb")?;
        if !out.status.success() { return Err(anyhow!("initdb failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""))); }
    }
    // reuse a running cluster (a previous `cori up` still owns it), else start on a free port
    let port_file = data.join("port");
    if let Ok(p) = std::fs::read_to_string(&port_file) { if let Ok(port) = p.trim().parse::<u16>() { if std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), std::time::Duration::from_millis(300)).is_ok() { return Ok(Embedded { url: format!("postgres://cori@127.0.0.1:{port}/postgres"), port, bin, data: data.to_path_buf() }); } } }
    let port = free_port();
    let sock = std::env::temp_dir().join(format!("cori-pg-{port}")); std::fs::create_dir_all(&sock)?;
    let log = data.join("postgres.log");
    let out = std::process::Command::new(bin.join("pg_ctl")).args(["-D", &cluster.display().to_string(), "-l", &log.display().to_string(), "-w", "-t", "30", "-o", &format!("-p {port} -k {} -c listen_addresses=127.0.0.1", sock.display()), "start"]).output().context("pg_ctl start")?;
    if !out.status.success() { return Err(anyhow!("pg_ctl start failed: {} (see {})", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""), log.display())); }
    std::fs::write(&port_file, port.to_string())?;
    Ok(Embedded { url: format!("postgres://cori@127.0.0.1:{port}/postgres"), port, bin, data: data.to_path_buf() })
}

/// The admin URL of the embedded cluster under `data` when a `cori up` is running it (the port file is live), else None.
pub fn running_url(data: &std::path::Path) -> Option<String> {
    let port: u16 = std::fs::read_to_string(data.join("port")).ok()?.trim().parse().ok()?;
    std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().ok()?, std::time::Duration::from_millis(300)).ok()?;
    Some(format!("postgres://cori@127.0.0.1:{port}/postgres"))
}

impl Embedded {
    pub fn stop(&self) {
        let _ = std::process::Command::new(self.bin.join("pg_ctl")).args(["-D", &self.data.join("cluster").display().to_string(), "-m", "fast", "-w", "stop"]).output();
        let _ = std::fs::remove_file(self.data.join("port"));
    }
}

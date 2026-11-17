// SPDX-License-Identifier: Apache-2.0
//! Temporal connection.
use anyhow::{Context, Result};
use std::sync::Arc;
use temporalio_client::{Client, ClientOptions, ClientTlsOptions, Connection, ConnectionOptions, TlsOptions};
use temporalio_sdk::runtime::{Runtime, RuntimeOptions};
use temporalio_sdk_core::Url;

/// The namespace when none is given: `CORI_TEMPORAL_NAMESPACE`, else `default` (the dev server's).
pub fn default_namespace() -> String {
    std::env::var("CORI_TEMPORAL_NAMESPACE").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "default".into())
}

fn env_path(name: &str) -> Option<String> { std::env::var(name).ok().filter(|v| !v.trim().is_empty()) }

fn read_pem(var: &str, path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("{var}: cannot read {path}"))
}

/// Connection options for the Temporal frontend at `target`, the same on the server, the workers and the cli.
/// `target` is `host:port` or a URL; TLS is on when the URL is `https://`, `CORI_TEMPORAL_TLS=true`, or any TLS file is
/// set. From the environment:
/// - `CORI_TEMPORAL_TLS_CA`: PEM file of the CA that signed the frontend's certificate (system roots otherwise);
/// - `CORI_TEMPORAL_TLS_CERT` + `CORI_TEMPORAL_TLS_KEY`: PEM client certificate and key (mTLS);
/// - `CORI_TEMPORAL_TLS_SERVER_NAME`: the name to verify when it differs from the host;
/// - `CORI_TEMPORAL_API_KEY`: sent as a bearer on every call (Temporal Cloud API keys, or a JWT for the authorizer).
pub fn connection_options(target: &str, identity: &str) -> Result<ConnectionOptions> {
    let (ca, cert, key, server_name) = (env_path("CORI_TEMPORAL_TLS_CA"), env_path("CORI_TEMPORAL_TLS_CERT"), env_path("CORI_TEMPORAL_TLS_KEY"), env_path("CORI_TEMPORAL_TLS_SERVER_NAME"));
    let flag = std::env::var("CORI_TEMPORAL_TLS").map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")).unwrap_or(false);
    let tls = flag || target.starts_with("https://") || ca.is_some() || cert.is_some() || key.is_some() || server_name.is_some();
    let url = temporal_url(target, tls)?;
    let tls_options = if tls {
        let client = match (&cert, &key) {
            (Some(c), Some(k)) => Some(ClientTlsOptions::builder().client_cert(read_pem("CORI_TEMPORAL_TLS_CERT", c)?).client_private_key(read_pem("CORI_TEMPORAL_TLS_KEY", k)?).build()),
            (None, None) => None,
            _ => return Err(anyhow::anyhow!("mTLS needs both CORI_TEMPORAL_TLS_CERT and CORI_TEMPORAL_TLS_KEY")),
        };
        let ca = match &ca { Some(p) => Some(read_pem("CORI_TEMPORAL_TLS_CA", p)?), None => None };
        Some(TlsOptions::builder().maybe_server_root_ca_cert(ca).maybe_domain(server_name).maybe_client_tls_options(client).build())
    } else { None };
    let api_key = std::env::var("CORI_TEMPORAL_API_KEY").ok().filter(|v| !v.trim().is_empty());
    Ok(ConnectionOptions::new(url).identity(identity.to_string()).maybe_tls_options(tls_options).maybe_api_key(api_key).connect_timeout(std::time::Duration::from_secs(10)).build())
}

/// `host:port` or a URL → a URL whose scheme matches the TLS choice.
fn temporal_url(target: &str, tls: bool) -> Result<Url> {
    let rest = target.split_once("://").map(|(_, r)| r).unwrap_or(target);
    let url = format!("{}://{}", if tls { "https" } else { "http" }, rest.trim_end_matches('/'));
    Url::parse(&url).with_context(|| format!("invalid Temporal address `{target}` (host:port or http(s)://host:port)"))
}

pub struct TemporalRuntime { pub core: Arc<Runtime>, pub client: Arc<Client>, pub task_queue: String, pub namespace: String }

impl TemporalRuntime {
    pub async fn connect(target: &str, namespace: &str, task_queue: &str) -> Result<Self> {
        let core = Runtime::from_current_tokio(RuntimeOptions::builder().build().map_err(anyhow::Error::msg)?)?;
        let connection = Connection::connect(connection_options(target, "cori")?).await.with_context(|| format!("could not connect to Temporal at `{target}` (namespace `{namespace}`); locally, start one with `temporal server start-dev`"))?;
        let client = Client::new(connection, ClientOptions::new(namespace.to_string()).build())?;
        Ok(Self { core: Arc::new(core), client: Arc::new(client), task_queue: task_queue.to_string(), namespace: namespace.to_string() })
    }
}

pub fn port_open(host: &str, port: u16) -> bool {
    let Ok(addr) = format!("{host}:{port}").parse() else { return false };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// Ctrl+C, or SIGTERM (how Kubernetes and Docker stop a container): what `cori serve`, `cori work` and `cori up` stop on,
/// finishing in-flight requests and activity tasks first.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => { tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} } }
            Err(_) => { let _ = tokio::signal::ctrl_c().await; }
        }
    }
    #[cfg(not(unix))]
    { let _ = tokio::signal::ctrl_c().await; }
}

#[cfg(test)]
mod tests {
    use super::temporal_url;

    #[test]
    fn urls_follow_the_tls_choice() {
        assert_eq!(temporal_url("temporal-frontend.temporal:7233", false).unwrap().as_str(), "http://temporal-frontend.temporal:7233/");
        assert_eq!(temporal_url("http://127.0.0.1:7233", true).unwrap().as_str(), "https://127.0.0.1:7233/");
        assert_eq!(temporal_url("https://ns.acct.tmprl.cloud:7233", true).unwrap().as_str(), "https://ns.acct.tmprl.cloud:7233/");
    }
}

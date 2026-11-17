// SPDX-License-Identifier: Apache-2.0
//! Outbound HTTP: every client Cori builds (the issuer, the hooks, webhooks, S3, LLM providers, connectors, the workers'
//! calls to the server, a key service) trusts the system's roots and, on a network with a private CA, the deployment's
//! own bundle: `CORI_CA_FILE`, one or more PEM certificates. A proxy is taken from `HTTPS_PROXY` / `HTTP_PROXY` /
//! `NO_PROXY`, as reqwest does by default.
use std::sync::OnceLock;

/// The certificates of `CORI_CA_FILE`, read once: none when it is unset.
fn extra_roots() -> &'static Result<Vec<reqwest::Certificate>, String> {
    static ROOTS: OnceLock<Result<Vec<reqwest::Certificate>, String>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let Some(path) = std::env::var("CORI_CA_FILE").ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) else { return Ok(Vec::new()) };
        let pem = std::fs::read(&path).map_err(|e| format!("CORI_CA_FILE `{path}`: {e}"))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| format!("CORI_CA_FILE `{path}`: {e}"))?;
        if certs.is_empty() { return Err(format!("CORI_CA_FILE `{path}` holds no PEM certificate")); }
        Ok(certs)
    })
}

/// Whether `CORI_CA_FILE` reads (`serve`, `work` and `sandbox serve` check it at start): its problem otherwise.
pub fn check() -> Result<usize, String> { extra_roots().as_ref().map(|c| c.len()).map_err(|e| e.clone()) }

/// A client builder that trusts the deployment's CA bundle besides the system's roots. A bundle that does not read is
/// left out here (`check` refuses it at start).
pub fn builder() -> reqwest::ClientBuilder {
    let mut b = reqwest::Client::builder();
    if let Ok(certs) = extra_roots() { for c in certs { b = b.add_root_certificate(c.clone()); } }
    b
}

/// A shared client with `builder`'s trust and no timeout of its own (callers set one per request).
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| builder().build().unwrap_or_else(|e| { tracing::warn!(error = %e, "HTTP client with the deployment's CA bundle"); reqwest::Client::new() })).clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_ca_bundle_must_hold_certificates() {
        assert!(reqwest::Certificate::from_pem_bundle(b"not a certificate").map(|c| c.is_empty()).unwrap_or(true));
    }
}

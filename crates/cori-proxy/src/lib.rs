//! # cori-proxy
//!
//! Postgres wire protocol proxy for Cori AI Database Proxy.
//!
//! This crate implements a 100% Postgres wire-compatible proxy that:
//! - Accepts Postgres wire protocol connections
//! - Authenticates via Biscuit token (passed as password or connection param)
//! - Parses SQL, injects RLS predicates via `cori-rls`
//! - Forwards queries to upstream Postgres
//! - Audits all queries via `cori-audit`
//!
//! ## Architecture
//!
//! ```text
//! AI Agent / App
//!       │
//!       │ Postgres wire protocol + Biscuit token
//!       ▼
//! ┌─────────────────┐
//! │  Cori Proxy     │
//! │  1. Verify token│  ← cori-biscuit
//! │  2. Parse SQL   │  ← cori-rls
//! │  3. Inject RLS  │  ← cori-rls
//! │  4. Forward     │
//! │  5. Audit log   │  ← cori-audit
//! └────────┬────────┘
//!          │
//!          ▼
//!    Upstream Postgres
//! ```
//!
//! ## Usage
//!
//! ```no_run
//! use cori_core::config::{ProxyConfig, TenancyConfig, UpstreamConfig};
//! use cori_proxy::CoriProxy;
//! use cori_biscuit::{KeyPair, TokenVerifier};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let keypair = KeyPair::generate()?;
//!     let verifier = TokenVerifier::new(keypair.public_key());
//!     let config = ProxyConfig::default();
//!     let upstream = UpstreamConfig::default();
//!     let tenancy_config = TenancyConfig::default();
//!     let proxy = CoriProxy::new(config, upstream, verifier, tenancy_config, None);
//!     proxy.run().await?;
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod handler;
pub mod proxy;

pub use error::ProxyError;
pub use handler::{BiscuitAuthStartupHandler, CoriQueryHandler, CoriServerHandlers, QueryResult, SessionContext};
pub use proxy::{CoriProxy, RolePermissions};


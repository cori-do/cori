// SPDX-License-Identifier: Apache-2.0
//! Modules: what a binary built on Cori adds to it without changing it. A module brings its tables, its routes and its
//! hooks on the workers' steps; such a binary passes its modules to `cori_cli::run` (the core's own `cori` passes
//! none), which loads them after the core's own (`cori_cli::core_modules`: Vault). A module's routes sit behind the same
//! bearer guard as the core's (agent sessions and app fronts reach none of them), with the same body limit, CORS and
//! tracing, and its handlers check their caller with the functions re-exported here, so that every refusal reads the
//! same. Its handlers take the state of their request's realm (`InRealm`): the router's own state (`Global`) is outside
//! any. A module names a capability, never a product or a customer.
use crate::api::Global;
use axum::Router;
use std::sync::Arc;

pub use crate::api::{ApiError, AppRecord, Caller, InRealm, Power, require_builder, require_human, require_power};
pub use crate::auth::Principal;
pub use crate::realm::{Admitted, Realm, RealmSettings, Tenancy, TenancyFuture};
pub use crate::secrets::{KeyFuture, KeyWrapper, SecretScope, Wrapped};

pub trait Module: Send + Sync + 'static {
    /// A short name, for the logs and `cori --version` (`llm-ops`).
    fn id(&self) -> &'static str;
    /// Statements run after the platform schema's, under the same lock, at every start: `CREATE TABLE IF NOT EXISTS …`,
    /// Postgres, like the core's.
    fn schema(&self) -> &'static [&'static str] { &[] }
    /// Routes merged into the server's; one the core already has makes the server panic at start.
    fn routes(&self) -> Router<Global> { Router::new() }
    /// Binding modes besides `primary` that `POST /v1/apps/{app}/llm/bindings` accepts, for the module's `llm` step hook to
    /// read; with any, `off` removes a step's bindings in those modes.
    fn binding_modes(&self) -> &'static [&'static str] { &[] }
    /// What the module adds to the `llm` steps of the workers this binary runs (`cori work`, `cori up`).
    fn llm_step_hook(&self) -> Option<Arc<dyn cori_engine::hooks::LlmStepHook>> { None }
    /// The organizations this binary keeps apart (`crate::realm`): at most one module provides it. Asked once, when the
    /// server starts: an error (its configuration) stops it.
    fn tenancy(&self) -> anyhow::Result<Option<Arc<dyn Tenancy>>> { Ok(None) }
    /// What wraps the data keys of the secrets of the scopes it covers, instead of the master key (`crate::secrets`): an
    /// external key service. At most one module provides it; asked once, when the server starts: an error stops it.
    fn key_wrapper(&self) -> anyhow::Result<Option<Arc<dyn KeyWrapper>>> { Ok(None) }
    /// The module's settings that must not reach a network, as `ServerConfig::production_problems` reports the core's.
    fn production_problems(&self) -> Vec<String> { Vec::new() }
    /// What the module checks of the deployment for `cori preflight` (`crate::preflight`): its outside services reached and
    /// answering as configured.
    fn preflight<'a>(&'a self) -> futures::future::BoxFuture<'a, Vec<crate::preflight::Check>> { Box::pin(async { Vec::new() }) }
}

/// The binary's modules, as `ServerConfig` carries them.
#[derive(Clone, Default)]
pub struct Modules(pub Vec<Arc<dyn Module>>);

impl std::fmt::Debug for Modules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_list().entries(self.0.iter().map(|m| m.id())).finish() }
}

impl Modules {
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Module>> { self.0.iter() }

    pub fn ids(&self) -> Vec<&'static str> { self.0.iter().map(|m| m.id()).collect() }

    /// Every module's schema statements, module after module.
    pub fn schema(&self) -> Vec<&'static str> { self.0.iter().flat_map(|m| m.schema().iter().copied()).collect() }

    /// The binding modes the modules add, each once.
    pub fn binding_modes(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for m in self.0.iter().flat_map(|m| m.binding_modes().iter().copied()) { if !out.contains(&m) { out.push(m); } }
        out
    }

    /// Their `llm` step hooks, for `cori_engine::hooks::install`.
    pub fn llm_step_hooks(&self) -> Vec<Arc<dyn cori_engine::hooks::LlmStepHook>> { self.0.iter().filter_map(|m| m.llm_step_hook()).collect() }

    /// The tenancy of the one module providing it; two make the server refuse to start.
    pub fn tenancy(&self) -> anyhow::Result<Option<Arc<dyn Tenancy>>> {
        let mut found = Vec::new();
        for m in self.0.iter() { if let Some(t) = m.tenancy().map_err(|e| anyhow::anyhow!("module `{}`: {e:#}", m.id()))? { found.push((m.id(), t)); } }
        if let [(a, _), (b, _), ..] = found.as_slice() { anyhow::bail!("two modules keep organizations apart (`{a}`, `{b}`): a binary has one tenancy"); }
        Ok(found.pop().map(|(_, t)| t))
    }

    /// The key wrapper of the one module providing it; two make the server refuse to start.
    pub fn key_wrapper(&self) -> anyhow::Result<Option<Arc<dyn KeyWrapper>>> {
        let mut found = Vec::new();
        for m in self.0.iter() { if let Some(w) = m.key_wrapper().map_err(|e| anyhow::anyhow!("module `{}`: {e:#}", m.id()))? { found.push((m.id(), w)); } }
        if let [(a, _), (b, _), ..] = found.as_slice() { anyhow::bail!("two modules wrap the secrets' keys (`{a}`, `{b}`): a binary has one key wrapper"); }
        Ok(found.pop().map(|(_, w)| w))
    }

    /// Every module's preflight checks, module after module.
    pub async fn preflight(&self) -> Vec<crate::preflight::Check> {
        let mut out = Vec::new();
        for m in self.0.iter() { out.extend(m.preflight().await); }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A; struct B;
    impl Module for A { fn id(&self) -> &'static str { "a" } fn schema(&self) -> &'static [&'static str] { &["CREATE TABLE IF NOT EXISTS a (id TEXT PRIMARY KEY)"] } fn binding_modes(&self) -> &'static [&'static str] { &["shadow"] } }
    impl Module for B { fn id(&self) -> &'static str { "b" } fn binding_modes(&self) -> &'static [&'static str] { &["shadow", "canary"] } }

    #[test]
    fn modules_add_up() {
        let m = Modules(vec![Arc::new(A), Arc::new(B)]);
        assert_eq!(m.ids(), ["a", "b"]);
        assert_eq!(m.binding_modes(), ["shadow", "canary"]);
        assert_eq!(m.schema().len(), 1);
        assert!(m.llm_step_hooks().is_empty());
        assert!(m.tenancy().unwrap().is_none());
        assert_eq!(format!("{m:?}"), r#"["a", "b"]"#);
        assert!(Modules::default().binding_modes().is_empty());
    }
}

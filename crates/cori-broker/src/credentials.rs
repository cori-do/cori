//! Per-user credential resolution.
//!
//! Phase 4 introduces this module as a thin funnel over the existing
//! environment-variable based credential resolution. Phase 5 will add
//! OAuth tokens keyed by `user_id` (see the redesign migration plan).
//!
//! The contract is intentionally minimal: callers pass the `user_id`
//! that originated the run and the capability id they need creds for
//! (`"openai"`, `"github"`, `"gws"`, …), and get back an opaque bundle
//! they can hand to the underlying provider client.
//!
//! Today the lookup ignores `user_id` entirely — service workers and
//! solo runs both use process-wide env vars. The signature exists so
//! the broker call sites can be migrated incrementally; Phase 5 swaps
//! the implementation without touching callers.

use crate::llm::LlmCredentials;

/// Opaque credential bundle. Phase 4 stores only LLM keys; Phase 5
/// will add OAuth tokens and per-capability secrets.
#[derive(Debug, Clone, Default)]
pub struct UserCredentials {
    pub llm: LlmCredentials,
}

/// Resolve credentials for `user_id` × `capability_id`.
///
/// Phase 4: ignores both arguments and returns the process-wide env
/// var resolution. The arguments are part of the signature so call
/// sites can be migrated to the per-user shape now.
pub fn for_user(_user_id: &str, _capability_id: &str) -> UserCredentials {
    UserCredentials {
        llm: LlmCredentials::from_env(),
    }
}

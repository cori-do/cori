//! Token generation. 32 random bytes → base64url-encoded string.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

/// Master token printed in the `cori work` startup URL. Compared
/// constant-time by the session-exchange handler.
pub fn generate_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Session cookie value — issued in exchange for a valid master token.
/// Distinct from the master token so the cookie can be rotated without
/// breaking `Authorization: Bearer` reuse on state-changing endpoints
/// (Phase 3+).
pub fn generate_session_value() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

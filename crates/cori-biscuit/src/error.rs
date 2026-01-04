//! Error types for the Biscuit crate.

use thiserror::Error;

/// Errors that can occur during Biscuit token operations.
#[derive(Debug, Error)]
pub enum BiscuitError {
    /// Failed to generate keypair.
    #[error("failed to generate keypair: {0}")]
    KeyGenerationFailed(String),

    /// Failed to parse private key.
    #[error("failed to parse private key: {0}")]
    InvalidPrivateKey(String),

    /// Failed to parse public key.
    #[error("failed to parse public key: {0}")]
    InvalidPublicKey(String),

    /// Failed to create token.
    #[error("failed to create token: {0}")]
    TokenCreationFailed(String),

    /// Failed to parse token.
    #[error("failed to parse token: {0}")]
    TokenParseFailed(String),

    /// Token verification failed.
    #[error("token verification failed: {0}")]
    VerificationFailed(String),

    /// Token has expired.
    #[error("token has expired at {expired_at}")]
    TokenExpired { expired_at: String },

    /// Token is missing required claim.
    #[error("token missing required claim: {claim}")]
    MissingClaim { claim: String },

    /// Token attenuation failed.
    #[error("failed to attenuate token: {0}")]
    AttenuationFailed(String),

    /// Failed to serialize/deserialize token.
    #[error("token serialization error: {0}")]
    SerializationError(String),

    /// IO error (reading/writing keys).
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}


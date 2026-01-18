//! # cori-biscuit
//!
//! Biscuit token handling for Cori MCP Server.
//!
//! This crate provides functionality for:
//! - Generating Ed25519 keypairs for token signing
//! - Minting role tokens with permissions
//! - Attenuating tokens with tenant and expiration claims
//! - Verifying and extracting claims from tokens
//!
//! ## Two-Level Token Model
//!
//! Cori uses a **role token + attenuation** model:
//!
//! | Token Type | Created By | Contains | Lifetime |
//! |------------|------------|----------|----------|
//! | **Role Token** | Admin (dashboard/CLI) | Role permissions, table access | Long-lived |
//! | **Agent Token** | Attenuated from role token | Role + tenant + expiration | Short-lived |
//!
//! ## Why Biscuit?
//!
//! - **Decentralized verification**: Tokens are self-contained
//! - **Attenuable**: Tokens can be restricted further (time limits, tenant scope)
//! - **Cryptographically secure**: Ed25519 signatures, no forgery
//! - **Perfect for multi-tenant AI**: Mint a token per (tenant, role) pair

pub mod claims;
pub mod error;
pub mod keys;
pub mod token;

pub use biscuit_auth::PublicKey;
pub use claims::{AgentClaims, RoleClaims};
pub use error::BiscuitError;
pub use keys::KeyPair;
pub use token::{TokenBuilder, TokenInfo, TokenVerifier, VerifiedToken, inspect_token_unverified};

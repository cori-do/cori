//! Token creation and verification.

use crate::claims::RoleClaims;
use crate::error::BiscuitError;
use crate::keys::KeyPair;
use biscuit_auth::builder::AuthorizerBuilder;
use biscuit_auth::macros::{check, fact};
use biscuit_auth::{builder::BlockBuilder, Biscuit, PublicKey};
use chrono::{Duration, Utc};

/// Builder for creating Biscuit tokens.
pub struct TokenBuilder {
    keypair: KeyPair,
}

impl TokenBuilder {
    /// Create a new token builder with the given keypair.
    pub fn new(keypair: KeyPair) -> Self {
        Self { keypair }
    }

    /// Mint a role token (base token without tenant restriction).
    pub fn mint_role_token(&self, claims: &RoleClaims) -> Result<String, BiscuitError> {
        let mut builder = Biscuit::builder();

        // Add role fact
        builder = builder
            .fact(fact!("role({role})", role = claims.role.clone()))
            .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;

        // Add table access facts
        for (table, perms) in &claims.tables {
            builder = builder
                .fact(fact!("table_access({table})", table = table.clone()))
                .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;

            // Add readable columns
            for column in &perms.readable {
                builder = builder
                    .fact(fact!(
                        "readable({table}, {column})",
                        table = table.clone(),
                        column = column.clone()
                    ))
                    .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;
            }

            // Add editable columns
            for column in perms.editable.keys() {
                builder = builder
                    .fact(fact!(
                        "editable({table}, {column})",
                        table = table.clone(),
                        column = column.clone()
                    ))
                    .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;
            }
        }

        // Add blocked tables
        for table in &claims.blocked_tables {
            builder = builder
                .fact(fact!("blocked_table({table})", table = table.clone()))
                .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;
        }

        // Add max_rows if set
        if let Some(max_rows) = claims.max_rows_per_query {
            builder = builder
                .fact(fact!("max_rows({max_rows})", max_rows = max_rows as i64))
                .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;
        }

        // Add minted_at timestamp
        let now = Utc::now().timestamp();
        builder = builder
            .fact(fact!("minted_at({timestamp})", timestamp = now))
            .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;

        let biscuit = builder
            .build(self.keypair.inner())
            .map_err(|e| BiscuitError::TokenCreationFailed(e.to_string()))?;

        Ok(biscuit
            .to_base64()
            .map_err(|e| BiscuitError::SerializationError(e.to_string()))?)
    }

    /// Attenuate a role token with tenant and expiration.
    pub fn attenuate(
        &self,
        base_token: &str,
        tenant: &str,
        expires_in: Option<Duration>,
        source: Option<&str>,
    ) -> Result<String, BiscuitError> {
        let biscuit = Biscuit::from_base64(base_token, self.keypair.public_key())
            .map_err(|e| BiscuitError::TokenParseFailed(e.to_string()))?;

        let mut block = BlockBuilder::new();

        // Add tenant restriction
        block = block
            .fact(fact!("tenant({tenant})", tenant = tenant.to_string()))
            .map_err(|e| BiscuitError::AttenuationFailed(e.to_string()))?;

        // Add expiration if specified
        if let Some(duration) = expires_in {
            let expires_at = Utc::now() + duration;
            block = block
                .check(check!(
                    "check if time($time), $time < {expires_at}",
                    expires_at = expires_at.timestamp()
                ))
                .map_err(|e| BiscuitError::AttenuationFailed(e.to_string()))?;
        }

        // Add source
        if let Some(src) = source {
            block = block
                .fact(fact!("source({source})", source = src.to_string()))
                .map_err(|e| BiscuitError::AttenuationFailed(e.to_string()))?;
        }

        // Add attenuation timestamp
        let now = Utc::now().timestamp();
        block = block
            .fact(fact!("attenuated_at({timestamp})", timestamp = now))
            .map_err(|e| BiscuitError::AttenuationFailed(e.to_string()))?;

        let attenuated = biscuit
            .append(block)
            .map_err(|e| BiscuitError::AttenuationFailed(e.to_string()))?;

        Ok(attenuated
            .to_base64()
            .map_err(|e| BiscuitError::SerializationError(e.to_string()))?)
    }
}

/// Verifier for Biscuit tokens.
pub struct TokenVerifier {
    public_key: PublicKey,
}

impl TokenVerifier {
    /// Create a new token verifier with the given public key.
    pub fn new(public_key: PublicKey) -> Self {
        Self { public_key }
    }

    /// Verify a token and extract claims.
    pub fn verify(&self, token: &str) -> Result<VerifiedToken, BiscuitError> {
        let biscuit = Biscuit::from_base64(token, self.public_key.clone())
            .map_err(|e| BiscuitError::TokenParseFailed(e.to_string()))?;

        // Create authorizer with policies using AuthorizerBuilder
        let now = Utc::now().timestamp();
        let mut authorizer = AuthorizerBuilder::new()
            .code(format!(
                r#"
                time({now});
                allow if true;
                "#
            ))
            .map_err(|e| BiscuitError::VerificationFailed(e.to_string()))?
            .build(&biscuit)
            .map_err(|e| BiscuitError::VerificationFailed(e.to_string()))?;

        // Run authorization
        authorizer
            .authorize()
            .map_err(|e| BiscuitError::VerificationFailed(e.to_string()))?;

        // Extract role from authority block (block 0) via authorizer query
        let role = self.extract_string_fact(&mut authorizer, "role")?;

        // Extract tenant from attenuation blocks by parsing block sources directly
        // In Biscuit, facts in attenuation blocks are not visible to the authorizer's query,
        // so we need to parse the blocks directly
        let tenant = self.extract_tenant_from_blocks(&biscuit);
        let block_count_value = biscuit.block_count();

        Ok(VerifiedToken {
            role,
            tenant,
            block_count_value,
        })
    }

    /// Extract tenant from attenuation blocks by parsing block source.
    /// Attenuation blocks (block 1+) contain facts that are scoped to that block,
    /// so we need to parse them directly to extract the tenant.
    fn extract_tenant_from_blocks(&self, biscuit: &Biscuit) -> Option<String> {
        // Iterate through attenuation blocks (starting from block 1)
        for block_idx in 1..biscuit.block_count() {
            if let Ok(block_source) = biscuit.print_block_source(block_idx) {
                // Parse the block source to find tenant("value") pattern
                if let Some(tenant) = Self::parse_tenant_from_source(&block_source) {
                    return Some(tenant);
                }
            }
        }
        None
    }

    /// Parse a block source string to extract tenant value.
    /// The block source format includes facts like: tenant("client_a");
    fn parse_tenant_from_source(source: &str) -> Option<String> {
        // Look for tenant("...") pattern in the block source
        // The format is: tenant("value");
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("tenant(") {
                // Extract the value between tenant(" and ")
                if let Some(start) = trimmed.find("tenant(\"") {
                    let value_start = start + 8; // length of 'tenant("'
                    if let Some(end) = trimmed[value_start..].find("\")") {
                        return Some(trimmed[value_start..value_start + end].to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_string_fact(
        &self,
        authorizer: &mut biscuit_auth::Authorizer,
        name: &str,
    ) -> Result<String, BiscuitError> {
        // Use the query method with a rule string
        let rule_str = format!("data($x) <- {}($x)", name);
        let rule: biscuit_auth::builder::Rule = rule_str
            .parse()
            .map_err(|e: biscuit_auth::error::Token| BiscuitError::VerificationFailed(e.to_string()))?;

        let results: Vec<(String,)> = authorizer
            .query(rule)
            .map_err(|e| BiscuitError::VerificationFailed(e.to_string()))?;

        results
            .into_iter()
            .next()
            .map(|(s,)| s)
            .ok_or_else(|| BiscuitError::MissingClaim {
                claim: name.to_string(),
            })
    }
}

/// A verified token with extracted claims.
#[derive(Debug)]
pub struct VerifiedToken {
    /// The role from the token.
    pub role: String,
    /// The tenant (if attenuated).
    pub tenant: Option<String>,
    /// Block count from the original token.
    block_count_value: usize,
}

impl Clone for VerifiedToken {
    fn clone(&self) -> Self {
        Self {
            role: self.role.clone(),
            tenant: self.tenant.clone(),
            block_count_value: self.block_count_value,
        }
    }
}

impl VerifiedToken {
    /// Check if this is an attenuated (agent) token.
    pub fn is_attenuated(&self) -> bool {
        self.tenant.is_some()
    }

    /// Get the number of blocks in the token.
    pub fn block_count(&self) -> usize {
        self.block_count_value
    }
}

/// Inspect a token without verification (for debugging).
pub fn inspect_token_unverified(token: &str) -> Result<TokenInfo, BiscuitError> {
    // Use UnverifiedBiscuit for inspection
    let biscuit = biscuit_auth::UnverifiedBiscuit::from_base64(token)
        .map_err(|e| BiscuitError::TokenParseFailed(e.to_string()))?;

    // Get a string representation by formatting the blocks
    let print = format!("Biscuit with {} blocks", biscuit.block_count());

    Ok(TokenInfo {
        block_count: biscuit.block_count(),
        print,
    })
}

/// Information about a token (for inspection).
pub struct TokenInfo {
    /// Number of blocks in the token.
    pub block_count: usize,
    /// Human-readable representation.
    pub print: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mint_and_verify_role_token() {
        let keypair = KeyPair::generate().unwrap();
        let builder = TokenBuilder::new(keypair.clone());

        let claims = RoleClaims::new("support_agent")
            .add_readable_table("customers", vec!["id".into(), "name".into()]);

        let token = builder.mint_role_token(&claims).unwrap();
        assert!(!token.is_empty());

        let verifier = TokenVerifier::new(keypair.public_key());
        let verified = verifier.verify(&token).unwrap();
        assert_eq!(verified.role, "support_agent");
        assert!(verified.tenant.is_none());
    }

    #[test]
    fn test_attenuate_token() {
        let keypair = KeyPair::generate().unwrap();
        let builder = TokenBuilder::new(keypair.clone());

        let claims = RoleClaims::new("agent");
        let role_token = builder.mint_role_token(&claims).unwrap();

        let agent_token = builder
            .attenuate(
                &role_token,
                "client_a",
                Some(Duration::hours(24)),
                Some("cli"),
            )
            .unwrap();

        let verifier = TokenVerifier::new(keypair.public_key());
        let verified = verifier.verify(&agent_token).unwrap();

        assert_eq!(verified.role, "agent");
        assert_eq!(verified.tenant, Some("client_a".to_string()));
        assert!(verified.is_attenuated());
    }
}

//! Token management commands.
//!
//! `cori token mint` - Mint a new role token.
//! `cori token attenuate` - Attenuate a role token with tenant and expiration.
//! `cori token inspect` - Inspect a token's contents.
//! `cori token verify` - Verify a token is valid.

use anyhow::Context;
use cori_biscuit::{inspect_token_unverified, KeyPair, RoleClaims, TokenBuilder, TokenVerifier};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolve a private key from either a file path or a hex-encoded string.
///
/// The key string can be:
/// - A path to a file containing a hex-encoded private key
/// - A hex-encoded private key directly (e.g., from BISCUIT_PRIVATE_KEY env var)
fn resolve_private_key(key: Option<String>) -> anyhow::Result<KeyPair> {
    let key_str = key.context(
        "Private key not provided. Either pass --key <path> or set BISCUIT_PRIVATE_KEY env var",
    )?;

    // If it looks like a file path and the file exists, load from file
    let path = Path::new(&key_str);
    if path.exists() {
        return KeyPair::load_from_file(path)
            .with_context(|| format!("Failed to load private key from file: {}", path.display()));
    }

    // Otherwise, treat it as a hex-encoded private key
    KeyPair::from_private_key_hex(key_str.trim())
        .context("Failed to parse private key. Expected hex-encoded Ed25519 private key")
}

/// Resolve a public key from either a file path or a hex-encoded string.
///
/// The key string can be:
/// - A path to a file containing a hex-encoded public key
/// - A hex-encoded public key directly (e.g., from BISCUIT_PUBLIC_KEY env var)
fn resolve_public_key(key: Option<String>) -> anyhow::Result<cori_biscuit::PublicKey> {
    let key_str = key.context(
        "Public key not provided. Either pass --key <path> or set BISCUIT_PUBLIC_KEY env var",
    )?;

    // If it looks like a file path and the file exists, load from file
    let path = Path::new(&key_str);
    if path.exists() {
        return cori_biscuit::keys::load_public_key_file(path)
            .with_context(|| format!("Failed to load public key from file: {}", path.display()));
    }

    // Otherwise, treat it as a hex-encoded public key
    cori_biscuit::keys::load_public_key_hex(key_str.trim())
        .context("Failed to parse public key. Expected hex-encoded Ed25519 public key")
}

/// Parse a duration string like "24h", "7d", "1h30m" into chrono::Duration.
fn parse_duration(s: &str) -> anyhow::Result<chrono::Duration> {
    let s = s.trim().to_lowercase();
    
    // Simple parsing for common formats
    if let Some(hours) = s.strip_suffix('h') {
        let h: i64 = hours.parse()?;
        return Ok(chrono::Duration::hours(h));
    }
    if let Some(days) = s.strip_suffix('d') {
        let d: i64 = days.parse()?;
        return Ok(chrono::Duration::days(d));
    }
    if let Some(minutes) = s.strip_suffix('m') {
        let m: i64 = minutes.parse()?;
        return Ok(chrono::Duration::minutes(m));
    }
    if let Some(seconds) = s.strip_suffix('s') {
        let sec: i64 = seconds.parse()?;
        return Ok(chrono::Duration::seconds(sec));
    }

    // Try parsing as hours if no suffix
    let h: i64 = s.parse()?;
    Ok(chrono::Duration::hours(h))
}

/// Mint a new role token (or agent token if tenant is specified).
pub fn mint(
    private_key: Option<String>,
    role: String,
    tenant: Option<String>,
    expires: Option<String>,
    tables: Vec<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Load private key from file or env var
    let keypair = resolve_private_key(private_key)?;
    let builder = TokenBuilder::new(keypair);

    // Build role claims
    let mut claims = RoleClaims::new(&role);
    
    // Add tables (format: "table:col1,col2" or just "table")
    for table_spec in tables {
        let parts: Vec<&str> = table_spec.splitn(2, ':').collect();
        let table_name = parts[0].to_string();
        let columns = if parts.len() > 1 {
            parts[1].split(',').map(|s| s.trim().to_string()).collect()
        } else {
            vec!["*".to_string()]
        };
        claims = claims.add_readable_table(table_name, columns);
    }

    // Mint the role token
    let role_token = builder.mint_role_token(&claims)?;

    // If tenant is specified, attenuate immediately
    let final_token = if let Some(tenant_id) = &tenant {
        let duration = expires.as_ref().map(|e| parse_duration(e)).transpose()?;
        builder.attenuate(&role_token, tenant_id, duration, Some("cli"))?
    } else {
        role_token
    };

    // Output the token
    if let Some(output_path) = output {
        fs::write(&output_path, &final_token)?;
        println!("✔ Token written to: {}", output_path.display());
        
        if tenant.is_some() {
            println!("  Type: Agent token (tenant-restricted)");
        } else {
            println!("  Type: Role token (base)");
        }
        println!("  Role: {}", role);
        if let Some(t) = &tenant {
            println!("  Tenant: {}", t);
        }
        if let Some(e) = &expires {
            println!("  Expires: {}", e);
        }
    } else {
        // Print to stdout
        println!("{}", final_token);
    }

    Ok(())
}

/// Attenuate an existing role token with tenant and expiration.
pub fn attenuate(
    private_key: Option<String>,
    base_token_file: PathBuf,
    tenant: String,
    expires: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Load private key from file or env var
    let keypair = resolve_private_key(private_key)?;
    let builder = TokenBuilder::new(keypair);

    // Load base token
    let base_token = fs::read_to_string(&base_token_file)?.trim().to_string();

    // Parse duration
    let duration = expires.as_ref().map(|e| parse_duration(e)).transpose()?;

    // Attenuate
    let agent_token = builder.attenuate(&base_token, &tenant, duration, Some("cli"))?;

    // Output the token
    if let Some(output_path) = output {
        fs::write(&output_path, &agent_token)?;
        println!("✔ Attenuated token written to: {}", output_path.display());
        println!("  Tenant: {}", tenant);
        if let Some(e) = &expires {
            println!("  Expires: {}", e);
        }
    } else {
        println!("{}", agent_token);
    }

    Ok(())
}

/// Inspect a token without verification.
pub fn inspect(token: String) -> anyhow::Result<()> {
    // Try to load from file if it looks like a path
    let token_str = if std::path::Path::new(&token).exists() {
        fs::read_to_string(&token)?.trim().to_string()
    } else {
        token
    };

    let info = inspect_token_unverified(&token_str)?;

    println!("Token Information:");
    println!("  Block count: {}", info.block_count);
    println!();
    println!("{}", info.print);

    Ok(())
}

/// Verify a token is valid.
pub fn verify(public_key: Option<String>, token: String) -> anyhow::Result<()> {
    // Load public key from file or env var
    let public_key = resolve_public_key(public_key)?;
    let verifier = TokenVerifier::new(public_key);

    // Load token from file if it looks like a path
    let token_str = if std::path::Path::new(&token).exists() {
        fs::read_to_string(&token)?.trim().to_string()
    } else {
        token
    };

    // Verify
    match verifier.verify(&token_str) {
        Ok(verified) => {
            println!("✔ Token is valid");
            println!();
            println!("Token Details:");
            println!("  Role: {}", verified.role);
            if let Some(tenant) = &verified.tenant {
                println!("  Tenant: {} (attenuated)", tenant);
                println!("  Type: Agent token");
            } else {
                println!("  Tenant: (none - base role token)");
                println!("  Type: Role token");
            }
            println!("  Block count: {}", verified.block_count());
        }
        Err(e) => {
            println!("✖ Token verification failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("24h").unwrap(), chrono::Duration::hours(24));
        assert_eq!(parse_duration("7d").unwrap(), chrono::Duration::days(7));
        assert_eq!(parse_duration("30m").unwrap(), chrono::Duration::minutes(30));
        assert_eq!(parse_duration("60s").unwrap(), chrono::Duration::seconds(60));
    }

    #[test]
    fn test_mint_role_token() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("private.key");
        let token_path = dir.path().join("token.biscuit");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        fs::write(&key_path, keypair.private_key_hex()).unwrap();

        // Mint token using file path
        mint(
            Some(key_path.to_string_lossy().to_string()),
            "test_role".to_string(),
            None,
            None,
            vec!["users".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        assert!(token_path.exists());
        let token = fs::read_to_string(&token_path).unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_mint_role_token_with_hex_key() {
        let dir = tempdir().unwrap();
        let token_path = dir.path().join("token.biscuit");

        // Generate keypair and use hex directly (simulating env var)
        let keypair = KeyPair::generate().unwrap();
        let private_key_hex = keypair.private_key_hex();

        // Mint token using hex key directly
        mint(
            Some(private_key_hex),
            "test_role".to_string(),
            None,
            None,
            vec!["users".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        assert!(token_path.exists());
        let token = fs::read_to_string(&token_path).unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_mint_agent_token() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("private.key");
        let token_path = dir.path().join("token.biscuit");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        fs::write(&key_path, keypair.private_key_hex()).unwrap();

        // Mint token with tenant
        mint(
            Some(key_path.to_string_lossy().to_string()),
            "test_role".to_string(),
            Some("tenant_123".to_string()),
            Some("24h".to_string()),
            vec!["users:id,name,email".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        // Verify the token using file path
        let public_path = dir.path().join("public.key");
        fs::write(&public_path, keypair.public_key_hex()).unwrap();

        let token = fs::read_to_string(&token_path).unwrap();
        verify(Some(public_path.to_string_lossy().to_string()), token).unwrap();
    }

    #[test]
    fn test_verify_with_hex_key() {
        let dir = tempdir().unwrap();
        let token_path = dir.path().join("token.biscuit");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        let private_key_hex = keypair.private_key_hex();
        let public_key_hex = keypair.public_key_hex();

        // Mint token using hex key
        mint(
            Some(private_key_hex),
            "test_role".to_string(),
            Some("tenant_123".to_string()),
            Some("24h".to_string()),
            vec!["users:id,name,email".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        // Verify the token using hex public key directly
        let token = fs::read_to_string(&token_path).unwrap();
        verify(Some(public_key_hex), token).unwrap();
    }
}


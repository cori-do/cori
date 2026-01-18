//! Token management commands.
//!
//! `cori token mint` - Mint a new role token.
//! `cori token attenuate` - Attenuate a role token with tenant and expiration.
//! `cori token inspect` - Inspect a token's contents, optionally verify with public key.
//!
//! Uses convention over configuration - keys are loaded from cori.yaml or default locations.

use anyhow::Context;
use cori_biscuit::{KeyPair, RoleClaims, TokenBuilder, TokenVerifier, inspect_token_unverified};
use cori_core::config::CoriConfig;
use std::fs;
use std::path::{Path, PathBuf};

/// Load private key from configuration or standard location.
///
/// Resolution order:
/// 1. Explicit --key argument (file path or hex string)
/// 2. BISCUIT_PRIVATE_KEY environment variable
/// 3. biscuit.private_key_file from cori.yaml
/// 4. biscuit.private_key_env from cori.yaml
/// 5. Default location: keys/private.key
fn load_private_key_from_config(
    explicit_key: Option<String>,
    config_path: &Path,
) -> anyhow::Result<KeyPair> {
    let base_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // 1. Try explicit key argument first
    if let Some(key_str) = explicit_key {
        // If it looks like a file path and exists, load from file
        let path = Path::new(&key_str);
        if path.exists() {
            return KeyPair::load_from_file(path)
                .with_context(|| format!("Failed to load private key from file: {}", path.display()));
        }
        // Otherwise, treat it as a hex-encoded private key
        return KeyPair::from_private_key_hex(key_str.trim())
            .context("Failed to parse private key. Expected hex-encoded Ed25519 private key");
    }

    // 2. Try BISCUIT_PRIVATE_KEY environment variable
    if let Ok(key_str) = std::env::var("BISCUIT_PRIVATE_KEY") {
        return KeyPair::from_private_key_hex(key_str.trim())
            .context("Failed to parse BISCUIT_PRIVATE_KEY environment variable");
    }

    // Load config file to check for key configuration
    let config = if config_path.exists() {
        CoriConfig::load_with_context(config_path).ok()
    } else {
        None
    };

    if let Some(ref config) = config {
        // 3. Try from biscuit.private_key_file
        if let Some(ref key_file) = config.biscuit.private_key_file {
            let path = if key_file.is_absolute() {
                key_file.clone()
            } else {
                base_dir.join(key_file)
            };
            if path.exists() {
                return KeyPair::load_from_file(&path)
                    .with_context(|| format!("Failed to load private key from {:?}", path));
            }
        }

        // 4. Try from biscuit.private_key_env
        if let Some(ref env_var) = config.biscuit.private_key_env {
            if let Ok(key_str) = std::env::var(env_var) {
                return KeyPair::from_private_key_hex(key_str.trim())
                    .with_context(|| format!("Failed to parse private key from env var {}", env_var));
            }
        }
    }

    // 5. Try default location: keys/private.key
    let default_path = base_dir.join("keys/private.key");
    if default_path.exists() {
        return KeyPair::load_from_file(&default_path)
            .with_context(|| format!("Failed to load private key from {:?}", default_path));
    }

    anyhow::bail!(
        "No private key found. Either:\n\
         • Pass --key <path> or --key <hex>\n\
         • Set BISCUIT_PRIVATE_KEY environment variable\n\
         • Configure biscuit.private_key_file in cori.yaml\n\
         • Place key in keys/private.key"
    )
}

/// Load public key from configuration or standard location.
///
/// Resolution order:
/// 1. Explicit --key argument (file path or hex string)
/// 2. BISCUIT_PUBLIC_KEY environment variable
/// 3. biscuit.public_key_file from cori.yaml
/// 4. biscuit.public_key_env from cori.yaml
/// 5. Default location: keys/public.key
fn load_public_key_from_config(
    explicit_key: Option<String>,
    config_path: &Path,
) -> anyhow::Result<cori_biscuit::PublicKey> {
    let base_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // 1. Try explicit key argument first
    if let Some(key_str) = explicit_key {
        // If it looks like a file path and exists, load from file
        let path = Path::new(&key_str);
        if path.exists() {
            return cori_biscuit::keys::load_public_key_file(path)
                .with_context(|| format!("Failed to load public key from file: {}", path.display()));
        }
        // Otherwise, treat it as a hex-encoded public key
        return cori_biscuit::keys::load_public_key_hex(key_str.trim())
            .context("Failed to parse public key. Expected hex-encoded Ed25519 public key");
    }

    // 2. Try BISCUIT_PUBLIC_KEY environment variable
    if let Ok(key_str) = std::env::var("BISCUIT_PUBLIC_KEY") {
        return cori_biscuit::keys::load_public_key_hex(key_str.trim())
            .context("Failed to parse BISCUIT_PUBLIC_KEY environment variable");
    }

    // Load config file to check for key configuration
    let config = if config_path.exists() {
        CoriConfig::load_with_context(config_path).ok()
    } else {
        None
    };

    if let Some(ref config) = config {
        // 3. Try from biscuit.public_key_file
        if let Some(ref key_file) = config.biscuit.public_key_file {
            let path = if key_file.is_absolute() {
                key_file.clone()
            } else {
                base_dir.join(key_file)
            };
            if path.exists() {
                return cori_biscuit::keys::load_public_key_file(&path)
                    .with_context(|| format!("Failed to load public key from {:?}", path));
            }
        }

        // 4. Try from biscuit.public_key_env
        if let Some(ref env_var) = config.biscuit.public_key_env {
            if let Ok(key_str) = std::env::var(env_var) {
                return cori_biscuit::keys::load_public_key_hex(key_str.trim())
                    .with_context(|| format!("Failed to parse public key from env var {}", env_var));
            }
        }
    }

    // 5. Try default location: keys/public.key
    let default_path = base_dir.join("keys/public.key");
    if default_path.exists() {
        return cori_biscuit::keys::load_public_key_file(&default_path)
            .with_context(|| format!("Failed to load public key from {:?}", default_path));
    }

    anyhow::bail!(
        "No public key found. Either:\n\
         • Pass --key <path> or --key <hex>\n\
         • Set BISCUIT_PUBLIC_KEY environment variable\n\
         • Configure biscuit.public_key_file in cori.yaml\n\
         • Place key in keys/public.key"
    )
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
    config_path: PathBuf,
    private_key: Option<String>,
    role: String,
    tenant: Option<String>,
    expires: Option<String>,
    tables: Vec<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Load private key from config or explicit argument
    let keypair = load_private_key_from_config(private_key, &config_path)?;
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
    config_path: PathBuf,
    private_key: Option<String>,
    base_token_file: PathBuf,
    tenant: String,
    expires: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Load private key from config or explicit argument
    let keypair = load_private_key_from_config(private_key, &config_path)?;
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

/// Inspect a token, optionally verify with public key.
///
/// Without --key: Shows unverified token contents (block count, facts, checks)
/// With --key or config: Verifies signature and shows verified role/tenant information
pub fn inspect(config_path: PathBuf, token: String, key: Option<String>, verify: bool) -> anyhow::Result<()> {
    // Try to load from file if it looks like a path
    let token_str = if std::path::Path::new(&token).exists() {
        fs::read_to_string(&token)?.trim().to_string()
    } else {
        token
    };

    // Determine if we should verify: either --verify flag or --key explicitly provided
    let should_verify = verify || key.is_some();

    // If verification is requested, try to load the public key
    if should_verify {
        let public_key = load_public_key_from_config(key, &config_path)?;
        let verifier = TokenVerifier::new(public_key);

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
    } else {
        // Without key, show unverified token contents
        let info = inspect_token_unverified(&token_str)?;

        println!("Token Information (unverified):");
        println!("  Block count: {}", info.block_count);
        println!();
        println!("{}", info.print);
        println!();
        println!("💡 Use --verify to verify the token signature (uses key from cori.yaml)");
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
        assert_eq!(
            parse_duration("30m").unwrap(),
            chrono::Duration::minutes(30)
        );
        assert_eq!(
            parse_duration("60s").unwrap(),
            chrono::Duration::seconds(60)
        );
    }

    #[test]
    fn test_mint_role_token() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("private.key");
        let token_path = dir.path().join("token.biscuit");
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        fs::write(&key_path, keypair.private_key_hex()).unwrap();

        // Mint token using file path (no config needed since key is explicit)
        mint(
            config_path,
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
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair and use hex directly (simulating env var)
        let keypair = KeyPair::generate().unwrap();
        let private_key_hex = keypair.private_key_hex();

        // Mint token using hex key directly
        mint(
            config_path,
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
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        fs::write(&key_path, keypair.private_key_hex()).unwrap();

        // Mint token with tenant
        mint(
            config_path.clone(),
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
        // Use inspect with key to verify
        inspect(config_path, token, Some(public_path.to_string_lossy().to_string()), true).unwrap();
    }

    #[test]
    fn test_inspect_with_hex_key() {
        let dir = tempdir().unwrap();
        let token_path = dir.path().join("token.biscuit");
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();
        let private_key_hex = keypair.private_key_hex();
        let public_key_hex = keypair.public_key_hex();

        // Mint token using hex key
        mint(
            config_path.clone(),
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
        // Use inspect with key to verify
        inspect(config_path, token, Some(public_key_hex), true).unwrap();
    }

    #[test]
    fn test_inspect_without_key() {
        let dir = tempdir().unwrap();
        let token_path = dir.path().join("token.biscuit");
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair
        let keypair = KeyPair::generate().unwrap();

        // Mint token
        mint(
            config_path.clone(),
            Some(keypair.private_key_hex()),
            "test_role".to_string(),
            None,
            None,
            vec!["users".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        // Inspect without verification (just show contents)
        let token = fs::read_to_string(&token_path).unwrap();
        inspect(config_path, token, None, false).unwrap();
    }

    #[test]
    fn test_mint_with_convention_over_configuration() {
        let dir = tempdir().unwrap();
        let keys_dir = dir.path().join("keys");
        fs::create_dir_all(&keys_dir).unwrap();
        
        let private_key_path = keys_dir.join("private.key");
        let public_key_path = keys_dir.join("public.key");
        let token_path = dir.path().join("token.biscuit");
        let config_path = dir.path().join("cori.yaml");

        // Generate keypair and save to default locations
        let keypair = KeyPair::generate().unwrap();
        fs::write(&private_key_path, keypair.private_key_hex()).unwrap();
        fs::write(&public_key_path, keypair.public_key_hex()).unwrap();

        // Mint token WITHOUT explicit key - should use keys/private.key
        mint(
            config_path.clone(),
            None, // No explicit key provided
            "test_role".to_string(),
            Some("tenant_xyz".to_string()),
            Some("1h".to_string()),
            vec!["orders".to_string()],
            Some(token_path.clone()),
        )
        .unwrap();

        assert!(token_path.exists());
        let token = fs::read_to_string(&token_path).unwrap();
        assert!(!token.is_empty());

        // Inspect with --verify flag but no explicit key - should use keys/public.key
        inspect(config_path, token, None, true).unwrap();
    }
}

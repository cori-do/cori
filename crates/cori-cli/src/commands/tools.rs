//! Tools introspection commands.
//!
//! `cori tools list` - List available tools for a role/token (offline).
//! `cori tools describe` - Show detailed schema for a specific tool.
//!
//! Uses convention over configuration - all paths derived from cori.yaml location.
//! Uses shared tool_generation module for consistency with MCP server and dashboard.

use anyhow::{Context, Result};
use cori_biscuit::{TokenVerifier, keys::load_public_key_file};
use cori_core::config::CoriConfig;
use std::path::PathBuf;

/// Generate tools for a role using the shared tool_generation module.
/// This ensures CLI, MCP server, and dashboard all use identical logic.
fn generate_tools_for_role(
    config: &CoriConfig,
    role_name: &str,
) -> Result<Vec<cori_mcp::ToolDefinition>> {
    cori_mcp::tool_generation::generate_tools_for_role(config, role_name)
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// List tools available for a role or token.
pub fn list(
    config_path: PathBuf,
    role: Option<String>,
    token: Option<PathBuf>,
    verbose: bool,
) -> Result<()> {
    // Load configuration with all context (roles, schema, etc.)
    let config = CoriConfig::load_with_context(&config_path)
        .with_context(|| format!("Failed to load configuration from {:?}", config_path))?;

    // Determine role either from --role flag or from token
    let (role_name, tenant) = if let Some(token_path) = token {
        // Load and verify token to get role
        let public_key = load_public_key_from_config(&config, &config_path)?;
        let token_str = std::fs::read_to_string(&token_path)
            .with_context(|| format!("Failed to read token file: {:?}", token_path))?
            .trim()
            .to_string();

        let verifier = TokenVerifier::new(public_key);
        let verified = verifier
            .verify(&token_str)
            .context("Token verification failed")?;

        (verified.role.clone(), verified.tenant.clone())
    } else if let Some(name) = role {
        (name, None)
    } else {
        anyhow::bail!("Either --role or --token is required");
    };

    // Print header
    println!("\n🔑 Role Information:");
    println!("   Role: {}", role_name);
    if let Some(t) = &tenant {
        println!("   Tenant: {}", t);
    }

    // Generate tools using shared logic (same as MCP server and dashboard)
    let tools = generate_tools_for_role(&config, &role_name)?;

    // Print available tools
    println!("\n🔧 Available Tools ({}):", tools.len());

    for tool in &tools {
        let annotations = tool.annotations.as_ref();
        let read_only = annotations.is_some_and(|a| a.read_only == Some(true));
        let requires_approval = annotations.is_some_and(|a| a.requires_approval == Some(true));
        let dry_run = annotations.is_some_and(|a| a.dry_run_supported == Some(true));

        let mut badges = Vec::new();
        if read_only {
            badges.push("read");
        } else {
            badges.push("write");
        }
        if requires_approval {
            badges.push("approval");
        }
        if dry_run {
            badges.push("dry-run");
        }

        println!("   • {} ({})", tool.name, badges.join(", "));

        if let Some(desc) = &tool.description {
            println!("     {}", desc);
        }

        if verbose {
            println!(
                "     Schema: {}",
                serde_json::to_string_pretty(&tool.input_schema)?
            );
        }
    }

    // Print table access summary
    if let Some(role_config) = config.get_role(&role_name) {
        println!("\n📊 Table Access:");
        for (table, perms) in &role_config.tables {
            let can_read = !perms.readable.is_empty();
            let can_write = !perms.creatable.is_empty() || !perms.updatable.is_empty();

            let mut access = Vec::new();
            if can_read {
                access.push("read");
            }
            if can_write {
                access.push("write");
            }

            if access.is_empty() {
                access.push("none");
            }

            println!("   • {} ({})", table, access.join(", "));
        }
    }

    println!();

    Ok(())
}

/// Load public key from configuration or standard location.
fn load_public_key_from_config(
    config: &CoriConfig,
    config_path: &PathBuf,
) -> Result<cori_biscuit::PublicKey> {
    let base_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Try from biscuit config
    if let Some(ref key_file) = config.biscuit.public_key_file {
        let path = if key_file.is_absolute() {
            key_file.clone()
        } else {
            base_dir.join(key_file)
        };
        if path.exists() {
            return load_public_key_file(&path)
                .with_context(|| format!("Failed to load public key from {:?}", path));
        }
    }

    // Try from environment variable
    if let Some(ref env_var) = config.biscuit.public_key_env
        && let Ok(key_str) = std::env::var(env_var) {
            return cori_biscuit::keys::load_public_key_hex(&key_str)
                .with_context(|| format!("Failed to parse public key from env var {}", env_var));
        }

    // Try default location
    let default_path = base_dir.join("keys/public.key");
    if default_path.exists() {
        return load_public_key_file(&default_path)
            .with_context(|| format!("Failed to load public key from {:?}", default_path));
    }

    anyhow::bail!(
        "No public key found. Configure biscuit.public_key_file in cori.yaml or place key in keys/public.key"
    )
}
/// Show detailed schema for a specific tool.
pub fn describe(config_path: PathBuf, tool_name: String, role: String) -> Result<()> {
    // Load configuration with all context
    let config = CoriConfig::load_with_context(&config_path)
        .with_context(|| format!("Failed to load configuration from {:?}", config_path))?;

    // Generate tools using shared logic (same as MCP server and dashboard)
    let tools = generate_tools_for_role(&config, &role)?;

    // Find the specific tool
    let tool = tools
        .iter()
        .find(|t| t.name == tool_name)
        .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found for role '{}'", tool_name, role))?;

    // Print tool details
    println!("\nTool: {}", tool.name);

    if let Some(desc) = &tool.description {
        println!("\nDescription: {}", desc);
    }

    println!("\nInput Schema:");
    println!("{}", serde_json::to_string_pretty(&tool.input_schema)?);

    if let Some(annotations) = &tool.annotations {
        println!("\nAnnotations:");
        if let Some(true) = annotations.read_only {
            println!("  • readOnly: true");
        }
        if let Some(true) = annotations.requires_approval {
            println!("  • requiresApproval: true");
        }
        if let Some(true) = annotations.dry_run_supported {
            println!("  • dryRunSupported: true");
        }
    }

    println!();

    Ok(())
}

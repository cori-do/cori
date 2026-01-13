//! Tools introspection commands.
//!
//! `cori tools list` - List available tools for a role/token (offline).
//! `cori tools describe` - Show detailed schema for a specific tool.

use anyhow::{Context, Result};
use cori_biscuit::{keys::load_public_key_file, TokenVerifier};
use cori_core::config::role_definition::RoleDefinition;
use cori_core::config::McpConfig;
use cori_mcp::McpServer;
use std::path::PathBuf;

/// Resolve a public key from either a file path or a hex-encoded string.
fn resolve_public_key(key_path: Option<PathBuf>) -> Result<cori_biscuit::PublicKey> {
    let path = key_path.context(
        "Public key not provided. Pass --key <path>",
    )?;

    load_public_key_file(&path)
        .with_context(|| format!("Failed to load public key from file: {}", path.display()))
}

/// List tools available for a role or token.
pub fn list(
    role: Option<String>,
    token: Option<PathBuf>,
    key: Option<PathBuf>,
    roles_dir: PathBuf,
    verbose: bool,
) -> Result<()> {
    // Determine role either from --role flag or from token
    let (role_name, tenant) = if let Some(token_path) = token {
        // Load and verify token to get role
        let public_key = resolve_public_key(key)?;
        let token_str = std::fs::read_to_string(&token_path)
            .with_context(|| format!("Failed to read token file: {:?}", token_path))?
            .trim()
            .to_string();

        let verifier = TokenVerifier::new(public_key);
        let verified = verifier.verify(&token_str)
            .context("Token verification failed")?;

        (verified.role.clone(), verified.tenant.clone())
    } else if let Some(name) = role {
        (name, None)
    } else {
        anyhow::bail!("Either --role or --token is required");
    };

    // Load role configuration
    let role_path = roles_dir.join(format!("{}.yaml", role_name));
    let role_config = if role_path.exists() {
        RoleDefinition::from_file(&role_path)
            .with_context(|| format!("Failed to load role config: {:?}", role_path))?
    } else {
        anyhow::bail!(
            "No role configuration file found at {:?} for role '{}'",
            role_path,
            role_name
        );
    };

    // Print header
    println!("\n🔑 Role Information:");
    println!("   Role: {}", role_name);
    if let Some(t) = &tenant {
        println!("   Tenant: {}", t);
    }

    // Create server and generate tools
    let mcp_config = McpConfig::default();
    let mut server = McpServer::new(mcp_config).with_role(role_config.clone());

    // Generate tools
    server.generate_tools();

    // Print available tools
    let tools = server.tools_mut().list();
    println!("\n🔧 Available Tools ({}):", tools.len());

    for tool in tools {
        let annotations = tool.annotations.as_ref();
        let read_only = annotations.map_or(false, |a| a.read_only == Some(true));
        let requires_approval = annotations.map_or(false, |a| a.requires_approval == Some(true));
        let dry_run = annotations.map_or(false, |a| a.dry_run_supported == Some(true));

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

    println!();

    Ok(())
}

/// Show detailed schema for a specific tool.
pub fn describe(tool_name: String, role: String, roles_dir: PathBuf) -> Result<()> {
    // Load role configuration
    let role_path = roles_dir.join(format!("{}.yaml", role));
    let role_config = if role_path.exists() {
        RoleDefinition::from_file(&role_path)
            .with_context(|| format!("Failed to load role config: {:?}", role_path))?
    } else {
        anyhow::bail!(
            "No role configuration file found at {:?} for role '{}'",
            role_path,
            role
        );
    };

    // Create server and generate tools
    let mcp_config = McpConfig::default();
    let mut server = McpServer::new(mcp_config).with_role(role_config);

    // Generate tools
    server.generate_tools();

    // Find the specific tool
    let tools = server.tools_mut().list();
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

//! `cori capability install|list` — manage Cori-blessed capability
//! binaries (the CBC registry).
//!
//! `install` fetches the platform release asset, verifies its published
//! SHA-256, and places the binary in `~/.cori/bin` (no sudo, no PATH
//! edits — the broker resolves that directory itself). `list` prints
//! the registry with per-capability install/auth state; `--json` emits
//! the same as a machine-readable array (consumed by the Console).

use anyhow::{Context, Result};
use cori_broker::cli_auth;
use cori_broker::install;
use serde::Serialize;

pub fn install_capability(id: &str) -> Result<()> {
    let spec = install::spec_for(id).with_context(|| {
        format!(
            "no install recipe for `{id}` — known capabilities: {}",
            known_ids().join(", ")
        )
    })?;

    if let Some(existing) = install::resolve_binary(id) {
        println!(
            "✓ {} is already installed at {}",
            spec.display_name,
            existing.display()
        );
        return Ok(());
    }

    println!(
        "Installing {} from github.com/{}…",
        spec.display_name, spec.github_repo
    );
    let path = install::install(id)?;
    println!("✓ Installed to {}", path.display());
    if cli_auth::for_binary(id).is_some() {
        println!("  Next: `cori login {id}` to sign in.");
    } else {
        println!("  No sign-in required — ready to use.");
    }
    Ok(())
}

#[derive(Serialize)]
struct CapabilityRow {
    #[serde(flatten)]
    status: cori_broker::capabilities::RegistryCapability,
    /// A managed `cori login` can drive sign-in end-to-end (an OAuth
    /// client is provisioned or provisionable in this build).
    managed_login: bool,
}

pub fn list(json: bool) -> Result<()> {
    let rows: Vec<CapabilityRow> = cori_broker::capabilities::registry_status()
        .into_iter()
        .map(|status| {
            let managed_login = cli_auth::for_binary(&status.id)
                .and_then(|a| {
                    cli_auth::resolve_client(&status.id, None).and_then(|c| a.managed_login(&c, &[]))
                })
                .is_some();
            CapabilityRow {
                status,
                managed_login,
            }
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    for r in &rows {
        let s = &r.status;
        let state = match (s.installed, s.authed) {
            (false, _) => "not installed".to_string(),
            (true, Some(true)) => "installed, signed in".to_string(),
            (true, Some(false)) => "installed, signed out".to_string(),
            (true, None) => "installed".to_string(),
        };
        // Installed is ready unless an auth probe says signed out
        // (auth-free capabilities have no probe and no sign-in step).
        let mark = if s.installed && s.authed != Some(false) {
            "✓"
        } else {
            "·"
        };
        println!("{mark} {:<11} {:<56} {state}", s.id, s.display_name);
        println!("         use for: {}", s.use_for);
        if let Some(p) = &s.path {
            println!("         {p}");
        }
        if let Some(remedy) = &s.remedy {
            println!("         → {remedy}");
        }
    }
    Ok(())
}

fn known_ids() -> Vec<&'static str> {
    install::REGISTRY.iter().map(|s| s.id).collect()
}

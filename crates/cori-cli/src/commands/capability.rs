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
    println!("  Next: `cori login {id}` to sign in.");
    Ok(())
}

#[derive(Serialize)]
struct CapabilityRow {
    id: String,
    display_name: String,
    installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// `true` / `false` from the auth probe; `None` when the probe
    /// could not run (binary missing or no adapter).
    #[serde(skip_serializing_if = "Option::is_none")]
    authed: Option<bool>,
    /// A managed `cori login` can drive sign-in end-to-end (an OAuth
    /// client is provisioned or provisionable in this build).
    managed_login: bool,
}

pub fn list(json: bool) -> Result<()> {
    let rows: Vec<CapabilityRow> = install::REGISTRY.iter().map(row_for).collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    for r in &rows {
        let state = match (r.installed, r.authed) {
            (false, _) => "not installed".to_string(),
            (true, Some(true)) => "installed, signed in".to_string(),
            (true, Some(false)) => "installed, signed out".to_string(),
            (true, None) => "installed".to_string(),
        };
        let mark = if r.installed && r.authed == Some(true) {
            "✓"
        } else {
            "·"
        };
        println!("{mark} {:<6} {:<24} {state}", r.id, r.display_name);
        if let Some(p) = &r.path {
            println!("         {p}");
        }
        if !r.installed {
            println!("         install: cori capability install {}", r.id);
        } else if r.authed == Some(false) {
            println!("         sign in: cori login {}", r.id);
        }
    }
    Ok(())
}

fn row_for(spec: &install::InstallSpec) -> CapabilityRow {
    let path = install::resolve_binary(spec.id);
    let installed = path.is_some();
    let authed = if installed {
        match cli_auth::check_known(spec.id) {
            cli_auth::AuthState::Ok => Some(true),
            cli_auth::AuthState::NeedsReauth { .. } => Some(false),
            cli_auth::AuthState::Unknown => None,
        }
    } else {
        None
    };
    let managed_login = cli_auth::for_binary(spec.id)
        .and_then(|a| {
            cli_auth::resolve_client(spec.id, None).and_then(|c| a.managed_login(&c, &[]))
        })
        .is_some();
    CapabilityRow {
        id: spec.id.to_string(),
        display_name: spec.display_name.to_string(),
        installed,
        path: path.map(|p| p.display().to_string()),
        authed,
        managed_login,
    }
}

fn known_ids() -> Vec<&'static str> {
    install::REGISTRY.iter().map(|s| s.id).collect()
}

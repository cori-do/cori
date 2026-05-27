//! `cori skill install --agent <name>`.
//!
//! Extracts the embedded `skill/` tree (see `build.rs`) into the target
//! agent's local skill directory. v1 supports a small set of agents whose
//! skill-directory conventions are stable; unknown agents fail with a
//! pointer to `--path` (a manual escape hatch for power users).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::embedded;

/// Supported agent identifiers and their conventional skill directories,
/// expressed relative to the user's home directory.
fn agent_skill_dir(agent: &str, home: &Path) -> Result<PathBuf> {
    let rel = match agent {
        "claude-code" => ".claude/skills/cori",
        "cursor" => ".cursor/skills/cori",
        "gemini-cli" => ".gemini/skills/cori",
        "copilot-cli" => ".copilot/skills/cori",
        other => {
            bail!(
                "unknown agent `{other}`. Supported agents in v1: claude-code, cursor, gemini-cli, copilot-cli.\nUse `--path <dir>` to install into an arbitrary directory."
            );
        }
    };
    Ok(home.join(rel))
}

pub fn install(agent: Option<String>, path: Option<PathBuf>) -> Result<()> {
    let dest = match (agent.as_deref(), path) {
        (Some(_), Some(_)) => bail!("pass either `--agent` or `--path`, not both"),
        (Some(name), None) => {
            let home = home_dir()?;
            agent_skill_dir(name, &home)?
        }
        (None, Some(p)) => p,
        (None, None) => bail!(
            "missing `--agent <name>` (e.g. claude-code, cursor, gemini-cli, copilot-cli) or `--path <dir>`"
        ),
    };

    let count = embedded::extract(embedded::skill::SKILL_FILES, &dest)
        .with_context(|| format!("installing skill into `{}`", dest.display()))?;

    println!(
        "✓ Installed Cori skill ({count} files) at {}",
        dest.display()
    );
    if let Some(name) = agent {
        match name.as_str() {
            "claude-code" => {
                println!("Open Claude Code and try `save_workflow` at the end of your next task.")
            }
            _ => println!("Restart {name} so it picks up the new skill."),
        }
    }
    Ok(())
}

/// Honour `$CORI_AGENT_HOME` for tests; fall back to the real home.
fn home_dir() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("CORI_AGENT_HOME")
        && !h.is_empty()
    {
        return Ok(PathBuf::from(h));
    }
    // The CLI already uses `paths::home()` for `~/.cori`. For skills we
    // want the *actual* user home, not `~/.cori` — the agent doesn't live
    // inside our state dir. Compute it directly.
    dirs::home_dir().ok_or_else(|| anyhow!("could not resolve user home directory ($HOME unset?)"))
}

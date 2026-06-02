//! Launcher search-bar backend: `peek_source` (classify input cheaply
//! per the §3 ladder of the bar guide) and `list_dir` (one-level local
//! directory listing for the local-browse context).
//!
//! Neither command touches the network. `peek_source` does at most one
//! `stat`; `list_dir` does one `read_dir`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{IpcError, IpcResult};

const DEFAULT_REMOTE_HOST: &str = "github.com";

/// Built-in host allowlist that mirrors `cori_run::remote::DEFAULT_HOSTS`.
/// Kept inline (rather than imported) so `peek_source` stays a pure
/// pattern check with no crate-internal coupling beyond errors.
const KNOWN_HOSTS: &[&str] = &["github.com", "gitlab.com", "bitbucket.org"];

// ---------- peek_source ----------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeekKind {
    Filter,
    Local,
    Remote,
}

#[derive(Debug, Serialize)]
pub struct PeekResult {
    pub kind: PeekKind,
    /// What Enter would act on. For local: tilde-expanded absolute
    /// path. For remote shorthand: prefixed with the default host so
    /// the chip is honest about the resolved target.
    pub normalized: String,
    /// Whether `normalized` stats to an existing directory. Only
    /// meaningful for `local`.
    pub local_exists: bool,
    /// True when `normalized` is a directory containing `manifest.md`
    /// — i.e. the path itself names a workflow folder, not just any
    /// parent directory. Used by the drag-drop handler to decide
    /// between "open launch screen" and "drill into local context."
    /// Only meaningful for `local`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_workflow_dir: bool,
    /// Set to "github.com" for the bare `owner/repo` case so the chip
    /// can render `github.com/owner/repo @ latest`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_host: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn peek_source(input: String) -> IpcResult<PeekResult> {
    // Stat is cheap but blocking. Push to the blocking pool so a slow
    // network mount doesn't stall the IPC thread on a keystroke.
    tokio::task::spawn_blocking(move || classify(&input))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("peek_source join: {e}")))
}

fn classify(raw: &str) -> PeekResult {
    let trimmed = raw.trim();

    // 1. Empty or bare token (no slash) → filter.
    if trimmed.is_empty() || (!trimmed.contains('/') && !trimmed.contains('\\')) {
        return PeekResult::filter(trimmed);
    }

    // 2. Path-ish or stat-exists → local.
    let path_marker = has_path_marker(trimmed);
    let local_path = expand_tilde(trimmed);
    let local_exists = local_path.is_dir();
    let is_workflow_dir = local_exists && local_path.join("manifest.md").is_file();
    if path_marker || local_exists {
        return PeekResult {
            kind: PeekKind::Local,
            normalized: local_path.to_string_lossy().into_owned(),
            local_exists,
            is_workflow_dir,
            default_host: None,
        };
    }

    // 3. Scheme prefix → remote.
    if has_remote_scheme(trimmed) {
        return PeekResult {
            kind: PeekKind::Remote,
            normalized: trimmed.to_string(),
            local_exists: false,
            is_workflow_dir: false,
            default_host: None,
        };
    }

    // Count slashes in the part *before* an `@ref` tail so
    // `acme/flows@v1` still classifies as the single-slash shorthand.
    let pre_at = trimmed.split('@').next().unwrap_or(trimmed);
    let first_seg = pre_at.split('/').next().unwrap_or("");
    let slash_count = pre_at.matches('/').count();
    let host_shaped = first_seg.contains('.') || is_known_host(first_seg);

    // 3 (cont.). Explicit host like `github.com/acme/flows` or a
    // configured custom host.
    if host_shaped && slash_count >= 2 {
        return PeekResult {
            kind: PeekKind::Remote,
            normalized: trimmed.to_string(),
            local_exists: false,
            is_workflow_dir: false,
            default_host: None,
        };
    }

    // 4. `owner/repo` shorthand → github.com default.
    if slash_count == 1 && !host_shaped {
        return PeekResult {
            kind: PeekKind::Remote,
            normalized: format!("{DEFAULT_REMOTE_HOST}/{trimmed}"),
            local_exists: false,
            is_workflow_dir: false,
            default_host: Some(DEFAULT_REMOTE_HOST.to_string()),
        };
    }

    // Anything else with slashes that doesn't match → filter.
    PeekResult::filter(trimmed)
}

impl PeekResult {
    fn filter(s: &str) -> Self {
        Self {
            kind: PeekKind::Filter,
            normalized: s.to_string(),
            local_exists: false,
            is_workflow_dir: false,
            default_host: None,
        }
    }
}

fn has_path_marker(s: &str) -> bool {
    s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('~')
        || s.starts_with('/')
        || s.starts_with('\\')
        || is_windows_drive_path(s)
}

fn is_windows_drive_path(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && matches!(chars.next(), Some(':'))
        && matches!(chars.next(), Some('/' | '\\'))
}

fn has_remote_scheme(s: &str) -> bool {
    s.starts_with("https://")
        || s.starts_with("http://")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
}

fn is_known_host(h: &str) -> bool {
    KNOWN_HOSTS.iter().any(|k| k.eq_ignore_ascii_case(h))
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(s)
}

// ---------- list_dir -------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirEntryKind {
    Dir,
    Workflow,
    File,
}

#[derive(Debug, Serialize)]
pub struct DirEntryDto {
    pub name: String,
    pub kind: DirEntryKind,
    /// Absolute path of the entry, for one-shot drill-in without
    /// re-joining strings on the frontend.
    pub path: String,
    /// True if the entry is a symlink (we never follow it; surfaced
    /// so the UI can hint at the indirection).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub symlink: bool,
}

#[derive(Debug, Serialize)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntryDto>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_dir(path: String) -> IpcResult<DirListing> {
    let raw = path;
    tokio::task::spawn_blocking(move || -> IpcResult<DirListing> {
        let target = expand_tilde(&raw);
        if !target.exists() {
            return Err(IpcError::NotFound(format!(
                "path does not exist: `{}`",
                target.display()
            )));
        }
        if !target.is_dir() {
            return Err(IpcError::BadRequest(format!(
                "path is not a directory: `{}`",
                target.display()
            )));
        }

        let entries = read_dir_entries(&target).map_err(IpcError::Internal)?;
        let parent = target.parent().map(|p| p.to_string_lossy().into_owned());
        let listing = DirListing {
            path: target.to_string_lossy().into_owned(),
            parent,
            entries,
        };

        // Background-write the last-browsed dir so the launcher returns
        // here on next open. Doesn't block the response.
        let target_for_save = target.clone();
        std::thread::spawn(move || {
            if let Err(e) = save_last_local_dir(&target_for_save) {
                tracing::debug!(error = %e, "could not persist last_local_dir");
            }
        });

        Ok(listing)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("list_dir join: {e}")))?
}

fn read_dir_entries(dir: &Path) -> anyhow::Result<Vec<DirEntryDto>> {
    let mut out: Vec<DirEntryDto> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();

        // Hidden/dotfiles excluded by default.
        if name.starts_with('.') {
            continue;
        }

        let path = entry.path();
        let symlink = entry
            .file_type()
            .map(|t| t.is_symlink())
            .unwrap_or(false);

        // Never follow symlinks. Treat them as plain files with a flag.
        // (`symlink_metadata` doesn't follow either — but we want
        // `is_dir` decided without following, hence file_type above.)
        if symlink {
            out.push(DirEntryDto {
                name,
                kind: DirEntryKind::File,
                path: path.to_string_lossy().into_owned(),
                symlink: true,
            });
            continue;
        }

        let kind = if path.is_dir() {
            if path.join("manifest.md").is_file() {
                DirEntryKind::Workflow
            } else {
                DirEntryKind::Dir
            }
        } else {
            DirEntryKind::File
        };

        out.push(DirEntryDto {
            name,
            kind,
            path: path.to_string_lossy().into_owned(),
            symlink: false,
        });
    }

    // Sort: workflows first, then directories, then files; each group
    // alphabetical (case-insensitive).
    out.sort_by(|a, b| {
        let order = |k: &DirEntryKind| match k {
            DirEntryKind::Workflow => 0,
            DirEntryKind::Dir => 1,
            DirEntryKind::File => 2,
        };
        order(&a.kind)
            .cmp(&order(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

// ---------- last_local_dir persistence -------------------------------------

/// Read the launcher's last-browsed dir from `~/.cori/config.toml`'s
/// `[launcher].last_local_dir`. Returns `$HOME` (or `/`) when missing
/// or unreadable — see §5 of the bar guide.
pub fn last_local_dir() -> PathBuf {
    if let Ok(cfg) = cori_run::config::Config::load()
        && let Some(v) = cfg.get("launcher.last_local_dir")
        && let Some(s) = v.as_str()
    {
        let p = PathBuf::from(s);
        if p.is_dir() {
            return p;
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn save_last_local_dir(path: &Path) -> anyhow::Result<()> {
    let mut cfg = cori_run::config::Config::load()?;
    cfg.set("launcher.last_local_dir", &path.to_string_lossy())?;
    cfg.save()?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_last_local_dir() -> IpcResult<String> {
    let p = tokio::task::spawn_blocking(last_local_dir)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("last_local_dir join: {e}")))?;
    Ok(p.to_string_lossy().into_owned())
}

// ---------- tests ----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_bare_tokens_are_filter() {
        assert!(matches!(classify("").kind, PeekKind::Filter));
        assert!(matches!(classify("hello").kind, PeekKind::Filter));
        assert!(matches!(classify("  foo  ").kind, PeekKind::Filter));
    }

    #[test]
    fn path_markers_are_local_even_without_stat() {
        assert!(matches!(classify("./foo").kind, PeekKind::Local));
        assert!(matches!(classify("../foo").kind, PeekKind::Local));
        assert!(matches!(classify("/abs/path").kind, PeekKind::Local));
    }

    #[test]
    fn explicit_host_is_remote() {
        let r = classify("github.com/acme/flows");
        assert!(matches!(r.kind, PeekKind::Remote));
        assert_eq!(r.default_host, None);
        assert_eq!(r.normalized, "github.com/acme/flows");
    }

    #[test]
    fn scheme_is_remote() {
        assert!(matches!(
            classify("https://github.com/acme/flows").kind,
            PeekKind::Remote
        ));
        assert!(matches!(
            classify("git@github.com:acme/flows.git").kind,
            PeekKind::Remote
        ));
    }

    #[test]
    fn shorthand_defaults_to_github() {
        let r = classify("acme/flows");
        assert!(matches!(r.kind, PeekKind::Remote));
        assert_eq!(r.default_host.as_deref(), Some("github.com"));
        assert_eq!(r.normalized, "github.com/acme/flows");
    }

    #[test]
    fn shorthand_with_ref_keeps_default_host() {
        let r = classify("acme/flows@v1");
        assert!(matches!(r.kind, PeekKind::Remote));
        assert_eq!(r.default_host.as_deref(), Some("github.com"));
        assert_eq!(r.normalized, "github.com/acme/flows@v1");
    }

    #[test]
    fn three_segments_no_host_falls_back_to_filter() {
        // `acme/flows/sub` looks like a path but has no marker and no
        // host segment — fall back to filter.
        assert!(matches!(
            classify("acme/flows/sub").kind,
            PeekKind::Filter
        ));
    }
}

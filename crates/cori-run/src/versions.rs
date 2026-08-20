//! Published workflow versions — `~/.cori/versions/`.
//!
//! `publish` (MCP authoring, `docs/mcp-authoring-design.md` §2.4)
//! snapshots the workflow folder's visible files so `revert` is exact.
//! Snapshots are Cori state keyed like run history — the user's folder
//! stays clean and remains the only live copy:
//!
//! ```text
//! ~/.cori/versions/<folder>-<pathhash>/v<N>/<rel files…>
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{paths, workflow_loader};

/// Version-store key for a workflow folder — same shape as the run
/// history key, so the Console can correlate the two. Canonicalizes
/// first: `/var/…` and `/private/var/…` must never key two stores.
pub fn key_for(workflow_dir: &Path) -> String {
    let canon = workflow_dir
        .canonicalize()
        .unwrap_or_else(|_| workflow_dir.to_path_buf());
    let folder_name = canon
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workflow".to_string());
    workflow_loader::run_history_key(&canon, &folder_name)
}

pub fn version_dir(workflow_dir: &Path, version: u32) -> Result<PathBuf> {
    Ok(paths::versions_dir()?
        .join(key_for(workflow_dir))
        .join(format!("v{version}")))
}

pub fn exists(workflow_dir: &Path, version: u32) -> bool {
    version_dir(workflow_dir, version)
        .map(|d| d.is_dir())
        .unwrap_or(false)
}

/// All snapshotted version numbers for a folder, ascending.
pub fn list(workflow_dir: &Path) -> Vec<u32> {
    let Ok(root) = paths::versions_dir().map(|d| d.join(key_for(workflow_dir))) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<u32> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix('v'))
                .and_then(|n| n.parse().ok())
        })
        .collect();
    out.sort_unstable();
    out
}

/// Visible files (no dotfiles, no `.git`) under a folder, as sorted
/// relative paths.
pub fn visible_files(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file()
                && let Ok(rel) = path.strip_prefix(dir)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Snapshot the folder's visible files as `v<version>`. Refuses to
/// overwrite an existing snapshot — published versions are immutable.
pub fn snapshot(workflow_dir: &Path, version: u32) -> Result<PathBuf> {
    let dest = version_dir(workflow_dir, version)?;
    if dest.exists() {
        bail!("version v{version} is already published for this folder");
    }
    // Stage into a sibling temp dir, then rename — a torn snapshot can
    // never be mistaken for a published version.
    let staging = dest.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&staging);
    for rel in visible_files(workflow_dir)? {
        let src = workflow_dir.join(&rel);
        let dst = staging.join(&rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("snapshotting `{rel}`"))?;
    }
    if !staging.exists() {
        std::fs::create_dir_all(&staging)?; // empty folder is a valid snapshot
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("landing snapshot `{}`", dest.display()))?;
    Ok(dest)
}

/// The files of a snapshot, as `(rel_path, content)`.
pub fn read_files(workflow_dir: &Path, version: u32) -> Result<Vec<(String, Vec<u8>)>> {
    let dir = version_dir(workflow_dir, version)?;
    if !dir.is_dir() {
        bail!("no snapshot v{version} for this folder");
    }
    let mut out = Vec::new();
    for rel in visible_files(&dir)? {
        let bytes = std::fs::read(dir.join(&rel))?;
        out.push((rel, bytes));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_list_read_and_immutability() {
        crate::test_env::with_temp_home(|| {
            let wf = tempfile::tempdir().unwrap();
            let dir = wf.path().canonicalize().unwrap();
            std::fs::create_dir_all(dir.join("steps")).unwrap();
            std::fs::write(dir.join("manifest.md"), "m1").unwrap();
            std::fs::write(dir.join("steps/01_a.ts"), "a").unwrap();
            std::fs::write(dir.join(".hidden"), "no").unwrap();

            snapshot(&dir, 1).unwrap();
            assert!(exists(&dir, 1));
            assert_eq!(list(&dir), vec![1]);

            let files = read_files(&dir, 1).unwrap();
            let rels: Vec<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
            assert_eq!(rels, ["manifest.md", "steps/01_a.ts"]);

            // Immutable: same version cannot be re-snapshotted.
            assert!(snapshot(&dir, 1).is_err());

            std::fs::write(dir.join("manifest.md"), "m2").unwrap();
            snapshot(&dir, 2).unwrap();
            assert_eq!(list(&dir), vec![1, 2]);
            let v1_manifest = &read_files(&dir, 1).unwrap()[0].1;
            assert_eq!(v1_manifest, b"m1", "old snapshot untouched");
        });
    }
}

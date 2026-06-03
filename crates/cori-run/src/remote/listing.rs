//! Listing workflows inside a remote repository checkout.
//!
//! This is the read-only counterpart to [`super::resolve_remote`]: it
//! shares the resolve-ref-to-sha + ensure-checkout prefix (via
//! [`super::resolve_remote_to_checkout`]) but does **not** require a
//! `manifest.md` at any specific subpath. Instead it walks the
//! checkout (or `spec.subpath` if set) for directories containing a
//! `manifest.md` and returns each as a [`RemoteWorkflowEntry`].
//!
//! No network is touched beyond what the resolver already does on
//! first resolution (or under `--update`). Manifest parsing reuses
//! [`cori_manifest::parse_manifest`] — no second parser.
//!
//! Safety guards:
//! * Symlinks are not followed.
//! * Walk depth is capped to keep pathological repos bounded.
//! * Obvious noise directories (`node_modules`, `target`, etc.) are
//!   skipped — they never contain workflow manifests in practice.
//! * Parsing a malformed manifest does not abort the listing; the bad
//!   entry is dropped with a `tracing::warn!`.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::remote::{
    RemoteRef, ensure_host_allowed_pub as ensure_host_allowed, resolve_remote_to_checkout,
};

/// Maximum directory depth to descend into when walking a repo for
/// `manifest.md`. Workflows in real repos sit no deeper than ~3–4
/// levels under the root; 6 leaves comfortable headroom without
/// exploring every transitive dependency dir.
const MAX_WALK_DEPTH: usize = 6;

/// Directory names skipped during the walk. None of these are valid
/// workflow folders, and recursing into them is wasted work (often
/// substantial — `node_modules` and friends are huge).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "__pycache__",
    "venv",
    ".venv",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRepoListing {
    pub sha: String,
    pub workflows: Vec<RemoteWorkflowEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteWorkflowEntry {
    /// Path within the repo (forward-slashes), relative to the repo
    /// root. Empty string when the workflow sits at the root.
    pub subpath: String,
    /// `name` from the manifest's frontmatter.
    pub name: String,
    /// `description` from the manifest's frontmatter.
    pub description: String,
}

/// List every workflow found inside the repo at `spec`. Resolves +
/// caches the ref the same way `cori run` does, then walks the
/// checkout (or `spec.subpath` if set) for manifests.
pub fn list_workflows(spec: &RemoteRef, update: bool) -> Result<RemoteRepoListing> {
    ensure_host_allowed(&spec.host)?;
    let checkout = resolve_remote_to_checkout(spec, update)?;

    let walk_root = if spec.subpath.is_empty() {
        checkout.checkout.clone()
    } else {
        checkout.checkout.join(&spec.subpath)
    };
    if !walk_root.is_dir() {
        anyhow::bail!(
            "subpath `{}` not found in {}/{} (resolved sha {})",
            spec.subpath,
            spec.host,
            spec.repo,
            &checkout.sha[..crate::remote::short(&checkout.sha)]
        );
    }

    let mut workflows: Vec<RemoteWorkflowEntry> = Vec::new();
    let walk_start = std::time::Instant::now();
    walk_for_manifests(
        &walk_root,
        &checkout.checkout,
        MAX_WALK_DEPTH,
        &mut workflows,
    );
    let walk_ms = walk_start.elapsed().as_millis();
    workflows.sort_by(|a, b| a.subpath.cmp(&b.subpath));

    // Log walk duration so monorepo perf can be evaluated without
    // synthetic benchmarks (see bar guide §10 — monorepo walk cost).
    tracing::info!(
        host = %spec.host,
        repo = %spec.repo,
        sha = %&checkout.sha[..crate::remote::short(&checkout.sha)],
        found = workflows.len(),
        walk_ms,
        "listed remote workflows"
    );

    Ok(RemoteRepoListing {
        sha: checkout.sha,
        workflows,
    })
}

fn walk_for_manifests(
    dir: &Path,
    repo_root: &Path,
    remaining_depth: usize,
    out: &mut Vec<RemoteWorkflowEntry>,
) {
    let manifest_path = dir.join("manifest.md");
    if manifest_path.is_file() {
        match read_workflow_entry(&manifest_path, dir, repo_root) {
            Ok(entry) => out.push(entry),
            Err(e) => {
                tracing::warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "skipping unparseable manifest"
                );
            }
        }
        // Don't recurse into a workflow folder — workflows aren't nested.
        return;
    }

    if remaining_depth == 0 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Don't follow symlinks — workflow folders are real dirs.
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue; // .git, .github, etc.
        }
        if SKIP_DIRS.iter().any(|s| name_str == *s) {
            continue;
        }

        walk_for_manifests(&path, repo_root, remaining_depth - 1, out);
    }
}

fn read_workflow_entry(
    manifest_path: &Path,
    workflow_dir: &Path,
    repo_root: &Path,
) -> Result<RemoteWorkflowEntry> {
    let src = std::fs::read_to_string(manifest_path)?;
    let manifest = cori_manifest::parse_manifest(&src).map_err(|errs| {
        anyhow::anyhow!(
            "manifest has {} validation error(s); first: {}",
            errs.len(),
            errs.first().map(|e| e.to_string()).unwrap_or_default()
        )
    })?;

    let subpath = workflow_dir
        .strip_prefix(repo_root)
        .map(|p| {
            // Force forward-slashes on the wire — repo subpaths are
            // canonically posix-shaped (matches `cori run` syntax).
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();

    Ok(RemoteWorkflowEntry {
        subpath,
        name: manifest.name,
        description: manifest.description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Minimal valid manifest frontmatter. `cori_manifest::parse_manifest`
    /// requires id/name/description/created/version.
    fn manifest_md(id: &str, name: &str, description: &str) -> String {
        format!(
            "---\nid: {id}\nname: {name}\ndescription: {description}\ncreated: 2026-01-01\nversion: 1\n---\n\nbody\n"
        )
    }

    fn write_workflow(root: &Path, sub: &str, id: &str, name: &str, desc: &str) {
        let dir = root.join(sub);
        fs::create_dir_all(&dir).unwrap();
        let desc = if desc.is_empty() { "stub" } else { desc };
        fs::write(dir.join("manifest.md"), manifest_md(id, name, desc)).unwrap();
    }

    #[test]
    fn finds_workflow_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workflow(root, "", "wf_root", "Root WF", "the only one");
        let mut out = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subpath, "");
        assert_eq!(out[0].name, "Root WF");
        assert_eq!(out[0].description, "the only one");
    }

    #[test]
    fn finds_nested_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workflow(root, "a", "wf_a", "A", "");
        write_workflow(root, "b/c", "wf_bc", "BC", "");
        write_workflow(root, "d/e/f", "wf_def", "DEF", "");
        let mut out = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut out);
        out.sort_by(|x, y| x.subpath.cmp(&y.subpath));
        let subs: Vec<&str> = out.iter().map(|w| w.subpath.as_str()).collect();
        assert_eq!(subs, vec!["a", "b/c", "d/e/f"]);
    }

    #[test]
    fn stops_descending_into_a_workflow() {
        // A workflow folder may contain its own subdirs (e.g. `steps/`,
        // `tests/`); the walker must not recurse into them or it might
        // pick up nested fixtures as separate workflows.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workflow(root, "parent", "wf_parent", "Parent", "");
        write_workflow(root, "parent/nested", "wf_nested", "Nested", "");
        let mut out = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subpath, "parent");
    }

    #[test]
    fn skips_dotdirs_and_noise_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workflow(root, ".git/wf", "wf_git", "Git", "");
        write_workflow(root, ".github/wf", "wf_gh", "GH", "");
        write_workflow(root, "node_modules/pkg/wf", "wf_nm", "NM", "");
        write_workflow(root, "target/release/wf", "wf_tg", "Tg", "");
        write_workflow(root, "real/wf", "wf_real", "Real", "");
        let mut out = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut out);
        let subs: Vec<&str> = out.iter().map(|w| w.subpath.as_str()).collect();
        assert_eq!(subs, vec!["real/wf"]);
    }

    #[test]
    fn honors_max_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // 4 levels deep — exceeds depth=2 but inside depth=6.
        write_workflow(root, "a/b/c/d", "wf_deep", "Deep", "");
        let mut shallow = Vec::new();
        walk_for_manifests(root, root, 2, &mut shallow);
        assert!(shallow.is_empty(), "depth-2 walk must miss a/b/c/d");
        let mut deep = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut deep);
        assert_eq!(deep.len(), 1);
        assert_eq!(deep[0].subpath, "a/b/c/d");
    }

    #[test]
    fn malformed_manifest_is_dropped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workflow(root, "good", "wf_good", "Good", "ok");
        // Bad: no frontmatter at all.
        let bad = root.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("manifest.md"), "no frontmatter here\n").unwrap();

        let mut out = Vec::new();
        walk_for_manifests(root, root, MAX_WALK_DEPTH, &mut out);
        // The good one survives; the bad one is dropped silently.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subpath, "good");
    }
}

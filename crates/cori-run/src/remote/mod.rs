//! Remote workflow resolution.
//!
//! Turns a `cori run <ref>` argument that names a git-hosted workflow
//! into a local directory under `~/.cori/cache/remote/<host>/<repo>/<sha>/<subpath>/`
//! that the regular workflow loader can compile and execute.

pub mod git;
pub mod listing;
pub mod pins;
pub mod refspec;
pub mod trust;

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::config::Config;
use crate::paths;

// WorkflowSource is now in cori-protocol; re-export for callers.
pub use cori_protocol::WorkflowSource;

pub use listing::{RemoteRepoListing, RemoteWorkflowEntry, list_workflows};
pub use refspec::{ArgClass, RemoteRef, RemoteRefKind, classify_arg};

const DEFAULT_HOSTS: &[&str] = &["github.com", "gitlab.com", "bitbucket.org"];

/// Outcome of [`resolve`]: the on-disk workflow folder and provenance.
pub struct Resolved {
    pub workflow_dir: PathBuf,
    pub source: WorkflowSource,
    pub remote: Option<ResolvedRemote>,
}

pub struct ResolvedRemote {
    pub spec: RemoteRef,
    pub sha: String,
    #[allow(dead_code)]
    pub newly_pinned: bool,
}

pub fn resolve(arg: &str, update: bool) -> Result<Resolved> {
    match classify_arg(arg)? {
        ArgClass::Local(path) => {
            if update {
                bail!("--update is only meaningful for remote workflows");
            }
            Ok(Resolved {
                workflow_dir: path.clone(),
                source: WorkflowSource::Local {
                    path: path.to_string_lossy().into_owned(),
                },
                remote: None,
            })
        }
        ArgClass::Remote(spec) => {
            ensure_host_allowed(&spec.host)?;
            resolve_remote(spec, update)
        }
    }
}

fn ensure_host_allowed(host: &str) -> Result<()> {
    if DEFAULT_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return Ok(());
    }
    let cfg = Config::load().ok();
    if let Some(cfg) = cfg.as_ref()
        && let Some(v) = cfg.get("remotes.hosts")
        && let Some(arr) = v.as_array()
    {
        for entry in arr {
            if let Some(s) = entry.as_str()
                && s.eq_ignore_ascii_case(host)
            {
                return Ok(());
            }
        }
    }
    bail!(
        "unknown host `{host}` — add it to [remotes].hosts in ~/.cori/config.toml \
         (e.g. `hosts = [\"git.company.com\"]`)"
    );
}

fn resolve_remote(spec: RemoteRef, update: bool) -> Result<Resolved> {
    let RemoteCheckout {
        checkout,
        sha,
        newly_pinned,
    } = resolve_remote_to_checkout(&spec, update)?;

    let workflow_dir = secure_workflow_subpath(&checkout, &spec.subpath)?;
    let manifest_path = workflow_dir.join("manifest.md");
    let manifest_is_regular = std::fs::symlink_metadata(&manifest_path)
        .map(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false);
    if !manifest_is_regular {
        bail!(
            "no manifest.md at {}/{}{}@{} (resolved sha {}). Check the path inside the \
             repo, or try a different ref.",
            spec.host,
            spec.repo,
            if spec.subpath.is_empty() {
                String::new()
            } else {
                format!("/{}", spec.subpath)
            },
            spec.ref_str_display(),
            &sha[..short(&sha)],
        );
    }

    Ok(Resolved {
        workflow_dir,
        source: WorkflowSource::Remote {
            host: spec.host.clone(),
            repo: spec.repo.clone(),
            subpath: spec.subpath.clone(),
            ref_str: spec.ref_str.clone(),
            sha: sha.clone(),
        },
        remote: Some(ResolvedRemote {
            spec,
            sha,
            newly_pinned,
        }),
    })
}

/// Resolve an in-checkout workflow path without ever following a repository
/// symlink or allowing path traversal to escape the fetched tree.
fn secure_workflow_subpath(checkout: &Path, subpath: &str) -> Result<PathBuf> {
    let canonical_checkout = checkout
        .canonicalize()
        .with_context(|| format!("resolving remote checkout `{}`", checkout.display()))?;
    let candidate = if subpath.is_empty() {
        checkout.to_path_buf()
    } else {
        checkout.join(subpath)
    };

    let mut cursor = checkout.to_path_buf();
    for component in Path::new(subpath).components() {
        let Component::Normal(component) = component else {
            bail!(
                "remote workflow subpath `{subpath}` must be relative and cannot contain `.` or `..`"
            );
        };
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "remote workflow subpath `{subpath}` traverses symlink `{}`; repository symlinks cannot select executable workflow source",
                    cursor.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspecting remote workflow path `{}`", cursor.display())
                });
            }
        }
    }

    if !candidate.exists() {
        return Ok(candidate);
    }
    let canonical_candidate = candidate
        .canonicalize()
        .with_context(|| format!("resolving remote workflow path `{}`", candidate.display()))?;
    if !canonical_candidate.starts_with(&canonical_checkout) {
        bail!(
            "remote workflow subpath `{subpath}` resolves outside checkout `{}`",
            canonical_checkout.display()
        );
    }
    Ok(canonical_candidate)
}

/// Outcome of resolving a remote spec to a cached, on-disk checkout —
/// the shared prefix of [`resolve_remote`] (which then demands a
/// manifest at the subpath) and [`listing::list_workflows`] (which
/// walks the checkout for manifests instead).
pub(crate) struct RemoteCheckout {
    /// The repo's cached checkout root — `~/.cori/cache/remote/<host>/<repo>/<sha>/`.
    pub checkout: PathBuf,
    pub sha: String,
    #[allow(dead_code)]
    pub newly_pinned: bool,
}

/// Resolve `spec`'s ref to a sha (respecting pins.json + `--update`),
/// ensure the sha is checked out locally, and return both. Network is
/// only touched when no pin exists or `update == true`.
pub(crate) fn resolve_remote_to_checkout(spec: &RemoteRef, update: bool) -> Result<RemoteCheckout> {
    let remote_root = paths::remote_cache_dir()?;
    std::fs::create_dir_all(&remote_root)
        .with_context(|| format!("creating `{}`", remote_root.display()))?;

    let pin_key = spec.pin_key();
    let mut pins = pins::load()?;
    let existing_pin = pins.get(&pin_key).cloned();

    let mut newly_pinned = false;
    let sha = if let Some(existing) = existing_pin.clone() {
        if update {
            let resolved = resolve_ref_to_sha(spec)?;
            if matches!(spec.kind, RemoteRefKind::ExactSha | RemoteRefKind::ExactTag) {
                if resolved != existing {
                    bail!(
                        "tag/sha `{}` on {}/{} now points to {}; you have it pinned at {}. \
                         Delete the pin from {} to accept the new sha (consent will be re-prompted).",
                        spec.ref_str,
                        spec.host,
                        spec.repo,
                        &resolved[..short(&resolved)],
                        &existing[..short(&existing)],
                        paths::pins_file()?.display(),
                    );
                }
                existing
            } else if resolved == existing {
                existing
            } else {
                pins.set(pin_key.clone(), resolved.clone());
                pins::save(&pins)?;
                newly_pinned = true;
                resolved
            }
        } else {
            existing
        }
    } else {
        let resolved = resolve_ref_to_sha(spec)?;
        pins.set(pin_key.clone(), resolved.clone());
        pins::save(&pins)?;
        newly_pinned = true;
        resolved
    };

    let checkout = ensure_checkout(spec, &sha)?;

    Ok(RemoteCheckout {
        checkout,
        sha,
        newly_pinned,
    })
}

/// Visible to siblings (e.g. [`listing`]) so they can use the same
/// host-allowlist gate the CLI does.
pub(crate) fn ensure_host_allowed_pub(host: &str) -> Result<()> {
    ensure_host_allowed(host)
}

pub(crate) fn short(s: &str) -> usize {
    8.min(s.len())
}

fn resolve_ref_to_sha(spec: &RemoteRef) -> Result<String> {
    let url = spec.clone_url();
    let entries = git::ls_remote(&url).with_context(|| {
        format!(
            "resolving {} (try `git ls-remote {}` to debug auth/network)",
            spec.display(),
            url
        )
    })?;

    match &spec.kind {
        RemoteRefKind::ExactSha => {
            let want = &spec.ref_str;
            for entry in &entries {
                if entry.sha == *want || entry.sha.starts_with(want) {
                    return Ok(entry.sha.clone());
                }
            }
            if want.chars().all(|c| c.is_ascii_hexdigit()) && want.len() >= 7 {
                return Ok(want.clone());
            }
            bail!("sha `{want}` not found on {}/{}", spec.host, spec.repo)
        }
        RemoteRefKind::ExactTag => {
            let tag_ref = format!("refs/tags/{}", spec.ref_str);
            for entry in &entries {
                if entry.refname == tag_ref {
                    return Ok(entry.sha.clone());
                }
            }
            bail!(
                "tag `{}` not found on {}/{}",
                spec.ref_str,
                spec.host,
                spec.repo
            )
        }
        RemoteRefKind::Branch => {
            let head_ref = format!("refs/heads/{}", spec.ref_str);
            for entry in &entries {
                if entry.refname == head_ref {
                    return Ok(entry.sha.clone());
                }
            }
            bail!(
                "branch `{}` not found on {}/{}",
                spec.ref_str,
                spec.host,
                spec.repo
            )
        }
        RemoteRefKind::LatestSemverTag => {
            let best = refspec::select_highest_semver(&entries, None);
            match best {
                Some((tag, sha)) => {
                    tracing::info!(
                        "resolved {} → {} ({})",
                        spec.display(),
                        tag,
                        &sha[..short(&sha)]
                    );
                    Ok(sha)
                }
                None => bail!(
                    "No semver tags found on {}/{}. Tag a release (e.g. `git tag v1.0.0`) \
                     or specify a branch explicitly: `cori run {}/{}@main`.",
                    spec.host,
                    spec.repo,
                    spec.host,
                    spec.repo,
                ),
            }
        }
        RemoteRefKind::SemverPrefix(prefix) => {
            let best = refspec::select_highest_semver(&entries, Some(prefix));
            match best {
                Some((tag, sha)) => {
                    tracing::info!(
                        "resolved {} → {} ({})",
                        spec.display(),
                        tag,
                        &sha[..short(&sha)]
                    );
                    Ok(sha)
                }
                None => bail!(
                    "No semver tag matching `{}` found on {}/{}",
                    spec.ref_str,
                    spec.host,
                    spec.repo,
                ),
            }
        }
    }
}

fn ensure_checkout(spec: &RemoteRef, sha: &str) -> Result<PathBuf> {
    let repo_dir = paths::remote_cache_dir()?.join(&spec.host).join(&spec.repo);
    std::fs::create_dir_all(&repo_dir)
        .with_context(|| format!("creating `{}`", repo_dir.display()))?;

    let lock_path = repo_dir.join(".lock");
    let _guard = git::FileLock::acquire(&lock_path)
        .with_context(|| format!("locking `{}`", lock_path.display()))?;

    let bare = repo_dir.join(".bare.git");
    let url = spec.clone_url();

    if !bare.exists() {
        git::clone_bare(&url, &bare)?;
    }

    let checkout_dir = repo_dir.join(sha);
    let completion_marker = repo_dir.join(format!(".{sha}.complete"));
    if checkout_dir.is_dir()
        && std::fs::read_to_string(&completion_marker).is_ok_and(|value| value.trim() == sha)
    {
        return Ok(checkout_dir);
    }
    if completion_marker.exists() {
        let stale_marker =
            repo_dir.join(format!(".{sha}.complete.stale-{}", Uuid::new_v4().simple()));
        std::fs::rename(&completion_marker, &stale_marker).with_context(|| {
            format!(
                "quarantining stale remote checkout marker `{}`",
                completion_marker.display()
            )
        })?;
    }
    if checkout_dir.exists() {
        let quarantine = repo_dir.join(format!(".{sha}.incomplete-{}", Uuid::new_v4().simple()));
        std::fs::rename(&checkout_dir, &quarantine).with_context(|| {
            format!(
                "quarantining incomplete remote checkout `{}` to `{}`",
                checkout_dir.display(),
                quarantine.display()
            )
        })?;
    }

    if !git::has_commit(&bare, sha)? {
        git::fetch_all(&bare)?;
        if !git::has_commit(&bare, sha)? {
            bail!(
                "sha {} not reachable from any branch/tag on {} after fetch",
                sha,
                url
            );
        }
    }

    let partial = repo_dir.join(format!(
        ".{sha}.partial-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&partial).with_context(|| format!("creating `{}`", partial.display()))?;
    if let Err(error) = git::checkout_sha(&bare, sha, &partial) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(error);
    }
    std::fs::rename(&partial, &checkout_dir).with_context(|| {
        format!(
            "committing remote checkout `{}` to `{}`",
            partial.display(),
            checkout_dir.display()
        )
    })?;
    let marker_partial = repo_dir.join(format!(
        ".{sha}.complete-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::write(&marker_partial, format!("{sha}\n"))
        .with_context(|| format!("writing `{}`", marker_partial.display()))?;
    std::fs::rename(&marker_partial, &completion_marker).with_context(|| {
        format!(
            "committing remote checkout marker `{}`",
            completion_marker.display()
        )
    })?;
    Ok(checkout_dir)
}

pub fn remote_run_history_key(spec: &RemoteRef) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(spec.host.as_bytes());
    h.update(b"/");
    h.update(spec.repo.as_bytes());
    h.update(b"//");
    h.update(spec.subpath.as_bytes());
    let digest = h.finalize();
    let short = hex::encode(&digest[..4]);
    let name = spec.repo_leaf();
    let leaf = if spec.subpath.is_empty() {
        name
    } else {
        spec.subpath_leaf()
    };
    format!("{leaf}-{short}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn remote_workflow_subpath_stays_inside_checkout() {
        let checkout = tempdir().expect("checkout");
        std::fs::create_dir_all(checkout.path().join("workflows/example"))
            .expect("workflow directory");
        let resolved =
            secure_workflow_subpath(checkout.path(), "workflows/example").expect("safe subpath");
        assert!(resolved.starts_with(checkout.path().canonicalize().expect("checkout root")));
        assert!(secure_workflow_subpath(checkout.path(), "../escape").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn remote_workflow_subpath_rejects_repository_symlinks() {
        use std::os::unix::fs::symlink;

        let checkout = tempdir().expect("checkout");
        let outside = tempdir().expect("outside");
        std::fs::write(outside.path().join("manifest.md"), "outside").expect("outside manifest");
        symlink(outside.path(), checkout.path().join("workflow")).expect("repository symlink");

        let error = secure_workflow_subpath(checkout.path(), "workflow")
            .expect_err("repository symlink must fail");
        assert!(error.to_string().contains("traverses symlink"));
    }
}

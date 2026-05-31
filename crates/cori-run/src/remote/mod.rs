//! Remote workflow resolution.
//!
//! Turns a `cori run <ref>` argument that names a git-hosted workflow
//! into a local directory under `~/.cori/cache/remote/<host>/<repo>/<sha>/<subpath>/`
//! that the regular workflow loader can compile and execute.

pub mod git;
pub mod pins;
pub mod refspec;
pub mod trust;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::paths;

// WorkflowSource is now in cori-protocol; re-export for callers.
pub use cori_protocol::WorkflowSource;

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

fn resolve_remote(mut spec: RemoteRef, update: bool) -> Result<Resolved> {
    let remote_root = paths::remote_cache_dir()?;
    std::fs::create_dir_all(&remote_root)
        .with_context(|| format!("creating `{}`", remote_root.display()))?;

    let pin_key = spec.pin_key();
    let mut pins = pins::load()?;
    let existing_pin = pins.get(&pin_key).cloned();

    let mut newly_pinned = false;
    let sha = if let Some(existing) = existing_pin.clone() {
        if update {
            let resolved = resolve_ref_to_sha(&spec)?;
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
        let resolved = resolve_ref_to_sha(&spec)?;
        pins.set(pin_key.clone(), resolved.clone());
        pins::save(&pins)?;
        newly_pinned = true;
        resolved
    };

    let checkout = ensure_checkout(&spec, &sha)?;

    let workflow_dir = if spec.subpath.is_empty() {
        checkout.clone()
    } else {
        checkout.join(&spec.subpath)
    };
    if !workflow_dir.join("manifest.md").is_file() {
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

    let _ = &mut spec;

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

fn short(s: &str) -> usize {
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
    if checkout_dir.join("manifest.md").is_file()
        || (checkout_dir.exists() && has_any_file(&checkout_dir))
    {
        return Ok(checkout_dir);
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

    std::fs::create_dir_all(&checkout_dir)
        .with_context(|| format!("creating `{}`", checkout_dir.display()))?;
    git::checkout_sha(&bare, sha, &checkout_dir)?;
    Ok(checkout_dir)
}

fn has_any_file(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
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

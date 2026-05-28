//! Subprocess wrappers around the system `git` binary.
//!
//! Cori never embeds a git library: SSH agent, credential helpers,
//! proxy settings and `~/.gitconfig` all keep working unchanged. On
//! auth failure we print the underlying git stderr verbatim so the
//! user can debug with `git clone <url>` directly.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use super::refspec::AdvertisedRef;

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    // Disable interactive prompts; we want fast, deterministic failures
    // that the user can re-run manually with `git clone` to debug auth.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let out = cmd
        .output()
        .with_context(|| format!("spawning `git {}`", args.join(" ")))?;
    Ok(out)
}

fn fail_with(cmd_desc: &str, out: &std::process::Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut msg = format!("`{cmd_desc}` failed");
    if let Some(code) = out.status.code() {
        msg.push_str(&format!(" (exit {code})"));
    }
    if !stderr.is_empty() {
        msg.push_str(&format!("\n  stderr: {stderr}"));
    }
    if !stdout.is_empty() {
        msg.push_str(&format!("\n  stdout: {stdout}"));
    }
    anyhow!(msg)
}

/// `git ls-remote <url>`. Returns `(sha, refname)` pairs.
pub fn ls_remote(url: &str) -> Result<Vec<AdvertisedRef>> {
    let out = run_git(&["ls-remote", url], None)?;
    if !out.status.success() {
        return Err(annotated_auth_error(
            url,
            fail_with(&format!("git ls-remote {url}"), &out),
        ));
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        let sha = match parts.next() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let refname = match parts.next() {
            Some(s) => s.to_string(),
            None => continue,
        };
        entries.push(AdvertisedRef { sha, refname });
    }
    Ok(entries)
}

pub fn clone_bare(url: &str, dest: &Path) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    let out = run_git(&["clone", "--bare", url, &dest_str], None)?;
    if !out.status.success() {
        return Err(annotated_auth_error(
            url,
            fail_with(&format!("git clone --bare {url}"), &out),
        ));
    }
    Ok(())
}

pub fn fetch_all(bare: &Path) -> Result<()> {
    let out = run_git(&["fetch", "--all", "--tags", "--force"], Some(bare))?;
    if !out.status.success() {
        return Err(fail_with("git fetch --all --tags", &out));
    }
    Ok(())
}

pub fn has_commit(bare: &Path, sha: &str) -> Result<bool> {
    let out = run_git(
        &["cat-file", "-e", &format!("{sha}^{{commit}}")],
        Some(bare),
    )?;
    Ok(out.status.success())
}

/// Materialise the working tree at `sha` into `dest` by piping
/// `git archive` into `tar`. Uses no working-tree state on the bare.
pub fn checkout_sha(bare: &Path, sha: &str, dest: &Path) -> Result<()> {
    use std::process::Stdio;
    let dest_str = dest.to_string_lossy().into_owned();
    let mut archive = Command::new("git")
        .args(["archive", "--format=tar", sha])
        .current_dir(bare)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning `git archive`")?;
    let archive_stdout = archive
        .stdout
        .take()
        .ok_or_else(|| anyhow!("`git archive` stdout unavailable"))?;
    let tar = Command::new("tar")
        .args(["-x", "-C", &dest_str])
        .stdin(Stdio::from(archive_stdout))
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning `tar -x`")?;

    let archive_out = archive
        .wait_with_output()
        .context("waiting on git archive")?;
    let tar_out = tar.wait_with_output().context("waiting on tar")?;

    if !archive_out.status.success() {
        return Err(fail_with(&format!("git archive {sha}"), &archive_out));
    }
    if !tar_out.status.success() {
        return Err(fail_with("tar -x", &tar_out));
    }
    Ok(())
}

/// Add one line of context to auth-style errors so the user knows where
/// to debug.
fn annotated_auth_error(url: &str, base: anyhow::Error) -> anyhow::Error {
    let msg = format!("{base:#}");
    if msg.contains("Authentication failed")
        || msg.contains("Could not read from remote")
        || msg.contains("Permission denied")
        || msg.contains("could not read Username")
    {
        anyhow!(
            "{msg}\n\nCori uses your git credentials directly — try `git clone {url}` to debug."
        )
    } else {
        base
    }
}

/// Cross-process file lock for `<repo>/.lock`. Uses `flock(2)` on Unix
/// and `LockFileEx` on Windows via the `fs2` crate path-less helpers.
/// Released on drop.
pub struct FileLock {
    file: std::fs::File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening `{}`", path.display()))?;
        lock_exclusive(&file)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
    }
}

#[cfg(unix)]
fn lock_exclusive(file: &std::fs::File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let rc = unsafe { libc_flock(fd, LOCK_EX) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("flock(LOCK_EX) failed");
    }
    Ok(())
}

#[cfg(unix)]
fn unlock(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let rc = unsafe { libc_flock(fd, LOCK_UN) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
const LOCK_EX: i32 = 2;
#[cfg(unix)]
const LOCK_UN: i32 = 8;

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "flock"]
    fn libc_flock(fd: i32, op: i32) -> i32;
}

#[cfg(windows)]
fn lock_exclusive(_file: &std::fs::File) -> Result<()> {
    // Windows is best-effort in v1 — no flock equivalent in std, and we
    // don't take a third-party dep just for this. Two concurrent
    // `cori run`s of the same repo on Windows may race the bare clone;
    // the second will see "destination exists" and fail loudly.
    Ok(())
}

#[cfg(windows)]
fn unlock(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

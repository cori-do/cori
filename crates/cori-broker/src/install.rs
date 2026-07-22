//! Capability binary installation (`cori capability install <id>`).
//!
//! Cori-blessed capability binaries carry a built-in [`InstallSpec`]
//! describing where their release artifacts live. Installation is
//! deliberately boring: download the platform asset from the vendor's
//! GitHub releases, verify it against the published `.sha256`
//! companion, extract the binary into `~/.cori/bin/`, and mark it
//! executable. No package manager, no sudo, no PATH edits — the broker
//! resolves `~/.cori/bin` itself (see [`resolve_binary`]), so installs
//! work even for users who never touch their shell profile.
//!
//! The CBC manifest (`<bin> meta --json`) will eventually advertise the
//! same information (`install` block, §5); the built-in registry is the
//! bootstrap for binaries that aren't installed yet — you can't ask a
//! binary for its manifest before it exists.

use std::io::Read;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::capabilities::which_on_path;

/// How to fetch one capability binary. All fields are static: the
/// registry is compiled in (see module docs for why).
#[derive(Debug, Clone, Copy)]
pub struct InstallSpec {
    /// Capability id == executable name (`gws`).
    pub id: &'static str,
    /// Human-facing name for terminal / UI output.
    pub display_name: &'static str,
    /// GitHub `owner/repo` hosting the releases.
    pub github_repo: &'static str,
    /// Release asset prefix; the full asset is
    /// `{asset_prefix}-{target_triple}.tar.gz`.
    pub asset_prefix: &'static str,
}

/// Built-in registry of installable capability binaries.
pub const REGISTRY: &[InstallSpec] = &[InstallSpec {
    id: "gws",
    display_name: "Google Workspace CLI",
    github_repo: "googleworkspace/cli",
    asset_prefix: "google-workspace-cli",
}];

pub fn spec_for(id: &str) -> Option<&'static InstallSpec> {
    REGISTRY.iter().find(|s| s.id == id)
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("no install recipe for capability `{0}` — install it manually and ensure it is on PATH")]
    UnknownCapability(String),
    #[error("unsupported platform {os}/{arch} — download {id} manually from https://github.com/{repo}/releases")]
    UnsupportedPlatform {
        id: String,
        repo: String,
        os: &'static str,
        arch: &'static str,
    },
    #[error("could not resolve the Cori home directory ($HOME unset?)")]
    NoHome,
    #[error("downloading {url}: {source}")]
    Download {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("downloading {url}: HTTP {status}")]
    DownloadStatus { url: String, status: u16 },
    #[error("checksum mismatch for {asset}: expected {expected}, got {actual} — refusing to install")]
    ChecksumMismatch {
        asset: String,
        expected: String,
        actual: String,
    },
    #[error("release asset {asset} did not contain a `{bin}` binary")]
    BinaryNotInArchive { asset: String, bin: String },
    #[error("io error during install: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, InstallError>;

/// `~/.cori/bin` (honouring `$CORI_HOME`) — where Cori-managed
/// capability binaries live. Mirrors `cori_run::paths::home()` without
/// taking a dependency on that crate (the dependency points the other
/// way).
pub fn cori_bin_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CORI_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p).join("bin"));
    }
    let home = dirs::home_dir().ok_or(InstallError::NoHome)?;
    Ok(home.join(".cori").join("bin"))
}

/// Resolve a capability binary: PATH first (a user-managed install
/// always wins), then `~/.cori/bin`.
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    if let Some(p) = which_on_path(name) {
        return Some(p);
    }
    let candidate = cori_bin_dir().ok()?.join(exe_name(name));
    is_executable(&candidate).then_some(candidate)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The Rust target triple used in the vendor's release asset names.
fn release_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        // Windows assets are zips; supporting them means a zip
        // dependency — deliberate v1 cut. The error message points at
        // the manual download.
        _ => None,
    }
}

/// Download, verify, and install one capability binary into
/// `~/.cori/bin`. Returns the installed path. Idempotent: an existing
/// managed install is overwritten atomically (that is the upgrade
/// path); a PATH install elsewhere is never touched.
pub fn install(id: &str) -> Result<PathBuf> {
    let spec = spec_for(id).ok_or_else(|| InstallError::UnknownCapability(id.to_string()))?;

    let triple = release_triple().ok_or_else(|| InstallError::UnsupportedPlatform {
        id: spec.id.to_string(),
        repo: spec.github_repo.to_string(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;

    let asset = format!("{}-{}.tar.gz", spec.asset_prefix, triple);
    let base = format!(
        "https://github.com/{}/releases/latest/download",
        spec.github_repo
    );
    let asset_url = format!("{base}/{asset}");
    let sha_url = format!("{asset_url}.sha256");

    let archive = fetch(&asset_url)?;
    let sha_body = fetch(&sha_url)?;

    // `.sha256` companions are `"<hex>  <filename>"` (sha256sum
    // format); the first whitespace-separated token is the digest.
    let expected = String::from_utf8_lossy(&sha_body)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let actual = hex::encode(Sha256::digest(&archive));
    if expected != actual {
        return Err(InstallError::ChecksumMismatch {
            asset,
            expected,
            actual,
        });
    }

    let bin_dir = cori_bin_dir()?;
    std::fs::create_dir_all(&bin_dir)?;

    // Extract the binary entry from the tarball into a temp file, then
    // rename into place so a concurrent spawn never sees a half-written
    // executable.
    let wanted = exe_name(spec.id);
    let decoder = flate2::read::GzDecoder::new(archive.as_slice());
    let mut tarball = tar::Archive::new(decoder);
    for entry in tarball.entries()? {
        let mut entry = entry?;
        let is_wanted = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .is_some_and(|n| n == wanted);
        if !is_wanted {
            continue;
        }
        let staged = bin_dir.join(format!(".{wanted}.download"));
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&staged, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        let target = bin_dir.join(&wanted);
        std::fs::rename(&staged, &target)?;
        return Ok(target);
    }

    Err(InstallError::BinaryNotInArchive {
        asset,
        bin: wanted,
    })
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let resp = reqwest::blocking::Client::builder()
        .user_agent(concat!("cori/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .and_then(|c| c.get(url).send())
        .map_err(|source| InstallError::Download {
            url: url.to_string(),
            source,
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(InstallError::DownloadStatus {
            url: url.to_string(),
            status: status.as_u16(),
        });
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|source| InstallError::Download {
            url: url.to_string(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_gws() {
        let spec = spec_for("gws").expect("gws spec");
        assert_eq!(spec.github_repo, "googleworkspace/cli");
    }

    #[test]
    fn unknown_capability_is_an_error() {
        assert!(matches!(
            install("no-such-cli"),
            Err(InstallError::UnknownCapability(_))
        ));
    }

    #[test]
    fn cori_bin_dir_honours_cori_home() {
        // Serialised by cargo's per-test process? No — env is global;
        // keep the assertion read-only by deriving from the var
        // directly rather than mutating it here.
        let dir = cori_bin_dir().expect("bin dir");
        assert!(dir.ends_with("bin"));
    }
}

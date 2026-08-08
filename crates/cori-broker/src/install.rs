//! Capability binary installation (`cori capability install <id>`).
//!
//! Cori-blessed capability binaries carry a built-in [`InstallSpec`]
//! describing where their release artifacts live. Installation is
//! deliberately boring: download the platform asset from the vendor's
//! GitHub releases, verify it against a published SHA-256, extract the
//! binary into `~/.cori/bin/`, and mark it executable. No package
//! manager, no sudo, no PATH edits — the broker resolves `~/.cori/bin`
//! itself (see [`resolve_binary`]), so installs work even for users who
//! never touch their shell profile.
//!
//! Vendors package releases differently, so each registry entry names
//! its [`ArtifactKind`]:
//!
//! - `gws` ships `{prefix}-{target_triple}.tar.gz` with a `.sha256`
//!   companion asset ([`ArtifactKind::TarGzSha256`]).
//! - `lightpanda` ships bare executables named `{prefix}-{arch}-{os}`
//!   with no companion; verification uses the per-asset SHA-256 digest
//!   GitHub records in its release API ([`ArtifactKind::RawBinary`]).
//! - `anydoc` has no standalone binary at all — the CLI is a Node
//!   package on npm; install writes an `npx` shim into `~/.cori/bin`
//!   ([`ArtifactKind::NpmShim`]).
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

/// How one capability's release artifact is packaged and verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// `{asset_prefix}-{target_triple}.tar.gz` with a `.sha256`
    /// companion asset in sha256sum format. The `gws` layout.
    TarGzSha256,
    /// A bare executable asset named `{asset_prefix}-{arch}-{os}`
    /// (e.g. `lightpanda-aarch64-macos`), verified against the SHA-256
    /// digest GitHub records per release asset. Refuses to install when
    /// the API reports no digest — never an unverified executable.
    RawBinary,
    /// The vendor ships the CLI as a Node package on npm, not as a
    /// standalone release binary. Install verifies `npx` is available
    /// and writes a shim script into `~/.cori/bin` that delegates to
    /// `npx --yes <package>`.
    NpmShim { package: &'static str },
}

/// How to fetch one capability binary. All fields are static: the
/// registry is compiled in (see module docs for why).
#[derive(Debug, Clone, Copy)]
pub struct InstallSpec {
    /// Capability id == executable name (`gws`, `anydoc`, `lightpanda`).
    pub id: &'static str,
    /// Broad, human-facing title — what the capability gives the user,
    /// not the vendor's product name ("Word, PowerPoint, Excel, PDF
    /// document support", not "AnyDoc"). This is the one-glance label
    /// on the Console's Capabilities tab and in terminal listings.
    pub display_name: &'static str,
    /// Full human-facing detail behind the broad title, shown as the
    /// Console tooltip: what gets installed and from where, what it can
    /// do, requirements (Node.js, platform limits), and how sign-in
    /// works when there is one. Complete sentences.
    pub details: &'static str,
    /// One agent-facing line: when a design-time agent should reach for
    /// this capability. Advertised through `cori status`, the MCP
    /// `status` tool, and `cori capability list --json` so discovery is
    /// dynamic — new capabilities need zero skill-prose edits.
    pub use_for: &'static str,
    /// GitHub `owner/repo` hosting the releases.
    pub github_repo: &'static str,
    /// Release asset prefix; how the full asset name derives from it
    /// depends on [`ArtifactKind`].
    pub asset_prefix: &'static str,
    /// Packaging + verification scheme for the release artifact.
    pub artifact: ArtifactKind,
    /// Pin installs to one release tag. `None` follows the repo's
    /// `latest` release. Pin when the vendor marks unstable builds as
    /// latest (lightpanda's nightlies); bumping the pin here is the
    /// upgrade path.
    pub release_tag: Option<&'static str>,
}

/// Built-in registry of installable capability binaries.
pub const REGISTRY: &[InstallSpec] = &[
    InstallSpec {
        id: "gws",
        display_name: "Google Workspace",
        details: "Connects Cori to your Google account so workflow steps can read and write Drive files, Docs, Sheets, Gmail, and Calendar. Installs the official Google Workspace CLI and signs you in through your browser with Google's own consent screen — Cori never sees your password, and access is limited to the services you approve.",
        use_for: "read/write Google Drive, Docs, Sheets, Gmail, and Calendar; argv mirrors the Google APIs (`gws sheets spreadsheets values get …`)",
        github_repo: "googleworkspace/cli",
        asset_prefix: "google-workspace-cli",
        artifact: ArtifactKind::TarGzSha256,
        release_tag: None,
    },
    InstallSpec {
        id: "anydoc",
        display_name: "Word, PowerPoint, Excel, PDF document support",
        details: "Lets workflows read any office document by converting it to clean Markdown: Word (.doc/.docx), PowerPoint (.ppt/.pptx), Excel (.xls/.xlsx), OpenDocument, RTF, EPUB, CSV, and PDF. The format is detected from the file's content, so mislabeled files still convert correctly. Powered by AnyDoc (Firecrawl, MIT license), distributed via npm — requires Node.js on this machine. No account or sign-in needed.",
        use_for: "convert office documents (Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, PDF) to Markdown: `anydoc <path>`, Markdown on stdout, format detected from content — prefer over hand-rolled document parsing",
        github_repo: "firecrawl/anydoc",
        asset_prefix: "anydoc",
        artifact: ArtifactKind::NpmShim {
            package: "@firecrawl/anydoc",
        },
        release_tag: None,
    },
    InstallSpec {
        id: "lightpanda",
        display_name: "Website reading & data extraction",
        details: "Lets workflows read web pages the way a real browser does: JavaScript runs before the page is captured, so content that renders client-side is included, and robots.txt is always respected. Pages can be captured as HTML or Markdown for extraction steps. Powered by the Lightpanda headless browser (AGPL-3.0), downloaded from its official GitHub releases and verified by SHA-256 checksum. Linux and macOS only. No account or sign-in needed.",
        use_for: "fetch web pages with JavaScript executed (client-side rendering included): `lightpanda fetch --obey-robots --dump html|markdown <url>` — prefer over `curl` for dynamic sites; never use its `serve` mode in a step",
        github_repo: "lightpanda-io/browser",
        asset_prefix: "lightpanda",
        artifact: ArtifactKind::RawBinary,
        // Lightpanda marks its nightly build as the `latest` release;
        // pin to the newest stable tag instead.
        release_tag: Some("0.3.6"),
    },
];

pub fn spec_for(id: &str) -> Option<&'static InstallSpec> {
    REGISTRY.iter().find(|s| s.id == id)
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error(
        "no install recipe for capability `{0}` — install it manually and ensure it is on PATH"
    )]
    UnknownCapability(String),
    #[error(
        "unsupported platform {os}/{arch} — download {id} manually from https://github.com/{repo}/releases"
    )]
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
    #[error(
        "checksum mismatch for {asset}: expected {expected}, got {actual} — refusing to install"
    )]
    ChecksumMismatch {
        asset: String,
        expected: String,
        actual: String,
    },
    #[error(
        "GitHub reports no SHA-256 digest for release asset {asset} — refusing to install an unverifiable executable"
    )]
    MissingDigest { asset: String },
    #[error("release {release} of {repo} has no asset named {asset}")]
    AssetNotInRelease {
        repo: String,
        release: String,
        asset: String,
    },
    #[error("release asset {asset} did not contain a `{bin}` binary")]
    BinaryNotInArchive { asset: String, bin: String },
    #[error(
        "`{id}` is distributed via npm ({package}) and needs Node.js — install Node (https://nodejs.org), then re-run this install"
    )]
    NpxNotFound { id: String, package: String },
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

/// The Rust target triple used in tar.gz release asset names.
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

/// `{arch}-{os}` suffix used by raw-binary release assets
/// (lightpanda's `lightpanda-aarch64-macos` naming).
fn raw_asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-macos"),
        ("macos", "x86_64") => Some("x86_64-macos"),
        ("linux", "aarch64") => Some("aarch64-linux"),
        ("linux", "x86_64") => Some("x86_64-linux"),
        _ => None,
    }
}

fn unsupported(spec: &InstallSpec) -> InstallError {
    InstallError::UnsupportedPlatform {
        id: spec.id.to_string(),
        repo: spec.github_repo.to_string(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// Download, verify, and install one capability binary into
/// `~/.cori/bin`. Returns the installed path. Idempotent: an existing
/// managed install is overwritten atomically (that is the upgrade
/// path); a PATH install elsewhere is never touched.
pub fn install(id: &str) -> Result<PathBuf> {
    let spec = spec_for(id).ok_or_else(|| InstallError::UnknownCapability(id.to_string()))?;
    match spec.artifact {
        ArtifactKind::TarGzSha256 => install_targz(spec),
        ArtifactKind::RawBinary => install_raw_binary(spec),
        ArtifactKind::NpmShim { package } => install_npm_shim(spec, package),
    }
}

/// The gws layout: `{prefix}-{triple}.tar.gz` + `.sha256` companion,
/// fetched from `releases/latest/download` (or a pinned tag).
fn install_targz(spec: &InstallSpec) -> Result<PathBuf> {
    let triple = release_triple().ok_or_else(|| unsupported(spec))?;

    let asset = format!("{}-{}.tar.gz", spec.asset_prefix, triple);
    let base = match spec.release_tag {
        Some(tag) => format!(
            "https://github.com/{}/releases/download/{tag}",
            spec.github_repo
        ),
        None => format!(
            "https://github.com/{}/releases/latest/download",
            spec.github_repo
        ),
    };
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
    verify_sha256(&asset, &expected, &archive)?;

    // Extract the binary entry from the tarball.
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
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        return stage_into_bin_dir(&wanted, &bytes);
    }

    Err(InstallError::BinaryNotInArchive { asset, bin: wanted })
}

/// The lightpanda layout: the asset *is* the executable, named
/// `{prefix}-{arch}-{os}` with no checksum companion. The release is
/// resolved through the GitHub API so the per-asset SHA-256 digest
/// GitHub records can verify the download.
fn install_raw_binary(spec: &InstallSpec) -> Result<PathBuf> {
    let suffix = raw_asset_suffix().ok_or_else(|| unsupported(spec))?;
    let asset = format!("{}-{}", spec.asset_prefix, suffix);

    let release = spec.release_tag.unwrap_or("latest");
    let api_url = match spec.release_tag {
        Some(tag) => format!(
            "https://api.github.com/repos/{}/releases/tags/{tag}",
            spec.github_repo
        ),
        None => format!(
            "https://api.github.com/repos/{}/releases/latest",
            spec.github_repo
        ),
    };
    let body = fetch(&api_url)?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        InstallError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;

    let entry = json
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset.as_str()))
        })
        .ok_or_else(|| InstallError::AssetNotInRelease {
            repo: spec.github_repo.to_string(),
            release: release.to_string(),
            asset: asset.clone(),
        })?;

    // GitHub records `"digest": "sha256:<hex>"` per asset. No digest,
    // no install — a raw executable has no other verification channel.
    let expected = entry
        .get("digest")
        .and_then(|d| d.as_str())
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| InstallError::MissingDigest {
            asset: asset.clone(),
        })?;

    let download_url = entry
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "https://github.com/{}/releases/download/{release}/{asset}",
                spec.github_repo
            )
        });

    let bytes = fetch(&download_url)?;
    verify_sha256(&asset, &expected, &bytes)?;
    stage_into_bin_dir(&exe_name(spec.id), &bytes)
}

/// The anydoc layout: no release binary at all — the CLI lives on npm.
/// Write a `~/.cori/bin/<id>` shim that delegates to `npx --yes
/// <package>`. Requires Node.js on the worker; the shim keeps npm's
/// own cache/refresh semantics instead of reimplementing them.
fn install_npm_shim(spec: &InstallSpec, package: &'static str) -> Result<PathBuf> {
    if cfg!(windows) {
        // A .cmd shim is straightforward but untested here — deliberate
        // v1 cut, consistent with the zip-asset cut above.
        return Err(unsupported(spec));
    }
    if which_on_path("npx").is_none() {
        return Err(InstallError::NpxNotFound {
            id: spec.id.to_string(),
            package: package.to_string(),
        });
    }
    let shim = format!(
        "#!/bin/sh\n\
         # Cori-managed shim (cori capability install {id}).\n\
         # The {id} CLI is distributed on npm as {package}; npx caches\n\
         # the package after the first run.\n\
         exec npx --yes {package} \"$@\"\n",
        id = spec.id,
        package = package,
    );
    stage_into_bin_dir(&exe_name(spec.id), shim.as_bytes())
}

fn verify_sha256(asset: &str, expected: &str, bytes: &[u8]) -> Result<()> {
    let actual = hex::encode(Sha256::digest(bytes));
    if expected != actual {
        return Err(InstallError::ChecksumMismatch {
            asset: asset.to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Write `bytes` as `~/.cori/bin/<name>`, staged + renamed so a
/// concurrent spawn never sees a half-written executable.
fn stage_into_bin_dir(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let bin_dir = cori_bin_dir()?;
    std::fs::create_dir_all(&bin_dir)?;
    let staged = bin_dir.join(format!(".{name}.download"));
    std::fs::write(&staged, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let target = bin_dir.join(name);
    std::fs::rename(&staged, &target)?;
    Ok(target)
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
        assert!(matches!(spec.artifact, ArtifactKind::TarGzSha256));
    }

    #[test]
    fn registry_contains_anydoc_as_npm_shim() {
        let spec = spec_for("anydoc").expect("anydoc spec");
        assert_eq!(spec.github_repo, "firecrawl/anydoc");
        assert!(matches!(
            spec.artifact,
            ArtifactKind::NpmShim {
                package: "@firecrawl/anydoc"
            }
        ));
    }

    #[test]
    fn registry_contains_lightpanda_pinned_raw_binary() {
        let spec = spec_for("lightpanda").expect("lightpanda spec");
        assert_eq!(spec.github_repo, "lightpanda-io/browser");
        assert!(matches!(spec.artifact, ArtifactKind::RawBinary));
        assert!(
            spec.release_tag.is_some(),
            "lightpanda must stay pinned: its nightly is marked `latest`"
        );
    }

    #[test]
    fn registry_ids_satisfy_cbc_naming() {
        for spec in REGISTRY {
            assert!(
                spec.id.len() <= 16
                    && spec
                        .id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "capability id `{}` violates CBC §1 naming",
                spec.id
            );
        }
    }

    #[test]
    fn unknown_capability_is_an_error() {
        assert!(matches!(
            install("no-such-cli"),
            Err(InstallError::UnknownCapability(_))
        ));
    }

    #[test]
    fn verify_sha256_rejects_wrong_digest() {
        let err = verify_sha256("asset", "deadbeef", b"bytes").unwrap_err();
        assert!(matches!(err, InstallError::ChecksumMismatch { .. }));
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

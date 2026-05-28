//! Parsing and classification of `cori run` arguments.
//!
//! Rules from `remote-workflows.md` §2:
//!
//! 1. Argument starting with `./`, `../`, `/`, or naming an existing
//!    directory at cwd → local path. Wins unconditionally.
//! 2. Otherwise: try as git ref (`host/owner/repo[/sub]@ref`, or SSH
//!    form `user@host:repo[/sub]@ref`). The `//` separator may be used
//!    to disambiguate the repo/subpath split.
//! 3. Otherwise: error.

use std::path::PathBuf;

use anyhow::{Result, bail};
use semver::Version;

#[derive(Debug, Clone)]
pub enum ArgClass {
    Local(PathBuf),
    Remote(RemoteRef),
}

#[derive(Debug, Clone)]
pub struct RemoteRef {
    pub host: String,
    /// e.g. `org/workflows` — owner/repo path without leading host.
    pub repo: String,
    /// In-repo subpath; `""` when the workflow is at the repo root.
    pub subpath: String,
    /// Original ref string after `@` (`""` when omitted).
    pub ref_str: String,
    pub kind: RemoteRefKind,
    /// `true` when the user wrote `host/...//sub` to explicitly split
    /// repo and subpath. Disables the resolver's probing fallback.
    #[allow(dead_code)]
    pub explicit_split: bool,
    /// SSH (`git@host:path`) vs HTTPS (`https://host/path`).
    pub transport: Transport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ssh,
}

#[derive(Debug, Clone)]
pub enum RemoteRefKind {
    /// No `@ref` → highest semver tag `vX.Y.Z`.
    LatestSemverTag,
    /// `@v1` or `@v1.2` — match highest semver with that prefix.
    SemverPrefix(String),
    /// `@vX.Y.Z` exact tag.
    ExactTag,
    /// `@<branch>`.
    Branch,
    /// `@<7+ hex>`.
    ExactSha,
}

impl RemoteRef {
    /// URL passed to `git clone` / `git ls-remote`.
    pub fn clone_url(&self) -> String {
        match self.transport {
            Transport::Ssh => format!("git@{}:{}.git", self.host, self.repo),
            Transport::Https => format!("https://{}/{}.git", self.host, self.repo),
        }
    }

    /// Stable key for `pins.json` entries. Includes the original ref so
    /// `@main` and `@v1` coexist.
    pub fn pin_key(&self) -> String {
        format!(
            "{}/{}//{}@{}",
            self.host,
            self.repo,
            self.subpath,
            if self.ref_str.is_empty() {
                "(latest)"
            } else {
                self.ref_str.as_str()
            }
        )
    }

    /// Last path component of the repo (e.g. `workflows`).
    pub fn repo_leaf(&self) -> String {
        self.repo
            .rsplit('/')
            .next()
            .unwrap_or(&self.repo)
            .to_string()
    }

    /// Last path component of the subpath.
    pub fn subpath_leaf(&self) -> String {
        self.subpath
            .rsplit('/')
            .next()
            .unwrap_or(&self.subpath)
            .to_string()
    }

    /// Display-friendly form (`host/repo/sub@ref`).
    pub fn display(&self) -> String {
        let mut s = format!("{}/{}", self.host, self.repo);
        if !self.subpath.is_empty() {
            s.push('/');
            s.push_str(&self.subpath);
        }
        if !self.ref_str.is_empty() {
            s.push('@');
            s.push_str(&self.ref_str);
        }
        s
    }

    pub fn ref_str_display(&self) -> String {
        if self.ref_str.is_empty() {
            "(latest)".to_string()
        } else {
            self.ref_str.clone()
        }
    }
}

pub fn classify_arg(arg: &str) -> Result<ArgClass> {
    if arg.starts_with("./") || arg.starts_with("../") || arg.starts_with('/') {
        return Ok(ArgClass::Local(PathBuf::from(arg)));
    }
    // Local-wins rule: bare name that names an existing cwd directory.
    let as_path = std::path::Path::new(arg);
    if as_path.is_dir() {
        return Ok(ArgClass::Local(PathBuf::from(arg)));
    }
    // Windows drive letters / backslashes — treat as local.
    if arg.contains('\\') || (arg.len() >= 2 && arg.as_bytes()[1] == b':') {
        return Ok(ArgClass::Local(PathBuf::from(arg)));
    }
    parse_remote_ref(arg).map(ArgClass::Remote)
}

/// Parse a string into a [`RemoteRef`]. Returns an error if it doesn't
/// look like a git reference at all.
pub fn parse_remote_ref(arg: &str) -> Result<RemoteRef> {
    // Split off the `@<ref>` suffix first, but be careful: the SSH
    // transport form is `git@host:path` which also contains `@`. Only
    // the *last* `@` after the first `:` (or the only `@` for HTTPS
    // form) is the ref separator.
    let (transport, body, ref_str) = split_transport_and_ref(arg)?;

    // Body is now `host/repo[/sub]` for HTTPS or `host:repo[/sub]` for SSH.
    let (host, path) = match transport {
        Transport::Ssh => {
            // body == "host:repo[/sub]"
            let (h, p) = body.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("malformed SSH ref `{arg}` (expected `user@host:path`)")
            })?;
            (h.to_string(), p.to_string())
        }
        Transport::Https => {
            // body == "host/path", or — as a shorthand — `owner/repo[/sub]`
            // with no host, which defaults to `github.com`.
            let (h, p) = body.split_once('/').ok_or_else(|| {
                anyhow::anyhow!("malformed ref `{arg}` (expected `host/owner/repo`)")
            })?;
            if looks_like_host(h) {
                (h.to_string(), p.to_string())
            } else {
                // Shorthand form: `<owner>/<repo>[/sub]` → `github.com/<owner>/<repo>[/sub]`.
                ("github.com".to_string(), body.clone())
            }
        }
    };
    if host.is_empty() || !looks_like_host(&host) {
        bail!(
            "`{arg}` is not a recognised local path or git ref \
             (did you mean `./{arg}`?)"
        );
    }

    let (repo, subpath, explicit_split) = split_repo_subpath(&path)?;

    let kind = classify_ref(&ref_str);

    Ok(RemoteRef {
        host,
        repo,
        subpath,
        ref_str,
        kind,
        explicit_split,
        transport,
    })
}

fn split_transport_and_ref(arg: &str) -> Result<(Transport, String, String)> {
    // SSH form starts with `<user>@<host>:`. Detect by the presence of
    // `:` before the first `/`.
    let first_slash = arg.find('/').unwrap_or(arg.len());
    let first_colon = arg.find(':');
    let is_ssh = matches!(first_colon, Some(c) if c < first_slash);

    if is_ssh {
        // `git@host:path[@ref]` — the @-before-host is the SSH user
        // delimiter; the @-after-path is the ref. The ref @ comes
        // after the colon.
        let colon = first_colon.unwrap();
        let user_at = arg[..colon].rfind('@').ok_or_else(|| {
            anyhow::anyhow!("malformed SSH ref `{arg}` (expected `user@host:path`)")
        })?;
        let _user = &arg[..user_at];
        let after_user = &arg[user_at + 1..]; // host:path[@ref]
        let (body, ref_str) = match after_user.rfind('@') {
            Some(idx) if idx > after_user.find(':').unwrap_or(0) => (
                after_user[..idx].to_string(),
                after_user[idx + 1..].to_string(),
            ),
            _ => (after_user.to_string(), String::new()),
        };
        Ok((Transport::Ssh, body, ref_str))
    } else {
        let (body, ref_str) = match arg.rfind('@') {
            Some(idx) => (arg[..idx].to_string(), arg[idx + 1..].to_string()),
            None => (arg.to_string(), String::new()),
        };
        Ok((Transport::Https, body, ref_str))
    }
}

fn looks_like_host(s: &str) -> bool {
    // A host must contain at least one `.` and only ASCII host chars.
    s.contains('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Implements §2.2: explicit `//` split wins; otherwise the resolver
/// will probe (§2.2 fallback). We always seed the longest plausible
/// subpath here; the caller is responsible for any progressive
/// shortening if needed.
fn split_repo_subpath(path: &str) -> Result<(String, String, bool)> {
    if let Some((repo, sub)) = path.split_once("//") {
        return Ok((repo.to_string(), sub.to_string(), true));
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        bail!("`{path}` is not a recognised git path (expected `owner/repo[/subpath]`)");
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let subpath = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        String::new()
    };
    Ok((repo, subpath, false))
}

/// Classify what kind of ref the user typed. See §2.3.
fn classify_ref(ref_str: &str) -> RemoteRefKind {
    if ref_str.is_empty() {
        return RemoteRefKind::LatestSemverTag;
    }

    // Pure hex of length ≥7 → sha.
    if ref_str.len() >= 7 && ref_str.len() <= 40 && ref_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return RemoteRefKind::ExactSha;
    }

    // `vX.Y.Z` exact semver tag.
    if let Some(rest) = ref_str.strip_prefix('v') {
        // Exact `vX.Y.Z` (possibly with pre/build, but we treat that
        // as Branch since semver excludes pre-releases).
        if Version::parse(rest).is_ok() {
            return RemoteRefKind::ExactTag;
        }
        // Prefix like `v1` or `v1.2`.
        if is_semver_prefix(rest) {
            return RemoteRefKind::SemverPrefix(rest.to_string());
        }
    }

    RemoteRefKind::Branch
}

fn is_semver_prefix(s: &str) -> bool {
    // `1` or `1.2` — components all decimal.
    if s.is_empty() {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 2 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// An advertised ref returned by `git ls-remote` parsing.
#[derive(Debug, Clone)]
pub struct AdvertisedRef {
    pub sha: String,
    /// Full ref name (`refs/tags/v1.0.0`, `refs/heads/main`).
    pub refname: String,
}

/// Pick the highest `vX.Y.Z` tag from a list of advertised refs.
/// If `prefix` is `Some("1")` or `Some("1.2")`, only tags matching that
/// numeric prefix are considered. Pre-release tags (anything after `-`)
/// are excluded. Tags without a leading `v` are not eligible.
pub fn select_highest_semver(
    entries: &[AdvertisedRef],
    prefix: Option<&str>,
) -> Option<(String, String)> {
    let mut best: Option<(Version, String, String)> = None;
    for entry in entries {
        let Some(tag) = entry.refname.strip_prefix("refs/tags/") else {
            continue;
        };
        // Ignore peeled-tag refs (`refs/tags/v1.0.0^{}`).
        if tag.ends_with("^{}") {
            continue;
        }
        let Some(rest) = tag.strip_prefix('v') else {
            continue;
        };
        let Ok(version) = Version::parse(rest) else {
            continue;
        };
        if !version.pre.is_empty() {
            continue;
        }
        if let Some(p) = prefix
            && !matches_prefix(&version, p)
        {
            continue;
        }
        match &best {
            None => best = Some((version, tag.to_string(), entry.sha.clone())),
            Some((cur, _, _)) if version > *cur => {
                best = Some((version, tag.to_string(), entry.sha.clone()))
            }
            _ => {}
        }
    }
    best.map(|(_, tag, sha)| (tag, sha))
}

fn matches_prefix(v: &Version, prefix: &str) -> bool {
    let parts: Vec<&str> = prefix.split('.').collect();
    let major: u64 = match parts.first().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => return false,
    };
    if v.major != major {
        return false;
    }
    if parts.len() >= 2 {
        let minor: u64 = match parts[1].parse().ok() {
            Some(n) => n,
            None => return false,
        };
        if v.minor != minor {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ar(refname: &str, sha: &str) -> AdvertisedRef {
        AdvertisedRef {
            refname: refname.to_string(),
            sha: sha.to_string(),
        }
    }

    #[test]
    fn classifies_local_paths() {
        assert!(matches!(classify_arg("./foo").unwrap(), ArgClass::Local(_)));
        assert!(matches!(
            classify_arg("../foo").unwrap(),
            ArgClass::Local(_)
        ));
        assert!(matches!(
            classify_arg("/abs/foo").unwrap(),
            ArgClass::Local(_)
        ));
    }

    #[test]
    fn parses_https_ref_with_subpath() {
        let r = parse_remote_ref("github.com/org/workflows/translate@v1.1.12").unwrap();
        assert_eq!(r.host, "github.com");
        assert_eq!(r.repo, "org/workflows");
        assert_eq!(r.subpath, "translate");
        assert_eq!(r.ref_str, "v1.1.12");
        assert!(matches!(r.kind, RemoteRefKind::ExactTag));
        assert!(matches!(r.transport, Transport::Https));
    }

    #[test]
    fn parses_explicit_split() {
        let r = parse_remote_ref("github.com/org/workflows//translate/fr@main").unwrap();
        assert_eq!(r.repo, "org/workflows");
        assert_eq!(r.subpath, "translate/fr");
        assert!(r.explicit_split);
        assert!(matches!(r.kind, RemoteRefKind::Branch));
    }

    #[test]
    fn parses_ssh_ref() {
        let r = parse_remote_ref("git@git.company.com:repo/translate@v2.0").unwrap();
        assert_eq!(r.host, "git.company.com");
        assert_eq!(r.repo, "repo/translate");
        assert_eq!(r.ref_str, "v2.0");
        assert!(matches!(r.transport, Transport::Ssh));
        assert!(matches!(r.kind, RemoteRefKind::SemverPrefix(_)));
    }

    #[test]
    fn parses_refless_and_sha() {
        let r = parse_remote_ref("github.com/org/repo").unwrap();
        assert!(matches!(r.kind, RemoteRefKind::LatestSemverTag));
        let r2 = parse_remote_ref("github.com/org/repo@a3f9c2dabc").unwrap();
        assert!(matches!(r2.kind, RemoteRefKind::ExactSha));
    }

    #[test]
    fn rejects_unrecognised_arg() {
        assert!(parse_remote_ref("nothost").is_err());
    }

    #[test]
    fn parses_github_shorthand() {
        let r = parse_remote_ref("cori-do/workflows/code_only").unwrap();
        assert_eq!(r.host, "github.com");
        assert_eq!(r.repo, "cori-do/workflows");
        assert_eq!(r.subpath, "code_only");
        assert!(r.ref_str.is_empty());
        assert!(matches!(r.transport, Transport::Https));

        let r2 = parse_remote_ref("cori-do/workflows/code_only@v1").unwrap();
        assert_eq!(r2.host, "github.com");
        assert_eq!(r2.repo, "cori-do/workflows");
        assert_eq!(r2.subpath, "code_only");
        assert!(matches!(r2.kind, RemoteRefKind::SemverPrefix(_)));
    }

    #[test]
    fn picks_highest_semver() {
        let entries = vec![
            ar("refs/tags/v1.0.0", "sha1"),
            ar("refs/tags/v1.1.12", "sha2"),
            ar("refs/tags/v1.1.13-rc1", "sha3"),
            ar("refs/tags/v2.0.0", "sha4"),
            ar("refs/heads/main", "shaH"),
        ];
        let (tag, sha) = select_highest_semver(&entries, None).unwrap();
        assert_eq!(tag, "v2.0.0");
        assert_eq!(sha, "sha4");

        let (tag, _) = select_highest_semver(&entries, Some("1")).unwrap();
        assert_eq!(tag, "v1.1.12");

        let (tag, _) = select_highest_semver(&entries, Some("1.0")).unwrap();
        assert_eq!(tag, "v1.0.0");
    }

    #[test]
    fn ignores_unversioned_and_prerelease_tags() {
        let entries = vec![
            ar("refs/tags/1.2.3", "sha1"), // no leading v
            ar("refs/tags/v1.0.0-rc1", "sha2"),
        ];
        assert!(select_highest_semver(&entries, None).is_none());
    }
}

//! Deterministic, bounded transport for workflow source.
//!
//! A Temporal activity may run on a different machine from the process that
//! compiled the workflow. The bundle is therefore part of the activity input,
//! not an out-of-band mutable path. Extraction is deliberately manual and
//! fail-closed: archive paths and entry types are validated before any write.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use cori_protocol::{
    MAX_WORKFLOW_SOURCE_BYTES, MAX_WORKFLOW_SOURCE_FILE_BYTES, MAX_WORKFLOW_SOURCE_FILES,
    MAX_WORKFLOW_SOURCE_PATH_BYTES, MAX_WORKFLOW_SOURCE_PATH_DEPTH, SourceBundle,
    SourceBundleEncoding, workflow_source_component_looks_sensitive,
};
use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use walkdir::{DirEntry, WalkDir};

use crate::{BrokerError, Result};

pub const SOURCE_BUNDLE_FORMAT_VERSION: u32 = 1;
pub const MAX_SERIALIZED_BUNDLE_BYTES: usize = 256 * 1024;
pub const MAX_COMPRESSED_BYTES: usize = 192 * 1024;
pub const MAX_UNCOMPRESSED_BYTES: u64 = MAX_WORKFLOW_SOURCE_BYTES;
pub const MAX_FILE_BYTES: u64 = MAX_WORKFLOW_SOURCE_FILE_BYTES;
pub const MAX_FILES: u32 = MAX_WORKFLOW_SOURCE_FILES as u32;
pub const MAX_PATH_BYTES: usize = MAX_WORKFLOW_SOURCE_PATH_BYTES;
pub const MAX_PATH_DEPTH: usize = MAX_WORKFLOW_SOURCE_PATH_DEPTH;
pub const MAX_PROJECTED_HISTORY_BYTES: usize = 8 * 1024 * 1024;

fn rejected(message: impl Into<String>) -> BrokerError {
    BrokerError::SourceBundle {
        message: message.into(),
    }
}

/// Build a deterministic archive from an already-verified execution snapshot.
pub fn build(root: &Path, expected_content_sha256: &str) -> Result<SourceBundle> {
    validate_digest("workflow content", expected_content_sha256)?;
    verify_tree(root, expected_content_sha256)?;

    let mut files = collect_files(root)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    reject_path_collisions(files.iter().map(|(path, _, _)| path.as_str()))?;

    let encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    archive.mode(tar::HeaderMode::Deterministic);

    let mut uncompressed_bytes = 0_u64;
    for (relative, absolute, size) in &files {
        uncompressed_bytes = uncompressed_bytes
            .checked_add(*size)
            .ok_or_else(|| rejected("workflow source size overflow"))?;
        if uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(rejected(format!(
                "workflow source is {uncompressed_bytes} bytes; cross-machine runs allow at most {MAX_UNCOMPRESSED_BYTES} bytes"
            )));
        }

        let mut input = std::fs::File::open(absolute).map_err(|error| {
            rejected(format!(
                "opening source file `{}`: {error}",
                absolute.display()
            ))
        })?;
        let mut header = Header::new_gnu();
        header
            .set_path(relative)
            .map_err(|error| rejected(format!("encoding source path `{relative}`: {error}")))?;
        header.set_entry_type(EntryType::Regular);
        header.set_size(*size);
        header.set_mode(0o444);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append(&header, &mut input)
            .map_err(|error| rejected(format!("archiving source file `{relative}`: {error}")))?;
    }

    let encoder = archive
        .into_inner()
        .map_err(|error| rejected(format!("finishing source tar archive: {error}")))?;
    let archive_bytes = encoder
        .finish()
        .map_err(|error| rejected(format!("finishing source gzip stream: {error}")))?;
    if archive_bytes.len() > MAX_COMPRESSED_BYTES {
        return Err(rejected(format!(
            "compressed workflow source is {} bytes; cross-machine runs allow at most {MAX_COMPRESSED_BYTES} bytes",
            archive_bytes.len()
        )));
    }

    // Detect a concurrent source edit while the archive was being streamed.
    verify_tree(root, expected_content_sha256)?;

    let bundle = SourceBundle {
        format_version: SOURCE_BUNDLE_FORMAT_VERSION,
        encoding: SourceBundleEncoding::TarGzipBase64,
        content_sha256: expected_content_sha256.to_string(),
        archive_sha256: sha256(&archive_bytes),
        file_count: u32::try_from(files.len())
            .map_err(|_| rejected("workflow source file count overflow"))?,
        uncompressed_bytes,
        archive_b64: BASE64.encode(archive_bytes),
    };
    validate_metadata(&bundle)?;
    Ok(bundle)
}

/// Reject a run before Temporal start if repeating the bundle in activity
/// payloads would make history unreasonably large.
pub fn validate_history_budget(bundle: &SourceBundle, activity_count: usize) -> Result<()> {
    let serialized = serde_json::to_vec(bundle)
        .map_err(|error| rejected(format!("serializing source bundle: {error}")))?;
    if serialized.len() > MAX_SERIALIZED_BUNDLE_BYTES {
        return Err(rejected(format!(
            "serialized workflow source bundle is {} bytes; limit is {MAX_SERIALIZED_BUNDLE_BYTES}",
            serialized.len()
        )));
    }
    let copies = activity_count
        .checked_add(1)
        .ok_or_else(|| rejected("source history copy count overflow"))?;
    let projected = serialized
        .len()
        .checked_mul(copies)
        .ok_or_else(|| rejected("projected Temporal history size overflow"))?;
    if projected > MAX_PROJECTED_HISTORY_BYTES {
        return Err(rejected(format!(
            "workflow source would contribute approximately {projected} bytes to Temporal history across {activity_count} activities; limit is {MAX_PROJECTED_HISTORY_BYTES}"
        )));
    }
    Ok(())
}

/// Materialize a validated bundle into a worker-local content-addressed cache.
pub fn materialize(bundle: &SourceBundle, cache_root: &Path) -> Result<PathBuf> {
    validate_metadata(bundle)?;
    let archive_bytes = BASE64
        .decode(&bundle.archive_b64)
        .map_err(|error| rejected(format!("decoding source bundle base64: {error}")))?;
    if archive_bytes.len() > MAX_COMPRESSED_BYTES {
        return Err(rejected(format!(
            "compressed source bundle exceeds {MAX_COMPRESSED_BYTES} bytes"
        )));
    }
    let actual_archive_sha = sha256(&archive_bytes);
    if actual_archive_sha != bundle.archive_sha256 {
        return Err(rejected(format!(
            "source archive digest mismatch (expected {}, got {actual_archive_sha})",
            bundle.archive_sha256
        )));
    }

    create_private_dir_all(cache_root).map_err(|error| {
        rejected(format!(
            "creating source cache `{}`: {error}",
            cache_root.display()
        ))
    })?;
    let target = cache_root.join(&bundle.content_sha256);
    if target.exists() {
        match verify_tree(&target, &bundle.content_sha256) {
            Ok(()) => {
                make_tree_read_only(&target)?;
                return target.canonicalize().map_err(|error| {
                    rejected(format!(
                        "resolving cached workflow source `{}`: {error}",
                        target.display()
                    ))
                });
            }
            Err(_) => {
                let quarantine = cache_root.join(format!(
                    ".{}.corrupt-{}-{:016x}",
                    bundle.content_sha256,
                    std::process::id(),
                    rand::random::<u64>()
                ));
                fs::rename(&target, &quarantine).map_err(|error| {
                    rejected(format!(
                        "quarantining corrupt source cache `{}` to `{}`: {error}",
                        target.display(),
                        quarantine.display()
                    ))
                })?;
                make_tree_read_only(&quarantine)?;
            }
        }
    }

    let partial = cache_root.join(format!(
        ".{}.partial-{}-{:016x}",
        bundle.content_sha256,
        std::process::id(),
        rand::random::<u64>()
    ));
    create_private_dir(&partial).map_err(|error| {
        rejected(format!(
            "creating source extraction directory `{}`: {error}",
            partial.display()
        ))
    })?;

    let extracted = extract_archive(bundle, &archive_bytes, &partial)
        .and_then(|()| verify_tree(&partial, &bundle.content_sha256));
    if let Err(error) = extracted {
        let _ = fs::remove_dir_all(&partial);
        return Err(error);
    }

    match fs::rename(&partial, &target) {
        Ok(()) => {
            make_tree_read_only(&target)?;
        }
        Err(rename_error) if target.exists() => {
            let winner = verify_tree(&target, &bundle.content_sha256);
            let _ = fs::remove_dir_all(&partial);
            winner.map_err(|error| {
                rejected(format!(
                    "concurrent source-cache winner was invalid after rename failed ({rename_error}): {error}"
                ))
            })?;
            make_tree_read_only(&target)?;
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&partial);
            return Err(rejected(format!(
                "committing source cache `{}` to `{}`: {error}",
                partial.display(),
                target.display()
            )));
        }
    }

    target.canonicalize().map_err(|error| {
        rejected(format!(
            "resolving materialized workflow source `{}`: {error}",
            target.display()
        ))
    })
}

fn collect_files(root: &Path) -> Result<Vec<(String, PathBuf, u64)>> {
    fn include_entry(entry: &DirEntry) -> bool {
        entry.depth() == 0 || entry.file_name() != ".git"
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(include_entry)
    {
        let entry = entry
            .map_err(|error| rejected(format!("walking source `{}`: {error}", root.display())))?;
        if entry.depth() == 0 || entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(rejected(format!(
                "unsupported source entry `{}`; only regular files and directories are allowed",
                entry.path().display()
            )));
        }
        let relative = entry.path().strip_prefix(root).map_err(|error| {
            rejected(format!(
                "deriving relative source path for `{}`: {error}",
                entry.path().display()
            ))
        })?;
        let portable = validate_portable_path(relative)?;
        let size = entry
            .metadata()
            .map_err(|error| {
                rejected(format!(
                    "reading source metadata `{}`: {error}",
                    entry.path().display()
                ))
            })?
            .len();
        if size > MAX_FILE_BYTES {
            return Err(rejected(format!(
                "source file `{portable}` is {size} bytes; per-file limit is {MAX_FILE_BYTES}"
            )));
        }
        files.push((portable, entry.into_path(), size));
        if files.len() > MAX_FILES as usize {
            return Err(rejected(format!(
                "workflow source contains more than {MAX_FILES} files"
            )));
        }
    }
    Ok(files)
}

fn extract_archive(bundle: &SourceBundle, bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| rejected(format!("reading source tar entries: {error}")))?;
    let mut paths = BTreeSet::new();
    let mut folded_paths = BTreeSet::new();
    let mut file_count = 0_u32;
    let mut total_bytes = 0_u64;

    for entry in entries {
        let mut entry =
            entry.map_err(|error| rejected(format!("reading source tar entry: {error}")))?;
        if !entry.header().entry_type().is_file() {
            return Err(rejected(format!(
                "source archive contains unsupported entry type {:?}",
                entry.header().entry_type()
            )));
        }
        let raw_path = entry
            .path()
            .map_err(|error| rejected(format!("reading source archive path: {error}")))?;
        let portable = validate_portable_path(&raw_path)?;
        let folded = portable.to_lowercase();
        if !paths.insert(portable.clone()) || !folded_paths.insert(folded) {
            return Err(rejected(format!(
                "source archive contains duplicate or case-colliding path `{portable}`"
            )));
        }

        file_count = file_count
            .checked_add(1)
            .ok_or_else(|| rejected("source archive file count overflow"))?;
        if file_count > MAX_FILES {
            return Err(rejected(format!(
                "source archive contains more than {MAX_FILES} files"
            )));
        }
        let size = entry.size();
        if size > MAX_FILE_BYTES {
            return Err(rejected(format!(
                "source archive file `{portable}` exceeds {MAX_FILE_BYTES} bytes"
            )));
        }
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| rejected("source archive inflated-size overflow"))?;
        if total_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(rejected(format!(
                "source archive expands beyond {MAX_UNCOMPRESSED_BYTES} bytes"
            )));
        }

        let output_path = destination.join(&portable);
        let parent = output_path
            .parent()
            .ok_or_else(|| rejected(format!("source path `{portable}` has no parent")))?;
        create_private_dir_all(parent).map_err(|error| {
            rejected(format!(
                "creating source directory `{}`: {error}",
                parent.display()
            ))
        })?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options.open(&output_path).map_err(|error| {
            rejected(format!(
                "creating source file `{}`: {error}",
                output_path.display()
            ))
        })?;
        let copied = std::io::copy(&mut entry, &mut output).map_err(|error| {
            rejected(format!(
                "extracting source file `{}`: {error}",
                output_path.display()
            ))
        })?;
        output.flush().map_err(|error| {
            rejected(format!(
                "flushing source file `{}`: {error}",
                output_path.display()
            ))
        })?;
        if copied != size {
            return Err(rejected(format!(
                "source archive file `{portable}` declared {size} bytes but yielded {copied}"
            )));
        }
    }

    if file_count != bundle.file_count || total_bytes != bundle.uncompressed_bytes {
        return Err(rejected(format!(
            "source archive metadata mismatch (declared {} files/{} bytes, extracted {file_count} files/{total_bytes} bytes)",
            bundle.file_count, bundle.uncompressed_bytes
        )));
    }
    Ok(())
}

fn validate_metadata(bundle: &SourceBundle) -> Result<()> {
    if bundle.format_version != SOURCE_BUNDLE_FORMAT_VERSION {
        return Err(rejected(format!(
            "unsupported source bundle format version {}",
            bundle.format_version
        )));
    }
    if bundle.encoding != SourceBundleEncoding::TarGzipBase64 {
        return Err(rejected("unsupported source bundle encoding"));
    }
    validate_digest("workflow content", &bundle.content_sha256)?;
    validate_digest("source archive", &bundle.archive_sha256)?;
    if bundle.file_count > MAX_FILES {
        return Err(rejected(format!(
            "source bundle declares more than {MAX_FILES} files"
        )));
    }
    if bundle.uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
        return Err(rejected(format!(
            "source bundle declares more than {MAX_UNCOMPRESSED_BYTES} inflated bytes"
        )));
    }
    let serialized = serde_json::to_vec(bundle)
        .map_err(|error| rejected(format!("serializing source bundle: {error}")))?;
    if serialized.len() > MAX_SERIALIZED_BUNDLE_BYTES {
        return Err(rejected(format!(
            "serialized source bundle exceeds {MAX_SERIALIZED_BUNDLE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_digest(label: &str, digest: &str) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(rejected(format!(
            "{label} digest must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_portable_path(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| rejected("source paths must be valid UTF-8"))?;
    let mut depth = 0_usize;
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err(rejected(format!(
                "source path `{text}` must be relative and cannot contain `.` or `..`"
            )));
        };
        depth += 1;
        if depth > MAX_PATH_DEPTH {
            return Err(rejected(format!(
                "source path `{text}` exceeds maximum depth {MAX_PATH_DEPTH}"
            )));
        }
        let part = component
            .to_str()
            .ok_or_else(|| rejected("source path components must be valid UTF-8"))?;
        if part.chars().any(|character| {
            character <= '\u{1f}'
                || character == '\u{7f}'
                || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\')
        }) {
            return Err(rejected(format!(
                "source path component `{part}` contains a character forbidden on a supported platform"
            )));
        }
        if part.eq_ignore_ascii_case(".git") {
            return Err(rejected(
                "source bundle paths cannot contain a `.git` component",
            ));
        }
        if workflow_source_component_looks_sensitive(part) {
            return Err(rejected(format!(
                "workflow source `{text}` looks like a credential or secret file; keep secrets outside the workflow folder because activity source is persisted in Temporal history"
            )));
        }
        if part.ends_with(['.', ' ']) {
            return Err(rejected(format!(
                "source path component `{part}` has a non-portable trailing character"
            )));
        }
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem
                .strip_prefix("COM")
                .and_then(|value| value.parse::<u8>().ok())
                .is_some_and(|value| (1..=9).contains(&value))
            || stem
                .strip_prefix("LPT")
                .and_then(|value| value.parse::<u8>().ok())
                .is_some_and(|value| (1..=9).contains(&value));
        if reserved {
            return Err(rejected(format!(
                "source path component `{part}` is reserved on Windows"
            )));
        }
        parts.push(part);
    }
    if depth == 0 {
        return Err(rejected("source archive path cannot be empty"));
    }
    let portable = parts.join("/");
    if portable.len() > MAX_PATH_BYTES {
        return Err(rejected(format!(
            "source path length must be 1..={MAX_PATH_BYTES} UTF-8 bytes"
        )));
    }
    Ok(portable)
}

fn reject_path_collisions<'a>(paths: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for path in paths {
        if !exact.insert(path.to_string()) || !folded.insert(path.to_lowercase()) {
            return Err(rejected(format!(
                "workflow source contains duplicate or case-colliding path `{path}`"
            )));
        }
    }
    Ok(())
}

fn verify_tree(root: &Path, expected: &str) -> Result<()> {
    let actual = cori_compiler::workflow_content_hash(root)
        .map_err(|error| rejected(format!("hashing source tree `{}`: {error}", root.display())))?;
    if actual != expected {
        return Err(rejected(format!(
            "workflow source digest mismatch for `{}` (expected {expected}, got {actual})",
            root.display()
        )));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)
}

fn make_tree_read_only(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root).contents_first(true) {
        let entry = entry.map_err(|error| {
            rejected(format!(
                "walking materialized source `{}`: {error}",
                root.display()
            ))
        })?;
        let metadata = entry.metadata().map_err(|error| {
            rejected(format!(
                "reading materialized source metadata `{}`: {error}",
                entry.path().display()
            ))
        })?;
        #[cfg(unix)]
        let permissions = fs::Permissions::from_mode(if metadata.is_dir() { 0o500 } else { 0o400 });
        #[cfg(not(unix))]
        let permissions = {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            permissions
        };
        fs::set_permissions(entry.path(), permissions).map_err(|error| {
            rejected(format!(
                "marking materialized source `{}` read-only: {error}",
                entry.path().display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn source_tree(root: &Path) -> String {
        fs::create_dir_all(root.join("steps")).expect("steps");
        fs::write(
            root.join("manifest.md"),
            "---\nid: bundled\nname: Bundled\ndescription: test\ncreated: 2026-07-28\nversion: 1\ntools_required: []\n---\n",
        )
        .expect("manifest");
        fs::write(root.join("steps/01_code.ts"), "export const value = 1;\n").expect("step");
        cori_compiler::workflow_content_hash(root).expect("source hash")
    }

    #[test]
    fn bundle_is_deterministic_and_round_trips() {
        let source = tempdir().expect("source");
        let cache = tempdir().expect("cache");
        let digest = source_tree(source.path());
        let first = build(source.path(), &digest).expect("first bundle");

        let mut permissions = fs::metadata(source.path().join("steps/01_code.ts"))
            .expect("metadata")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(source.path().join("steps/01_code.ts"), permissions)
            .expect("permissions");
        let second = build(source.path(), &digest).expect("second bundle");
        assert_eq!(first, second);

        let materialized = materialize(&first, cache.path()).expect("materialized");
        assert_eq!(
            cori_compiler::workflow_content_hash(&materialized).expect("round-trip hash"),
            digest
        );
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&materialized)
                    .expect("materialized directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o500
            );
            assert_eq!(
                fs::metadata(materialized.join("manifest.md"))
                    .expect("materialized file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
        }
    }

    #[test]
    fn rejects_digest_and_declared_size_tampering() {
        let source = tempdir().expect("source");
        let cache = tempdir().expect("cache");
        let digest = source_tree(source.path());
        let mut bundle = build(source.path(), &digest).expect("bundle");
        bundle.archive_sha256 = "0".repeat(64);
        assert!(materialize(&bundle, cache.path()).is_err());

        let mut bundle = build(source.path(), &digest).expect("bundle");
        bundle.uncompressed_bytes += 1;
        assert!(materialize(&bundle, cache.path()).is_err());
    }

    #[test]
    fn portable_paths_reject_traversal_and_cross_platform_collisions() {
        for path in [
            "../escape.ts",
            "/absolute.ts",
            "steps\\escape.ts",
            "CON.ts",
            "steps/query?.ts",
            "steps/quote\".ts",
            "steps/control\u{001f}.ts",
        ] {
            assert!(validate_portable_path(Path::new(path)).is_err(), "{path}");
        }
        assert!(reject_path_collisions(["steps/A.ts", "steps/a.ts"].into_iter()).is_err());
        for path in [
            ".env",
            ".env.production",
            "credentials.json",
            "client_secret_123.json",
            "keys/service-account.pem",
        ] {
            assert!(validate_portable_path(Path::new(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn history_budget_counts_every_payload_copy() {
        let source = tempdir().expect("source");
        let digest = source_tree(source.path());
        let bundle = build(source.path(), &digest).expect("bundle");
        validate_history_budget(&bundle, 2).expect("small history");
        assert!(validate_history_budget(&bundle, usize::MAX).is_err());
    }
}

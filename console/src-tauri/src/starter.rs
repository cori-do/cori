//! Resolution for workflows shipped inside the desktop app.
//!
//! The launcher uses a stable `cori-starter://<id>` source instead of a
//! mutable git path. Tauri copies the canonical workflow folders into the app
//! resources directory, then both preview and run resolve the URI to that
//! immutable local folder before entering Cori's normal workflow pipeline.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::{IpcError, IpcResult};

const STARTER_SCHEME: &str = "cori-starter://";
const BUNDLED_STARTERS: &[&str] = &[
    "disk_space_snapshot",
    "largest_files_report",
    "gws_meeting_prep",
    "gws_weekly_digest",
    "gws_sheet_range_snapshot",
];

/// Resolve a launcher source when it names a bundled starter. Ordinary local
/// paths and git refs pass through unchanged.
pub fn resolve_source(app: &AppHandle, source: &str) -> IpcResult<String> {
    let resource_dir = app.path().resource_dir().map_err(|error| {
        IpcError::Internal(anyhow::anyhow!(
            "resolving Cori Console resource directory: {error}"
        ))
    })?;
    resolve_source_at(&resource_dir, source)
}

fn resolve_source_at(resource_dir: &Path, source: &str) -> IpcResult<String> {
    let Some(id) = source.strip_prefix(STARTER_SCHEME) else {
        return Ok(source.to_string());
    };
    if !BUNDLED_STARTERS.contains(&id) {
        return Err(IpcError::BadRequest(format!(
            "unknown bundled starter `{id}`"
        )));
    }

    let starter_root = resource_dir.join("starter_workflows");
    let folder = starter_root.join(id);
    if !folder.join("manifest.md").is_file() {
        return Err(IpcError::WorkflowMissing(source.to_string()));
    }

    let canonical_root = starter_root.canonicalize().map_err(|error| {
        IpcError::Internal(anyhow::anyhow!(
            "resolving bundled starter root `{}`: {error}",
            starter_root.display()
        ))
    })?;
    let canonical_folder = folder.canonicalize().map_err(|error| {
        IpcError::Internal(anyhow::anyhow!("resolving bundled starter `{id}`: {error}"))
    })?;
    if !canonical_folder.starts_with(&canonical_root) {
        return Err(IpcError::BadRequest(format!(
            "bundled starter `{id}` resolves outside its resource root"
        )));
    }

    path_to_source(canonical_folder)
}

fn path_to_source(path: PathBuf) -> IpcResult<String> {
    path.into_os_string().into_string().map_err(|_| {
        IpcError::Internal(anyhow::anyhow!("bundled starter path is not valid Unicode"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn examples_resource_dir() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.pop();
        path.push("examples");
        path
    }

    #[test]
    fn resolves_every_declared_bundled_starter() {
        let root = examples_resource_dir();
        for id in BUNDLED_STARTERS {
            let source = resolve_source_at(&root, &format!("{STARTER_SCHEME}{id}"))
                .unwrap_or_else(|error| panic!("{id} should resolve: {error:#}"));
            assert!(Path::new(&source).join("manifest.md").is_file());
        }
    }

    #[test]
    fn rejects_unknown_bundled_starter() {
        let error = resolve_source_at(&examples_resource_dir(), "cori-starter://not_in_the_bundle")
            .expect_err("unknown starter should fail");
        assert!(matches!(error, IpcError::BadRequest(_)));
    }

    #[test]
    fn leaves_regular_sources_unchanged() {
        let source = "github.com/cori-do/workflows/hn_digest@v0.2.1";
        assert_eq!(
            resolve_source_at(&examples_resource_dir(), source).expect("pass through"),
            source
        );
    }
}

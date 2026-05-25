//! Static tables of embedded asset files.
//!
//! Populated by [`build.rs`](../build.rs) at compile time from the on-disk
//! `skill/` and `examples/hello_world/` trees. Consumers iterate the slice
//! and write each `(relative_path, contents)` pair to disk; the embed
//! layer itself is intentionally dumb.

#![allow(clippy::needless_raw_string_hashes)]

pub mod skill {
    include!(concat!(env!("OUT_DIR"), "/skill_files.rs"));
}

pub mod hello_world {
    include!(concat!(env!("OUT_DIR"), "/hello_world_files.rs"));
}

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Materialise an embedded file table under `dest`, creating parent
/// directories as needed and overwriting existing files. Returns the
/// number of files written.
pub fn extract(files: &[(&str, &[u8])], dest: &Path) -> Result<usize> {
    fs::create_dir_all(dest)
        .with_context(|| format!("creating destination `{}`", dest.display()))?;
    for (rel, bytes) in files {
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        fs::write(&target, bytes).with_context(|| format!("writing `{}`", target.display()))?;
    }
    Ok(files.len())
}

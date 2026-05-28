//! `~/.cori/cache/remote/pins.json` — `<ref> → <sha>` map.
//!
//! The pin is the source of truth for what a reference resolves to.
//! See `remote-workflows.md` §4 for the resolution algorithm.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Pins {
    #[serde(flatten)]
    pub entries: BTreeMap<String, PinEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinEntry {
    pub sha: String,
    pub resolved_at: chrono::DateTime<chrono::Utc>,
}

impl Pins {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.entries.get(key).map(|e| &e.sha)
    }

    pub fn set(&mut self, key: String, sha: String) {
        self.entries.insert(
            key,
            PinEntry {
                sha,
                resolved_at: chrono::Utc::now(),
            },
        );
    }
}

pub fn load() -> Result<Pins> {
    let path = paths::pins_file()?;
    load_from(&path)
}

pub fn load_from(path: &Path) -> Result<Pins> {
    if !path.exists() {
        return Ok(Pins::default());
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading `{}`", path.display()))?;
    if bytes.is_empty() {
        return Ok(Pins::default());
    }
    // Accept either the flat `{ "<key>": { "sha": "...", ... } }`
    // shape or the older inline-string `{ "<key>": "<sha>" }` shape.
    if let Ok(p) = serde_json::from_slice::<Pins>(&bytes) {
        return Ok(p);
    }
    let raw: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing `{}`", path.display()))?;
    let mut entries: BTreeMap<String, PinEntry> = BTreeMap::new();
    for (k, v) in raw {
        if let Some(s) = v.as_str() {
            entries.insert(
                k,
                PinEntry {
                    sha: s.to_string(),
                    resolved_at: chrono::Utc::now(),
                },
            );
        }
    }
    Ok(Pins { entries })
}

pub fn save(pins: &Pins) -> Result<()> {
    let path = paths::pins_file()?;
    save_to(&path, pins)
}

pub fn save_to(path: &Path, pins: &Pins) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating `{}`", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(pins).context("serializing pins.json")?;
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

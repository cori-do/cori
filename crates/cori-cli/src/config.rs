//! `~/.cori/config.toml` reader/writer.
//!
//! v1 keeps the config a flat `key.subkey = value` TOML document and exposes
//! it through `cori config get|set <dotted.key> [<value>]`. No schema —
//! Phase 4+ commands look up the values they care about.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use toml::Value;

use crate::paths;

pub struct Config {
    path: PathBuf,
    doc: Value,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&paths::config_file()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let doc = if path.exists() {
            let src = std::fs::read_to_string(path)
                .with_context(|| format!("reading `{}`", path.display()))?;
            src.parse::<Value>()
                .with_context(|| format!("parsing TOML in `{}`", path.display()))?
        } else {
            Value::Table(toml::map::Map::new())
        };
        Ok(Self {
            path: path.to_path_buf(),
            doc,
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        let s = toml::to_string_pretty(&self.doc).context("serializing config")?;
        std::fs::write(&self.path, s)
            .with_context(|| format!("writing `{}`", self.path.display()))?;
        Ok(())
    }

    /// Look up a dotted key like `temporal.host`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let mut cur = &self.doc;
        for segment in key.split('.') {
            cur = cur.as_table()?.get(segment)?;
        }
        Some(cur)
    }

    /// Set a dotted key to a string value. Creates intermediate tables.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let segments: Vec<&str> = key.split('.').collect();
        if segments.iter().any(|s| s.is_empty()) {
            bail!("invalid config key `{key}`");
        }
        let parsed = parse_value(value);
        let mut cur = &mut self.doc;
        for seg in &segments[..segments.len() - 1] {
            if !cur.is_table() {
                bail!("config key `{key}` overlaps with a non-table value");
            }
            let table = cur.as_table_mut().unwrap();
            let entry = table
                .entry((*seg).to_string())
                .or_insert_with(|| Value::Table(toml::map::Map::new()));
            if !entry.is_table() {
                *entry = Value::Table(toml::map::Map::new());
            }
            cur = entry;
        }
        let last = segments.last().unwrap();
        let table = cur
            .as_table_mut()
            .ok_or_else(|| anyhow!("config root is not a table"))?;
        table.insert((*last).to_string(), parsed);
        Ok(())
    }

    /// Flatten the document into dotted keys for `cori config get` with no
    /// argument. Used by future phases; kept here so the surface is one
    /// module.
    #[allow(dead_code)]
    pub fn flatten(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        flatten_into(&self.doc, String::new(), &mut out);
        out
    }
}

fn flatten_into(v: &Value, prefix: String, out: &mut BTreeMap<String, Value>) {
    match v {
        Value::Table(t) => {
            for (k, child) in t {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_into(child, key, out);
            }
        }
        _ => {
            out.insert(prefix, v.clone());
        }
    }
}

/// Best-effort coercion: try bool/int/float, fall back to a string.
fn parse_value(raw: &str) -> Value {
    if let Ok(b) = raw.parse::<bool>() {
        return Value::Boolean(b);
    }
    if let Ok(i) = raw.parse::<i64>() {
        return Value::Integer(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return Value::Float(f);
    }
    Value::String(raw.to_string())
}

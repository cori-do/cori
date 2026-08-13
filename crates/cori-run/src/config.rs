//! `~/.cori/config.toml` reader/writer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
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
            toml::from_str::<Value>(&src)
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

    /// Set a dotted key to a string value, coercing bool/int/float.
    /// Creates intermediate tables.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        self.set_value(key, parse_value(value))
    }

    /// Set a dotted key to an already-typed value. The path for values
    /// `set`'s string coercion can't express, such as arrays and tables.
    pub fn set_value(&mut self, key: &str, parsed: Value) -> Result<()> {
        let segments: Vec<&str> = key.split('.').collect();
        if segments.iter().any(|s| s.is_empty()) {
            bail!("invalid config key `{key}`");
        }
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

    /// Remove a dotted key when present. Empty parent tables are retained;
    /// they are harmless and keep this operation deliberately non-destructive.
    pub fn remove(&mut self, key: &str) -> Result<()> {
        let segments: Vec<&str> = key.split('.').collect();
        if segments.iter().any(|segment| segment.is_empty()) {
            bail!("invalid config key `{key}`");
        }
        let mut current = &mut self.doc;
        for segment in &segments[..segments.len() - 1] {
            let Some(next) = current
                .as_table_mut()
                .and_then(|table| table.get_mut(*segment))
            else {
                return Ok(());
            };
            current = next;
        }
        if let Some(table) = current.as_table_mut() {
            table.remove(*segments.last().unwrap());
        }
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_parses_nested_table_document() {
        let path = std::env::temp_dir().join(format!(
            "cori-config-nested-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, "[llm.openai]\napi_key = \"test-value\"\n").unwrap();

        let config = Config::load_from(&path).unwrap();
        assert_eq!(
            config.get("llm.openai.api_key").and_then(Value::as_str),
            Some("test-value")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn remove_clears_only_the_named_setting() {
        let path = std::env::temp_dir().join(format!(
            "cori-config-remove-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            "[llm]\nactive = \"cursor\"\n[llm.models.cursor]\nmedium = \"composer-2.5\"\n",
        )
        .unwrap();
        let mut config = Config::load_from(&path).unwrap();
        config.remove("llm.active").unwrap();
        assert!(config.get("llm.active").is_none());
        assert_eq!(
            config
                .get("llm.models.cursor.medium")
                .and_then(Value::as_str),
            Some("composer-2.5")
        );
        let _ = std::fs::remove_file(path);
    }
}

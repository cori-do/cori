//! `cori config get|set`.

use anyhow::Result;

use cori_run::config::Config;

pub fn get(key: Option<&str>) -> Result<()> {
    let cfg = Config::load()?;
    match key {
        Some(k) => match cfg.get(k) {
            Some(v) => {
                println!("{}", render_value(v));
            }
            None => {
                eprintln!("no config key `{k}`");
                std::process::exit(1);
            }
        },
        None => {
            for (k, v) in cfg.flatten() {
                println!("{k} = {}", render_value(&v));
            }
        }
    }
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let mut cfg = Config::load()?;
    cfg.set(key, value)?;
    cfg.save()?;
    println!("✓ {key} = {value}");
    Ok(())
}

fn render_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

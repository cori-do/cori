//! Manifest schema, parser, and shared workflow types.
//!
//! A Cori workflow's `manifest.md` has a YAML frontmatter block (between two
//! `---` delimiters) followed by a freeform Markdown prose body. This crate
//! parses the frontmatter into a strongly-typed [`Manifest`] and validates
//! every field. The prose body is preserved verbatim (so downstream tooling
//! can surface goals, preconditions, etc.) but is not interpreted here.
//!
//! Errors are reported as [`ManifestError`] values with `field`, `line`, and
//! a human-readable `reason` so that the CLI can surface them with
//! file:line locations.

use std::collections::HashSet;

use chrono::NaiveDate;
use cron::Schedule as CronSchedule;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A parsed, validated `manifest.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created: NaiveDate,
    pub version: u32,

    #[serde(default)]
    pub updated: Option<NaiveDate>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub tools_required: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub route_default: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub schedule_tz: Option<String>,

    /// Prose body following the frontmatter. Not interpreted by this crate.
    /// `skip_deserializing` is intentionally absent — JSON round-trips
    /// through the registry preserve it.
    #[serde(default)]
    pub body: String,
}

/// Closed set of parameter types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Number,
    Boolean,
    Enum,
    Path,
}

/// A workflow parameter declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParameterType,
    pub description: String,
    #[serde(default)]
    pub values: Option<Vec<YamlValue>>,
    #[serde(default)]
    pub default: Option<YamlValue>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// One structured validation failure.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Error)]
#[error("{field}: {reason}{}", line.map(|l| format!(" (line {l})")).unwrap_or_default())]
pub struct ManifestError {
    pub field: String,
    pub line: Option<usize>,
    pub reason: String,
}

impl ManifestError {
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            line: None,
            reason: reason.into(),
        }
    }
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Split a `manifest.md` source into (YAML frontmatter, prose body,
/// frontmatter start line for error translation).
///
/// Returns an error if the file does not start with a `---` delimiter or the
/// closing delimiter is missing.
pub fn split_frontmatter(source: &str) -> Result<(String, String, usize), ManifestError> {
    let trimmed = source.trim_start_matches('\u{feff}');
    let mut lines = trimmed.split_inclusive('\n');
    let first = lines
        .next()
        .ok_or_else(|| ManifestError::new("frontmatter", "manifest.md is empty").with_line(1))?;
    if first.trim_end() != "---" {
        return Err(ManifestError::new(
            "frontmatter",
            "manifest.md must start with a `---` frontmatter delimiter",
        )
        .with_line(1));
    }
    let frontmatter_start_line: usize = 2;
    let mut frontmatter = String::new();
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if !in_body && line.trim_end() == "---" {
            in_body = true;
            continue;
        }
        if in_body {
            body.push_str(line);
        } else {
            frontmatter.push_str(line);
        }
    }
    if !in_body {
        return Err(ManifestError::new(
            "frontmatter",
            "missing closing `---` delimiter",
        ));
    }
    Ok((frontmatter, body, frontmatter_start_line))
}

/// Parse a full `manifest.md` source string into a validated [`Manifest`].
///
/// Aggregates as many errors as possible before returning.
pub fn parse_manifest(source: &str) -> Result<Manifest, Vec<ManifestError>> {
    let (frontmatter, body, frontmatter_start_line) =
        split_frontmatter(source).map_err(|e| vec![e])?;

    let value: YamlValue = match serde_yaml::from_str(&frontmatter) {
        Ok(v) => v,
        Err(e) => {
            let line = e.location().map(|l| l.line() + frontmatter_start_line - 1);
            return Err(vec![ManifestError {
                field: "frontmatter".into(),
                line,
                reason: format!("invalid YAML: {e}"),
            }]);
        }
    };

    let mut manifest: Manifest = match serde_yaml::from_value(value.clone()) {
        Ok(m) => m,
        Err(e) => {
            return Err(vec![ManifestError {
                field: "frontmatter".into(),
                line: None,
                reason: format!("schema mismatch: {e}"),
            }]);
        }
    };
    manifest.body = body;

    let mut errors = Vec::new();
    validate(&manifest, &value, &mut errors);
    if errors.is_empty() {
        Ok(manifest)
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

const ID_MAX_LEN: usize = 64;

fn snake_case_re() -> Regex {
    Regex::new(r"^[a-z][a-z0-9_]*$").expect("static regex")
}

fn validate(m: &Manifest, raw: &YamlValue, errors: &mut Vec<ManifestError>) {
    let id_re = snake_case_re();

    if m.id.is_empty() {
        errors.push(ManifestError::new("id", "must not be empty"));
    } else if m.id.len() > ID_MAX_LEN {
        errors.push(ManifestError::new(
            "id",
            format!("must be at most {ID_MAX_LEN} characters"),
        ));
    } else if !id_re.is_match(&m.id) {
        errors.push(ManifestError::new(
            "id",
            "must be snake_case: [a-z][a-z0-9_]*",
        ));
    }

    if m.name.trim().is_empty() {
        errors.push(ManifestError::new("name", "must not be empty"));
    }
    if m.description.trim().is_empty() {
        errors.push(ManifestError::new("description", "must not be empty"));
    }
    if m.version == 0 {
        errors.push(ManifestError::new("version", "must be >= 1"));
    }

    if let Some(cron_str) = &m.schedule
        && cron_str.parse::<CronSchedule>().is_err()
    {
        // The `cron` crate requires 6 or 7 fields. Accept the standard
        // 5-field POSIX form by prefixing a seconds field.
        let augmented = format!("0 {cron_str}");
        if augmented.parse::<CronSchedule>().is_err() {
            errors.push(ManifestError::new(
                "schedule",
                format!("invalid cron expression: `{cron_str}`"),
            ));
        }
    }
    if let Some(tz) = &m.schedule_tz
        && tz.parse::<chrono_tz::Tz>().is_err()
    {
        errors.push(ManifestError::new(
            "schedule_tz",
            format!("unknown IANA timezone: `{tz}`"),
        ));
    }

    let mut seen = HashSet::new();
    for (idx, p) in m.parameters.iter().enumerate() {
        let prefix = format!("parameters[{idx}]");
        if p.name.trim().is_empty() {
            errors.push(ManifestError::new(
                format!("{prefix}.name"),
                "must not be empty",
            ));
        } else if !id_re.is_match(&p.name) {
            errors.push(ManifestError::new(
                format!("{prefix}.name"),
                "must be snake_case: [a-z][a-z0-9_]*",
            ));
        } else if !seen.insert(p.name.clone()) {
            errors.push(ManifestError::new(
                format!("{prefix}.name"),
                format!("duplicate parameter name: `{}`", p.name),
            ));
        }

        if p.description.trim().is_empty() {
            errors.push(ManifestError::new(
                format!("{prefix}.description"),
                "must not be empty",
            ));
        }

        match p.ty {
            ParameterType::Enum => {
                let ok = p.values.as_ref().is_some_and(|v| !v.is_empty());
                if !ok {
                    errors.push(ManifestError::new(
                        format!("{prefix}.values"),
                        "enum parameter requires a non-empty `values` list",
                    ));
                }
            }
            _ => {
                if p.values.is_some() {
                    errors.push(ManifestError::new(
                        format!("{prefix}.values"),
                        "`values` is only valid for enum parameters",
                    ));
                }
            }
        }

        if let (Some(min), Some(max)) = (p.min, p.max)
            && min > max
        {
            errors.push(ManifestError::new(
                format!("{prefix}.min"),
                format!("min ({min}) is greater than max ({max})"),
            ));
        }
    }

    for (i, t) in m.tools_required.iter().enumerate() {
        if t.trim().is_empty() {
            errors.push(ManifestError::new(
                format!("tools_required[{i}]"),
                "must not be empty",
            ));
        }
    }
    for (i, s) in m.mcp_servers.iter().enumerate() {
        if s.trim().is_empty() {
            errors.push(ManifestError::new(
                format!("mcp_servers[{i}]"),
                "must not be empty",
            ));
        }
    }
    for (i, s) in m.tags.iter().enumerate() {
        if s.trim().is_empty() {
            errors.push(ManifestError::new(
                format!("tags[{i}]"),
                "must not be empty",
            ));
        }
    }

    // Reject unknown top-level fields so typos surface early.
    if let YamlValue::Mapping(map) = raw {
        const KNOWN: &[&str] = &[
            "id",
            "name",
            "description",
            "created",
            "version",
            "updated",
            "parameters",
            "tools_required",
            "mcp_servers",
            "tags",
            "route_default",
            "schedule",
            "schedule_tz",
        ];
        for key in map.keys() {
            if let YamlValue::String(k) = key
                && !KNOWN.contains(&k.as_str())
            {
                errors.push(ManifestError::new(
                    format!("frontmatter.{k}"),
                    "unknown manifest field",
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_manifest() -> &'static str {
        "---\nid: hello_world\nname: Hello World\ndescription: A demo workflow.\ncreated: 2026-05-25\nversion: 1\n---\n\n# Hello World\n\n## Goal\nSay hi.\n"
    }

    #[test]
    fn parses_minimal_manifest() {
        let m = parse_manifest(ok_manifest()).unwrap();
        assert_eq!(m.id, "hello_world");
        assert_eq!(m.name, "Hello World");
        assert_eq!(m.version, 1);
        assert!(m.parameters.is_empty());
        assert!(m.body.contains("Say hi"));
    }

    #[test]
    fn parses_full_manifest() {
        let src = "---\nid: translate_product_sheets_fr\nname: Translate Product Sheets\ndescription: Localize EN to FR.\ncreated: 2026-05-24\nupdated: 2026-05-25\nversion: 2\nparameters:\n  - name: spreadsheet_id\n    type: string\n    description: Target spreadsheet\n    default: abc123\n  - name: target_env\n    type: enum\n    values: [dev, staging, prod]\n    default: staging\n    description: Which environment\n  - name: dry_run\n    type: boolean\n    default: false\n    required: false\n    description: Skip writes\ntools_required: [gws]\nmcp_servers: []\ntags: [translation]\nschedule: \"0 3 * * *\"\nschedule_tz: Europe/Paris\n---\n\n# body\n";
        let m = parse_manifest(src).unwrap();
        assert_eq!(m.id, "translate_product_sheets_fr");
        assert_eq!(m.parameters.len(), 3);
        assert_eq!(m.parameters[1].ty, ParameterType::Enum);
        assert_eq!(m.schedule.as_deref(), Some("0 3 * * *"));
        assert_eq!(m.schedule_tz.as_deref(), Some("Europe/Paris"));
    }

    #[test]
    fn missing_id_is_error() {
        let src = "---\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("missing field `id`")));
    }

    #[test]
    fn missing_closing_delimiter_is_error() {
        let errs = parse_manifest("---\nid: x\n").unwrap_err();
        assert_eq!(errs[0].field, "frontmatter");
        assert!(errs[0].reason.contains("missing closing"));
    }

    #[test]
    fn missing_opening_delimiter_is_error() {
        let errs = parse_manifest("id: x\n").unwrap_err();
        assert_eq!(errs[0].field, "frontmatter");
    }

    #[test]
    fn invalid_id_rejected() {
        let src =
            "---\nid: Hello-World\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "id"));
    }

    #[test]
    fn id_too_long_rejected() {
        let long = "a".repeat(100);
        let src = format!(
            "---\nid: {long}\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\n---\n"
        );
        let errs = parse_manifest(&src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.field == "id" && e.reason.contains("64"))
        );
    }

    #[test]
    fn version_zero_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 0\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "version"));
    }

    #[test]
    fn enum_without_values_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: env\n    type: enum\n    description: which env\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.ends_with(".values")));
    }

    #[test]
    fn duplicate_parameter_name_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: env\n    type: string\n    description: a\n  - name: env\n    type: string\n    description: b\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("duplicate")));
    }

    #[test]
    fn invalid_parameter_type_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: x\n    type: float\n    description: x\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(!errs.is_empty());
    }

    #[test]
    fn invalid_cron_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nschedule: \"not a cron\"\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "schedule"));
    }

    #[test]
    fn invalid_tz_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nschedule: \"0 3 * * *\"\nschedule_tz: Mars/Olympus_Mons\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "schedule_tz"));
    }

    #[test]
    fn unknown_field_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nmystery_field: 42\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.contains("mystery_field")));
    }

    #[test]
    fn values_on_non_enum_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: x\n    type: string\n    values: [a, b]\n    description: x\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.ends_with(".values")));
    }

    #[test]
    fn parameter_snake_case_enforced() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: BadName\n    type: string\n    description: x\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.ends_with(".name")));
    }

    #[test]
    fn empty_tools_string_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\ntools_required: [\"\"]\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "tools_required[0]"));
    }

    #[test]
    fn five_field_cron_accepted() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nschedule: \"0 3 * * *\"\n---\n";
        parse_manifest(src).unwrap();
    }

    #[test]
    fn empty_description_rejected() {
        let src = "---\nid: x\nname: x\ndescription: \"\"\ncreated: 2026-05-25\nversion: 1\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "description"));
    }

    #[test]
    fn bad_date_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: not-a-date\nversion: 1\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(!errs.is_empty());
    }

    #[test]
    fn body_preserved() {
        let m = parse_manifest(ok_manifest()).unwrap();
        assert!(m.body.starts_with("\n# Hello World"));
    }

    #[test]
    fn min_greater_than_max_rejected() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nparameters:\n  - name: n\n    type: number\n    description: a number\n    min: 10\n    max: 1\n---\n";
        let errs = parse_manifest(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.ends_with(".min")));
    }

    #[test]
    fn updated_optional() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nupdated: 2026-05-26\nversion: 2\n---\n";
        let m = parse_manifest(src).unwrap();
        assert_eq!(
            m.updated,
            Some(NaiveDate::from_ymd_opt(2026, 5, 26).unwrap())
        );
    }

    #[test]
    fn route_default_passthrough() {
        let src = "---\nid: x\nname: x\ndescription: y\ncreated: 2026-05-25\nversion: 1\nroute_default: worker\n---\n";
        let m = parse_manifest(src).unwrap();
        assert_eq!(m.route_default.as_deref(), Some("worker"));
    }
}

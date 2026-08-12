//! Resolve an authored manifest result against the state available after a run.
//!
//! This deliberately runs after Temporal returns. It reconstructs the workflow's
//! shallow object-merge accumulator from persisted activity outputs, so result
//! presentation remains outside deterministic workflow code and the broker.

use cori_manifest::{
    ResultDeclaration, ResultFieldDeclaration, ResultFieldFormat, ResultSectionDeclaration,
    ResultSectionDisplay,
};
use cori_protocol::{
    ActivityTrace, ResolvedResult, ResolvedResultArtifact, ResolvedResultField,
    ResolvedResultSection, ResultIssue, ResultIssueKind,
};
use serde_json::Value as JsonValue;
use url::Url;

/// Resolve a declaration, returning `None` only when the workflow did not
/// declare a result. Resolution issues are data and never fail the run.
pub fn resolve_result(
    declaration: Option<&ResultDeclaration>,
    params: &JsonValue,
    activities: &[ActivityTrace],
    dry_run: bool,
) -> Option<ResolvedResult> {
    let declaration = declaration?;
    let state = reconstruct_final_state(params, activities, dry_run);
    let mut issues = Vec::new();

    let headline = match render_template(&declaration.headline, &state) {
        Ok(value) => value,
        Err(error) => {
            issues.push(error.issue("headline"));
            String::new()
        }
    };
    let description = declaration.description.as_ref().and_then(|template| {
        match render_template(template, &state) {
            Ok(value) => Some(value),
            Err(error) => {
                issues.push(error.issue("description"));
                None
            }
        }
    });

    let fields = declaration
        .fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| resolve_field(index, field, &state, &mut issues))
        .collect();
    let sections = declaration
        .sections
        .iter()
        .enumerate()
        .filter_map(|(index, section)| resolve_section(index, section, &state, &mut issues))
        .collect();
    let artifacts = declaration
        .artifacts
        .iter()
        .enumerate()
        .filter_map(|(index, artifact)| {
            let item = format!("artifacts[{index}]");
            let rendered = match render_template(&artifact.url, &state) {
                Ok(value) => value,
                Err(ResolveError::Missing { .. }) if !artifact.required => return None,
                Err(error) => {
                    issues.push(error.issue(item));
                    return None;
                }
            };
            match Url::parse(&rendered) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {
                    Some(ResolvedResultArtifact {
                        label: artifact.label.clone(),
                        url: url.to_string(),
                    })
                }
                _ => {
                    issues.push(ResultIssue {
                        item,
                        kind: ResultIssueKind::InvalidUrl,
                        message: format!(
                            "resolved URL is not an absolute HTTP(S) URL: `{rendered}`"
                        ),
                    });
                    None
                }
            }
        })
        .collect();

    Some(ResolvedResult {
        headline,
        description,
        fields,
        sections,
        artifacts,
        issues,
    })
}

/// Rebuild the dataflow accumulator: parameters first, then each successful
/// object output shallow-merged in workflow order. Dry-run stubs marked
/// `skipped` participate exactly as they do inside `CoriWorkflow`.
pub fn reconstruct_final_state(
    params: &JsonValue,
    activities: &[ActivityTrace],
    dry_run: bool,
) -> JsonValue {
    let mut state = params.as_object().cloned().unwrap_or_default();
    for activity in activities {
        if (activity.status == "ok" || (dry_run && activity.status == "skipped"))
            && let JsonValue::Object(output) = &activity.output
        {
            for (key, value) in output {
                state.insert(key.clone(), value.clone());
            }
        }
    }
    JsonValue::Object(state)
}

fn resolve_field(
    index: usize,
    field: &ResultFieldDeclaration,
    state: &JsonValue,
    issues: &mut Vec<ResultIssue>,
) -> Option<ResolvedResultField> {
    let item = format!("fields[{index}]");
    let value = match resolve_path(state, &field.path) {
        Ok(value) => value.clone(),
        Err(ResolveError::Missing { .. }) if !field.required => return None,
        Err(error) => {
            issues.push(error.issue(item));
            return None;
        }
    };

    let valid = match field.format {
        ResultFieldFormat::Auto => is_scalar(&value),
        ResultFieldFormat::Number
        | ResultFieldFormat::Currency
        | ResultFieldFormat::Percent
        | ResultFieldFormat::Duration => value.is_number(),
    };
    if !valid {
        issues.push(ResultIssue {
            item,
            kind: ResultIssueKind::TypeMismatch,
            message: format!(
                "`{}` does not contain a value compatible with {:?}",
                field.path, field.format
            ),
        });
        return None;
    }

    Some(ResolvedResultField {
        label: field.label.clone(),
        value,
        format: field.format,
        currency: field.currency.clone(),
        tone: field.tone,
    })
}

fn resolve_section(
    index: usize,
    section: &ResultSectionDeclaration,
    state: &JsonValue,
    issues: &mut Vec<ResultIssue>,
) -> Option<ResolvedResultSection> {
    let item = format!("sections[{index}]");
    let value = match resolve_path(state, &section.path) {
        Ok(value) => value.clone(),
        Err(ResolveError::Missing { .. }) if !section.required => return None,
        Err(error) => {
            issues.push(error.issue(item));
            return None;
        }
    };
    let valid = match section.display {
        ResultSectionDisplay::Auto => !value.is_null(),
        ResultSectionDisplay::Text => value.is_string(),
        ResultSectionDisplay::List => value.is_array(),
        ResultSectionDisplay::Table => value
            .as_array()
            .is_some_and(|rows| rows.iter().all(|row| row.is_object() || row.is_array())),
    };
    if !valid {
        issues.push(ResultIssue {
            item,
            kind: ResultIssueKind::TypeMismatch,
            message: format!(
                "`{}` does not contain a value compatible with {:?}",
                section.path, section.display
            ),
        });
        return None;
    }
    Some(ResolvedResultSection {
        label: section.label.clone(),
        value,
        display: section.display,
    })
}

fn resolve_path<'a>(state: &'a JsonValue, path: &str) -> Result<&'a JsonValue, ResolveError> {
    let mut current = state;
    for segment in path.split('.') {
        current = match current {
            JsonValue::Object(map) => map.get(segment).ok_or_else(|| ResolveError::Missing {
                path: path.to_string(),
            })?,
            JsonValue::Array(values) => {
                let index = segment
                    .parse::<usize>()
                    .map_err(|_| ResolveError::TypeMismatch {
                        path: path.to_string(),
                    })?;
                values.get(index).ok_or_else(|| ResolveError::Missing {
                    path: path.to_string(),
                })?
            }
            _ => {
                return Err(ResolveError::TypeMismatch {
                    path: path.to_string(),
                });
            }
        };
    }
    if current.is_null() {
        Err(ResolveError::Missing {
            path: path.to_string(),
        })
    } else {
        Ok(current)
    }
}

fn render_template(template: &str, state: &JsonValue) -> Result<String, ResolveError> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("}}") else {
            // Authored manifests are validated, so this is only reachable for
            // a malformed legacy compiled cache.
            return Err(ResolveError::TypeMismatch {
                path: "template".into(),
            });
        };
        let path = after_open[..end].trim();
        let value = resolve_path(state, path)?;
        let scalar = scalar_text(value).ok_or_else(|| ResolveError::NonScalar {
            path: path.to_string(),
        })?;
        rendered.push_str(&scalar);
        rest = &after_open[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn is_scalar(value: &JsonValue) -> bool {
    matches!(
        value,
        JsonValue::String(_) | JsonValue::Number(_) | JsonValue::Bool(_)
    )
}

fn scalar_text(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(value) => Some(value.clone()),
        JsonValue::Number(value) => Some(value.to_string()),
        JsonValue::Bool(value) => Some(value.to_string()),
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => None,
    }
}

#[derive(Debug)]
enum ResolveError {
    Missing { path: String },
    TypeMismatch { path: String },
    NonScalar { path: String },
}

impl ResolveError {
    fn issue(self, item: impl Into<String>) -> ResultIssue {
        let item = item.into();
        match self {
            Self::Missing { path } => ResultIssue {
                item,
                kind: ResultIssueKind::MissingValue,
                message: format!("required result path `{path}` was not available"),
            },
            Self::TypeMismatch { path } => ResultIssue {
                item,
                kind: ResultIssueKind::TypeMismatch,
                message: format!("result path `{path}` traversed an incompatible value"),
            },
            Self::NonScalar { path } => ResultIssue {
                item,
                kind: ResultIssueKind::NonScalarTemplate,
                message: format!("template path `{path}` resolved to an object or array"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cori_manifest::{ResultArtifactDeclaration, ResultFieldTone, ResultSectionDisplay};
    use cori_protocol::StepKind;
    use serde_json::json;

    fn activity(status: &str, output: JsonValue) -> ActivityTrace {
        let now = Utc::now();
        ActivityTrace {
            activity_id: "01_step".into(),
            step_name: "step".into(),
            kind: StepKind::Code,
            status: status.into(),
            started_at: now,
            ended_at: now,
            duration_ms: 1,
            attempts: 1,
            route: None,
            task_queue: None,
            worker_identity: None,
            input_summary: JsonValue::Null,
            output_summary: JsonValue::Null,
            output,
            cost_eur: None,
            tokens: None,
            error: None,
            notes: None,
        }
    }

    fn declaration() -> ResultDeclaration {
        ResultDeclaration {
            headline: "{{ account.name }} · {{ summary.count }} rows".into(),
            description: None,
            fields: vec![ResultFieldDeclaration {
                label: "Count".into(),
                path: "summary.count".into(),
                format: ResultFieldFormat::Number,
                currency: None,
                tone: ResultFieldTone::Neutral,
                required: true,
            }],
            sections: vec![ResultSectionDeclaration {
                label: "Rows".into(),
                path: "rows".into(),
                display: ResultSectionDisplay::Table,
                required: true,
            }],
            artifacts: vec![],
        }
    }

    #[test]
    fn resolves_parameters_nested_paths_and_later_output_precedence() {
        let activities = vec![
            activity(
                "ok",
                json!({ "summary": { "count": 2 }, "rows": [{"id": 1}] }),
            ),
            activity(
                "ok",
                json!({ "summary": { "count": 3 }, "rows": [{"id": 2}] }),
            ),
        ];
        let result = resolve_result(
            Some(&declaration()),
            &json!({ "account": { "name": "Acme" } }),
            &activities,
            false,
        )
        .unwrap();
        assert_eq!(result.headline, "Acme · 3 rows");
        assert_eq!(result.fields[0].value, json!(3));
        assert_eq!(result.sections[0].value, json!([{"id": 2}]));
    }

    #[test]
    fn dry_run_stubs_participate_in_resolution() {
        let result = resolve_result(
            Some(&declaration()),
            &json!({ "account": { "name": "Dry" } }),
            &[activity(
                "skipped",
                json!({ "summary": { "count": 0 }, "rows": [] }),
            )],
            true,
        )
        .unwrap();
        assert_eq!(result.headline, "Dry · 0 rows");
        assert!(result.issues.is_empty());
    }

    #[test]
    fn optional_missing_items_are_omitted_and_required_issues_are_nonfatal() {
        let mut declaration = declaration();
        declaration.fields[0].required = false;
        declaration.sections[0].path = "missing_rows".into();
        let result = resolve_result(
            Some(&declaration),
            &json!({ "account": { "name": "Acme" }, "summary": { "count": null } }),
            &[],
            false,
        )
        .unwrap();
        assert!(result.fields.is_empty());
        assert!(result.sections.is_empty());
        assert_eq!(result.issues.len(), 2); // headline + required section
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.kind == ResultIssueKind::MissingValue)
        );
    }

    #[test]
    fn records_non_scalar_templates_type_mismatches_and_invalid_urls() {
        let mut declaration = declaration();
        declaration.headline = "{{ rows }}".into();
        declaration.fields[0].path = "rows".into();
        declaration.sections.clear();
        declaration.artifacts = vec![ResultArtifactDeclaration {
            label: "Open".into(),
            url: "https://{{ bad_path }}/report".into(),
            required: true,
        }];
        let result = resolve_result(
            Some(&declaration),
            &json!({ "rows": [1], "bad_path": ":" }),
            &[],
            false,
        )
        .unwrap();
        assert!(result.headline.is_empty());
        assert!(result.fields.is_empty());
        assert!(result.artifacts.is_empty());
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.kind == ResultIssueKind::NonScalarTemplate)
        );
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.kind == ResultIssueKind::TypeMismatch)
        );
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.kind == ResultIssueKind::InvalidUrl)
        );
    }

    #[test]
    fn failed_activity_does_not_hide_available_earlier_outputs() {
        let activities = vec![
            activity(
                "ok",
                json!({ "summary": { "count": 1 }, "rows": [{"id": 1}] }),
            ),
            activity("failed", JsonValue::Null),
        ];
        let result = resolve_result(
            Some(&declaration()),
            &json!({ "account": { "name": "Partial" } }),
            &activities,
            false,
        )
        .unwrap();
        assert_eq!(result.headline, "Partial · 1 rows");
        assert!(result.issues.is_empty());
    }
}

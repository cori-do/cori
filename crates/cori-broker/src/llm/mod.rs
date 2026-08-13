//! Dispatch an `llm` step.
//!
//! The LLM dispatch path follows this flow:
//!
//! 1. Invoke the runner in `llm_prompt` mode to materialise the prompt
//!    string, the declared level, the (optional) batch config,
//!    and the (optional) output JSON Schema.
//! 2. Resolve a backend ([`resolve`]) from what the step asked for, the
//!    user's policy ([`policy`]), and what is usable on this machine —
//!    a signed-in subscription CLI ([`subscription`]) or a metered API
//!    key ([`providers`]).
//! 3. If `batch` was declared and the input has an array under the named
//!    field, split into chunks of `batch.size`, fan out to N parallel
//!    threads (default concurrency 4), and merge results.
//! 4. Validate every response against the output schema. Retry once with
//!    a stricter system message on schema-validation failure.
//! 5. Record cost via [`pricing::cost_eur`] and return an
//!    [`ActivityOutcome`] whose `output` is the (merged) parsed JSON and
//!    whose notes name the backend that actually answered.
//!
//! # What a step declares
//!
//! A step declares `level: "low" | "medium" | "high"`; omission means
//! `medium`. The machine's single active provider maps that level to a
//! concrete model. Workflows never select a provider or model.

pub mod catalog;
pub mod credentials;
pub mod policy;
mod pricing;
pub mod providers;
pub mod resolve;
pub mod subscription;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result, TokenUsage};

pub use catalog::LlmLevel;
pub use credentials::LlmCredentials;
pub use policy::{Deployment, LlmConfig, LlmPolicy};

/// Default fan-out for batched LLM calls.
const DEFAULT_BATCH_CONCURRENCY: usize = 4;

/// Options the CLI assembles per-run and passes to every LLM step.
#[derive(Debug, Clone, Default)]
pub struct LlmOptions {
    pub credentials: LlmCredentials,
    /// The single active backend. Built from `~/.cori/config.toml` and
    /// gated by the worker's identity — see [`policy`].
    pub policy: LlmPolicy,
}

#[derive(Debug, Deserialize)]
struct PromptSpec {
    #[serde(default)]
    level: Option<String>,
    /// Runtime-only compatibility with already-started Temporal histories.
    /// New workflow source containing `model` is rejected by the compiler.
    #[serde(default, rename = "legacyModel")]
    legacy_model: Option<String>,
    prompt: String,
    #[serde(default, rename = "batchPrompts")]
    batch_prompts: Vec<String>,
    #[serde(default)]
    batch: Option<BatchSpec>,
    #[serde(default, rename = "outputSchema")]
    output_schema: Option<JsonValue>,
    #[serde(default, rename = "hasOutputSchema")]
    has_output_schema: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct BatchSpec {
    by: String,
}

/// Run one `llm` step.
pub fn run(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    opts: &LlmOptions,
    expected_level: &ExpectedLevel,
) -> Result<ActivityOutcome> {
    let started = Instant::now();

    // The runner parses the input and renders every batch prompt in one
    // process, preserving transformed values that cannot cross JSON intact.
    let initial =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::LlmPrompt, input)?;
    let spec: PromptSpec =
        serde_json::from_value(initial.output.clone()).map_err(|e| BrokerError::BadEnvelope {
            envelope: initial.output.to_string(),
            source: e,
        })?;
    let level = validate_level_boundary(
        expected_level,
        spec.level.as_deref(),
        spec.legacy_model.as_deref(),
    )?;

    // Resolve the workflow level through exactly the active provider.
    let resolution = resolve::resolve(level, &opts.policy, &opts.credentials)?;
    let provider = &*resolution.provider;
    let wire_model = resolution.selection.resolved_model.clone();
    let output_schema = spec.output_schema.as_ref();

    let mut combined_stderr = initial.stderr;

    let (text_responses, total_usage) = if spec.batch_prompts.is_empty() {
        // No batching — single call, reuse the prompt we already rendered.
        let req = providers::LlmRequest {
            model: &wire_model,
            prompt: &spec.prompt,
            output_schema,
            strict_retry: false,
        };
        let resp = call_with_schema_retry(
            provider,
            &req,
            runtime,
            step_file_path,
            spec.has_output_schema,
        )?;
        (vec![resp.text], resp.usage)
    } else {
        // The runner renders every chunk immediately after parsing the input,
        // before transformed values cross the JSON process boundary. Parallel
        // provider dispatch can then use those frozen prompt strings directly.
        fan_out(
            provider,
            &wire_model,
            &spec.batch_prompts,
            output_schema,
            runtime,
            step_file_path,
            spec.has_output_schema,
        )?
    };

    // Validate + merge.
    let outputs: Result<Vec<JsonValue>> = text_responses
        .iter()
        .map(|t| parse_and_validate(t, output_schema, spec.has_output_schema, provider.name()))
        .collect();
    let outputs = outputs?;

    let final_output = if outputs.len() == 1 {
        outputs
            .into_iter()
            .next()
            .ok_or_else(|| BrokerError::StepFailed {
                message: "LLM provider produced no output".to_string(),
                stack: None,
            })?
    } else {
        let batch = spec.batch.as_ref().ok_or_else(|| BrokerError::StepFailed {
            message: "runner returned multiple LLM batch prompts without batch metadata"
                .to_string(),
            stack: None,
        })?;
        merge_batched_outputs(outputs, &batch.by)
    };
    let validated = dispatch::invoke_validate_output(runtime, step_file_path, &final_output)?;
    if !validated.stderr.trim().is_empty() {
        if !combined_stderr.is_empty() && !combined_stderr.ends_with('\n') {
            combined_stderr.push('\n');
        }
        combined_stderr.push_str(&validated.stderr);
    }

    // Subscription calls are covered by a flat fee the user already
    // paid, so `cost_model()` returns `None` for them and the run is not
    // charged API rates for tokens that cost nothing extra.
    let cost = resolution.cost_model().and_then(|model| {
        pricing::cost_eur(model, total_usage.input_tokens, total_usage.output_tokens)
    });

    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: validated.output,
        duration: started.elapsed(),
        stderr: combined_stderr,
        cost_eur: cost,
        usage: Some(total_usage),
        notes: vec![resolution.trace_note()],
    })
}

/// Enforce the frozen authoring boundary and adapt already-started histories.
pub(crate) fn validate_level_boundary(
    expected: &ExpectedLevel,
    actual_level: Option<&str>,
    actual_legacy_model: Option<&str>,
) -> Result<LlmLevel> {
    match expected {
        ExpectedLevel::Unknown => match (actual_level, actual_legacy_model) {
            (Some(level), _) => parse_runtime_level(level),
            (None, Some(model)) => Ok(LlmLevel::from_legacy_model(model)),
            (None, None) => Err(BrokerError::CapabilityDenied {
                kind: "LLM level",
                name: "missing".into(),
                hint: "the LLM step did not produce a level".into(),
            }),
        },
        ExpectedLevel::Declared(level) => {
            let actual = actual_level.map(parse_runtime_level).transpose()?;
            if actual == Some(*level) && actual_legacy_model.is_none() {
                Ok(*level)
            } else {
                Err(level_boundary_error(
                    level.as_str(),
                    actual_level.or(actual_legacy_model),
                ))
            }
        }
        ExpectedLevel::LegacyModel(model) => {
            if actual_legacy_model == Some(model.as_str()) {
                Ok(LlmLevel::from_legacy_model(model))
            } else {
                Err(level_boundary_error(
                    model,
                    actual_legacy_model.or(actual_level),
                ))
            }
        }
    }
}

fn parse_runtime_level(value: &str) -> Result<LlmLevel> {
    LlmLevel::parse(value).ok_or_else(|| BrokerError::CapabilityDenied {
        kind: "LLM level",
        name: value.to_string(),
        hint: "level must be one of `low`, `medium`, or `high`".into(),
    })
}

fn level_boundary_error(expected: &str, actual: Option<&str>) -> BrokerError {
    let actual = actual.unwrap_or("missing");
    BrokerError::CapabilityDenied {
        kind: "LLM level",
        name: actual.to_string(),
        hint: format!(
            "step was compiled for `{expected}` but runtime evaluation produced `{actual}`; level must remain the directly declared literal"
        ),
    }
}

/// What the compiler froze for an LLM step. `LegacyModel` exists only so
/// already-started Temporal histories can replay after this breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedLevel {
    Unknown,
    Declared(LlmLevel),
    LegacyModel(String),
}

impl ExpectedLevel {
    pub fn from_frozen(
        frozen_metadata: Option<&serde_json::Map<String, JsonValue>>,
    ) -> Result<Self> {
        let Some(metadata) = frozen_metadata else {
            return Ok(Self::Unknown);
        };
        if let Some(level) = metadata.get("level").and_then(JsonValue::as_str) {
            return Ok(Self::Declared(parse_runtime_level(level)?));
        }
        if let Some(model) = metadata.get("model").and_then(JsonValue::as_str) {
            return Ok(Self::LegacyModel(model.to_string()));
        }
        Ok(Self::Unknown)
    }
}

/// Fan out one provider call per prompt across worker threads.
fn fan_out(
    provider: &dyn providers::LlmProvider,
    model: &str,
    prompts: &[String],
    output_schema: Option<&JsonValue>,
    runtime: &Runtime,
    step_file_path: &Path,
    has_output_schema: bool,
) -> Result<(Vec<String>, TokenUsage)> {
    let concurrency = DEFAULT_BATCH_CONCURRENCY.min(prompts.len().max(1));
    let results: Arc<Mutex<Vec<Option<Result<providers::LlmResponse>>>>> =
        Arc::new(Mutex::new((0..prompts.len()).map(|_| None).collect()));
    let next_idx: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    // Cheap pre-clones so the workers don't borrow non-Send refs. The
    // provider is &dyn — wrap it in a `scoped` thread group.
    thread::scope(|scope| {
        for _ in 0..concurrency {
            let results = results.clone();
            let next_idx = next_idx.clone();
            scope.spawn(move || {
                loop {
                    let i = {
                        let mut g = next_idx.lock().unwrap();
                        if *g >= prompts.len() {
                            return;
                        }
                        let i = *g;
                        *g += 1;
                        i
                    };
                    let req = providers::LlmRequest {
                        model,
                        prompt: &prompts[i],
                        output_schema,
                        strict_retry: false,
                    };
                    let r = call_with_schema_retry(
                        provider,
                        &req,
                        runtime,
                        step_file_path,
                        has_output_schema,
                    );
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });

    let mut text = Vec::with_capacity(prompts.len());
    let mut total = TokenUsage::default();
    let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    for slot in results.drain(..) {
        let resp = slot.expect("every worker fills its slot")?;
        text.push(resp.text);
        total = total + resp.usage;
    }
    Ok((text, total))
}

/// Call the provider, retrying once with `strict_retry: true` when a declared
/// output schema rejects the response. The runner performs the same Zod parse
/// used at the activity boundary, so syntactically valid but shape-invalid JSON
/// receives the strict retry too.
fn call_with_schema_retry(
    provider: &dyn providers::LlmProvider,
    req: &providers::LlmRequest<'_>,
    runtime: &Runtime,
    step_file_path: &Path,
    has_output_schema: bool,
) -> Result<providers::LlmResponse> {
    let first = provider.complete(req)?;
    if !has_output_schema {
        return Ok(first);
    }
    if candidate_schema_error(runtime, step_file_path, &first.text)?.is_none() {
        return Ok(first);
    }
    // Retry once with a stricter system message.
    let retry_req = providers::LlmRequest {
        strict_retry: true,
        ..providers::LlmRequest {
            model: req.model,
            prompt: req.prompt,
            output_schema: req.output_schema,
            strict_retry: true,
        }
    };
    let second = provider.complete(&retry_req)?;
    if let Some(reason) = candidate_schema_error(runtime, step_file_path, &second.text)? {
        return Err(BrokerError::LlmSchemaMismatch {
            provider: provider.name(),
            attempts: 2,
            reason,
        });
    }
    // Sum usage across both attempts so cost accounting reflects reality.
    Ok(providers::LlmResponse {
        text: second.text,
        usage: first.usage + second.usage,
    })
}

fn candidate_schema_error(
    runtime: &Runtime,
    step_file_path: &Path,
    text: &str,
) -> Result<Option<String>> {
    let candidate = match serde_json::from_str::<JsonValue>(strip_json_fences(text)) {
        Ok(candidate) => candidate,
        Err(error) => return Ok(Some(format!("response was not valid JSON: {error}"))),
    };
    match dispatch::invoke_validate_output(runtime, step_file_path, &candidate) {
        Ok(_) => Ok(None),
        Err(BrokerError::SchemaValidation { message, .. }) => Ok(Some(message)),
        Err(error) => Err(error),
    }
}

/// Parse the LLM's text. With a schema, expect JSON; without one, return
/// the raw string (downstream `code` steps can do their own parsing).
fn parse_and_validate(
    text: &str,
    schema: Option<&JsonValue>,
    has_schema: bool,
    provider: &'static str,
) -> Result<JsonValue> {
    if !has_schema {
        return Ok(JsonValue::String(text.to_string()));
    }
    let stripped = strip_json_fences(text);
    let parsed: JsonValue =
        serde_json::from_str(stripped).map_err(|e| BrokerError::LlmSchemaMismatch {
            provider,
            attempts: 2,
            reason: format!("response was not valid JSON: {e}"),
        })?;
    // Very light shape check: if schema declares `type: object` and root
    // is not an object (or `type: array` and root is not an array), fail.
    if let Some(JsonValue::String(t)) = schema.and_then(|s| s.get("type")) {
        match t.as_str() {
            "object" if !parsed.is_object() => {
                return Err(BrokerError::LlmSchemaMismatch {
                    provider,
                    attempts: 2,
                    reason: "expected a JSON object at the root".to_string(),
                });
            }
            "array" if !parsed.is_array() => {
                return Err(BrokerError::LlmSchemaMismatch {
                    provider,
                    attempts: 2,
                    reason: "expected a JSON array at the root".to_string(),
                });
            }
            _ => {}
        }
    }
    Ok(parsed)
}

/// Merge per-chunk outputs back into a single result. The convention:
/// every chunk's output is an object; arrays under the same keys are
/// concatenated; for non-array fields the first chunk's value wins.
///
/// Special case: if every chunk's output is itself an array, return the
/// concatenated array.
fn merge_batched_outputs(outputs: Vec<JsonValue>, _batch_field: &str) -> JsonValue {
    if outputs.iter().all(|o| o.is_array()) {
        let mut all = Vec::new();
        for o in outputs {
            if let JsonValue::Array(mut a) = o {
                all.append(&mut a);
            }
        }
        return JsonValue::Array(all);
    }
    let mut merged: JsonMap<String, JsonValue> = JsonMap::new();
    for out in outputs {
        let JsonValue::Object(obj) = out else {
            continue;
        };
        for (k, v) in obj {
            match merged.get_mut(&k) {
                Some(JsonValue::Array(existing)) => {
                    if let JsonValue::Array(more) = v {
                        existing.extend(more);
                    }
                }
                Some(_) => { /* keep first */ }
                None => {
                    merged.insert(k, v);
                }
            }
        }
    }
    JsonValue::Object(merged)
}

/// Strip leading/trailing ```json … ``` fences if the model decided to
/// wrap the response despite our instructions.
fn strip_json_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t)
        .trim_start();
    t.strip_suffix("```").unwrap_or(t).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_runtime_level_switches() {
        let expected = ExpectedLevel::Declared(LlmLevel::Medium);
        assert_eq!(
            validate_level_boundary(&expected, Some("medium"), None).expect("matching level"),
            LlmLevel::Medium
        );
        let error = validate_level_boundary(&expected, Some("high"), None)
            .expect_err("runtime level switch must fail");
        assert!(matches!(
            error,
            BrokerError::CapabilityDenied { kind: "LLM level", name, .. } if name == "high"
        ));
    }

    #[test]
    fn expected_level_reads_new_and_legacy_metadata() {
        assert_eq!(
            ExpectedLevel::from_frozen(None).expect("unknown"),
            ExpectedLevel::Unknown
        );
        let mut new_metadata = JsonMap::new();
        new_metadata.insert("level".into(), json!("high"));
        assert_eq!(
            ExpectedLevel::from_frozen(Some(&new_metadata)).expect("level"),
            ExpectedLevel::Declared(LlmLevel::High)
        );
        let mut metadata = JsonMap::new();
        metadata.insert("model".into(), json!("fast"));
        assert_eq!(
            ExpectedLevel::from_frozen(Some(&metadata)).expect("legacy model"),
            ExpectedLevel::LegacyModel("fast".into())
        );
    }

    #[test]
    fn legacy_history_model_maps_to_a_level() {
        let expected = ExpectedLevel::LegacyModel("gpt-4o-mini".into());
        assert_eq!(
            validate_level_boundary(&expected, None, Some("gpt-4o-mini"))
                .expect("legacy compatibility"),
            LlmLevel::Low
        );
    }

    #[test]
    fn prompt_spec_accepts_level_and_pre_rendered_batch_prompts() {
        let spec: PromptSpec = serde_json::from_value(json!({
            "level": "low",
            "legacyModel": null,
            "prompt": "",
            "batch": { "by": "rows", "size": 2 },
            "batchPrompts": ["rows 1-2", "row 3"],
            "outputSchema": null,
            "hasOutputSchema": false
        }))
        .expect("prompt spec");
        assert_eq!(spec.level.as_deref(), Some("low"));
        assert_eq!(spec.batch_prompts, vec!["rows 1-2", "row 3"]);
        assert_eq!(spec.batch.expect("batch").by, "rows");
    }

    #[test]
    fn prompt_spec_accepts_the_normalized_medium_default() {
        let spec: PromptSpec = serde_json::from_value(json!({
            "level": "medium",
            "legacyModel": null,
            "prompt": "summarise",
            "batchPrompts": [],
            "outputSchema": null,
            "hasOutputSchema": false
        }))
        .expect("prompt spec");
        assert_eq!(
            validate_level_boundary(
                &ExpectedLevel::Declared(LlmLevel::Medium),
                spec.level.as_deref(),
                spec.legacy_model.as_deref(),
            )
            .expect("default level"),
            LlmLevel::Medium
        );
    }

    #[test]
    fn merge_object_outputs_concatenates_arrays() {
        let outputs = vec![
            json!({ "translations": [1, 2] }),
            json!({ "translations": [3] }),
        ];
        let merged = merge_batched_outputs(outputs, "translations");
        assert_eq!(merged, json!({ "translations": [1, 2, 3] }));
    }

    #[test]
    fn strips_json_fences() {
        assert_eq!(strip_json_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_json_fences("{\"a\":1}"), "{\"a\":1}");
    }
}

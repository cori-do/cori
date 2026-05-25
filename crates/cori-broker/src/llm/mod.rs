//! Dispatch an `llm` step.
//!
//! The LLM dispatch path follows this flow:
//!
//! 1. Invoke the runner in `llm_prompt` mode to materialise the prompt
//!    string, the model name, the (optional) batch config, and the
//!    (optional) output JSON Schema.
//! 2. Pick a [`providers::LlmProvider`] from the model-name prefix.
//! 3. Resolve credentials for the provider (env > config > interactive
//!    prompt).
//! 4. If `batch` was declared and the input has an array under the named
//!    field, split into chunks of `batch.size`, fan out to N parallel
//!    threads (default concurrency 4), and merge results.
//! 5. Validate every response against the output schema. Retry once with
//!    a stricter system message on schema-validation failure.
//! 6. Record cost via [`pricing::cost_eur`] and return an
//!    [`ActivityOutcome`] whose `output` is the (merged) parsed JSON.

pub mod credentials;
mod pricing;
mod providers;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result, TokenUsage, TriggerContext};

pub use credentials::LlmCredentials;

/// Default fan-out for batched LLM calls.
const DEFAULT_BATCH_CONCURRENCY: usize = 4;

/// Options the CLI assembles per-run and passes to every LLM step.
#[derive(Debug, Clone, Default)]
pub struct LlmOptions {
    pub credentials: LlmCredentials,
    pub trigger: Option<TriggerContext>,
}

#[derive(Debug, Deserialize)]
struct PromptSpec {
    model: String,
    prompt: String,
    #[serde(default)]
    batch: Option<BatchSpec>,
    #[serde(default, rename = "outputSchema")]
    output_schema: Option<JsonValue>,
    #[serde(default, rename = "hasOutputSchema")]
    has_output_schema: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct BatchSpec {
    size: usize,
    by: String,
}

/// Run one `llm` step.
pub fn run(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    opts: &LlmOptions,
) -> Result<ActivityOutcome> {
    let started = Instant::now();

    // For batched calls, we need to invoke `llm_prompt` once per chunk
    // (the prompt closes over `input` so the user's `prompt(input)`
    // function decides how to render each chunk). We start with one call
    // to read `model`, `batch`, and `outputSchema`.
    let initial =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::LlmPrompt, input)?;
    let spec: PromptSpec =
        serde_json::from_value(initial.output.clone()).map_err(|e| BrokerError::BadEnvelope {
            envelope: initial.output.to_string(),
            source: e,
        })?;

    let provider = pick_provider(&spec.model, &opts.credentials)?;
    let output_schema = spec.output_schema.as_ref();

    // Decide whether to batch.
    let chunks = decide_chunks(input, spec.batch.as_ref());
    let mut combined_stderr = initial.stderr;

    let (text_responses, total_usage) = if chunks.is_empty() {
        // No batching — single call, reuse the prompt we already rendered.
        let req = providers::LlmRequest {
            model: &spec.model,
            prompt: &spec.prompt,
            output_schema,
            strict_retry: false,
        };
        let resp = call_with_schema_retry(&*provider, &req)?;
        (vec![resp.text], resp.usage)
    } else {
        // Batched — re-render the prompt for each chunk by re-invoking
        // the runner with the chunk substituted into the named field, then
        // fan out provider calls across worker threads.
        let by = spec.batch.as_ref().unwrap().by.clone();
        let mut chunk_inputs: Vec<JsonValue> = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            chunk_inputs.push(substitute_field(input, &by, JsonValue::Array(chunk)));
        }

        // Render prompts for each chunk via the runner.
        let mut prompts: Vec<String> = Vec::with_capacity(chunk_inputs.len());
        for chunk_input in &chunk_inputs {
            let p = dispatch::invoke_with_input(
                runtime,
                step_file_path,
                RunnerMode::LlmPrompt,
                chunk_input,
            )?;
            let sp: PromptSpec =
                serde_json::from_value(p.output).map_err(|e| BrokerError::BadEnvelope {
                    envelope: "<batch prompt>".to_string(),
                    source: e,
                })?;
            prompts.push(sp.prompt);
            if !p.stderr.trim().is_empty() {
                combined_stderr.push('\n');
                combined_stderr.push_str(&p.stderr);
            }
        }

        // Parallel dispatch.
        fan_out(&*provider, &spec.model, &prompts, output_schema)?
    };

    // Validate + merge.
    let outputs: Result<Vec<JsonValue>> = text_responses
        .iter()
        .map(|t| parse_and_validate(t, output_schema, spec.has_output_schema, provider.name()))
        .collect();
    let outputs = outputs?;

    let final_output = if outputs.len() == 1 {
        outputs.into_iter().next().unwrap()
    } else {
        merge_batched_outputs(outputs, &spec.batch.as_ref().unwrap().by)
    };

    let cost = pricing::cost_eur(
        &spec.model,
        total_usage.input_tokens,
        total_usage.output_tokens,
    );

    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: final_output,
        duration: started.elapsed(),
        stderr: combined_stderr,
        cost_eur: cost,
        usage: Some(total_usage),
    })
}

/// Pick a provider implementation by model-name prefix.
fn pick_provider(model: &str, creds: &LlmCredentials) -> Result<Box<dyn providers::LlmProvider>> {
    let provider_name = provider_for_model(model).ok_or_else(|| BrokerError::LlmUnknownModel {
        model: model.to_string(),
    })?;
    let key = credentials::require(creds, provider_name)?;
    Ok(match provider_name {
        "openai" => Box::new(providers::OpenAiProvider::new(key)),
        "anthropic" => Box::new(providers::AnthropicProvider::new(key)),
        "gemini" => Box::new(providers::GeminiProvider::new(key)),
        _ => unreachable!("provider_for_model returned unknown provider"),
    })
}

/// Map a model name to a provider id. Returns `None` for unknown prefixes.
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        Some("openai")
    } else if model.starts_with("claude-") {
        Some("anthropic")
    } else if model.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
    }
}

/// Split `input[batch.by]` into chunks of `batch.size`. Returns an empty
/// vec when batching does not apply (no batch spec, named field missing
/// or not an array, or array short enough to fit in one call).
fn decide_chunks(input: &JsonValue, batch: Option<&BatchSpec>) -> Vec<Vec<JsonValue>> {
    let Some(batch) = batch else {
        return Vec::new();
    };
    let Some(arr) = input.get(&batch.by).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    if arr.len() <= batch.size {
        return Vec::new();
    }
    arr.chunks(batch.size).map(|c| c.to_vec()).collect()
}

/// Return a clone of `input` with `field` replaced by `value`.
fn substitute_field(input: &JsonValue, field: &str, value: JsonValue) -> JsonValue {
    let mut obj = match input {
        JsonValue::Object(m) => m.clone(),
        _ => JsonMap::new(),
    };
    obj.insert(field.to_string(), value);
    JsonValue::Object(obj)
}

/// Fan out one provider call per prompt across worker threads.
fn fan_out(
    provider: &dyn providers::LlmProvider,
    model: &str,
    prompts: &[String],
    output_schema: Option<&JsonValue>,
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
            scope.spawn(move || loop {
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
                let r = call_with_schema_retry(provider, &req);
                results.lock().unwrap()[i] = Some(r);
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

/// Call the provider, retrying once with `strict_retry: true` if the
/// response can't be JSON-parsed (when a schema was provided).
fn call_with_schema_retry(
    provider: &dyn providers::LlmProvider,
    req: &providers::LlmRequest<'_>,
) -> Result<providers::LlmResponse> {
    let first = provider.complete(req)?;
    if req.output_schema.is_none() {
        return Ok(first);
    }
    if serde_json::from_str::<JsonValue>(strip_json_fences(&first.text)).is_ok() {
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
    if serde_json::from_str::<JsonValue>(strip_json_fences(&second.text)).is_err() {
        return Err(BrokerError::LlmSchemaMismatch {
            provider: provider.name(),
            attempts: 2,
            reason: "response was not valid JSON".to_string(),
        });
    }
    // Sum usage across both attempts so cost accounting reflects reality.
    Ok(providers::LlmResponse {
        text: second.text,
        usage: first.usage + second.usage,
    })
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
    fn provider_dispatch() {
        assert_eq!(provider_for_model("gpt-4o-mini"), Some("openai"));
        assert_eq!(provider_for_model("o1"), Some("openai"));
        assert_eq!(
            provider_for_model("claude-3-5-sonnet-20241022"),
            Some("anthropic")
        );
        assert_eq!(provider_for_model("gemini-1.5-flash"), Some("gemini"));
        assert_eq!(provider_for_model("llama-3"), None);
    }

    #[test]
    fn chunks_split_when_oversized() {
        let input = json!({ "rows": [1, 2, 3, 4, 5] });
        let chunks = decide_chunks(
            &input,
            Some(&BatchSpec {
                size: 2,
                by: "rows".into(),
            }),
        );
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], vec![json!(1), json!(2)]);
        assert_eq!(chunks[2], vec![json!(5)]);
    }

    #[test]
    fn no_chunks_when_short() {
        let input = json!({ "rows": [1, 2] });
        let chunks = decide_chunks(
            &input,
            Some(&BatchSpec {
                size: 5,
                by: "rows".into(),
            }),
        );
        assert!(chunks.is_empty());
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

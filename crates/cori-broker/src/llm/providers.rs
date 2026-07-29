//! `LlmProvider` trait + the three implementations.
//!
//! Each provider speaks JSON to its respective HTTP API. Structured-output
//! enforcement uses the provider's native mechanism when one exists
//! (OpenAI `response_format`, Gemini `responseSchema`, Anthropic tool-use
//! coercion), falling back to schema-in-prompt for the rest.
//!
//! Providers are picked by model-name prefix in [`super::dispatch`].

use serde_json::{Value as JsonValue, json};

use crate::{BrokerError, Result, TokenUsage};

/// One request to a chat-completion LLM.
#[derive(Debug)]
pub struct LlmRequest<'a> {
    pub model: &'a str,
    pub prompt: &'a str,
    /// JSON Schema describing the expected response shape. Providers that
    /// don't natively enforce schemas fall back to embedding this in the
    /// system message.
    pub output_schema: Option<&'a JsonValue>,
    /// If true, this is the retry after a previous schema-validation
    /// failure — provider may use a stricter system message.
    pub strict_retry: bool,
}

/// One response from the LLM, normalised across providers.
#[derive(Debug)]
pub struct LlmResponse {
    /// The raw text returned. For structured-output calls this should be
    /// valid JSON.
    pub text: String,
    pub usage: TokenUsage,
}

pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn complete(&self, req: &LlmRequest<'_>) -> Result<LlmResponse>;
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

pub struct OpenAiProvider {
    pub api_key: String,
    pub base_url: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }
}

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn complete(&self, req: &LlmRequest<'_>) -> Result<LlmResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut system = system_message(req.strict_retry).to_string();
        let strict_schema = req.output_schema.and_then(sanitize_openai_schema);
        if let Some(schema) = req.output_schema
            && strict_schema.is_none()
        {
            system.push_str(
                "\n\nReturn JSON that matches this schema. Optional properties must remain optional; do not invent values for omitted fields:\n",
            );
            system.push_str(&schema.to_string());
        }

        // Use response_format=json_schema when a schema is supplied so the
        // provider enforces structure server-side when the workflow contract
        // is representable without strengthening optional-field semantics.
        let mut body = json!({
            "model": req.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user",   "content": req.prompt },
            ],
        });
        if let Some(schema) = strict_schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "cori_response",
                    "schema": schema,
                    "strict": true,
                }
            });
        } else if req.output_schema.is_some() {
            // OpenAI strict schemas require every property. For a Zod schema
            // with optional fields, use JSON mode plus Cori's local Zod
            // validation/retry instead of silently changing the contract.
            body["response_format"] = json!({ "type": "json_object" });
        }

        let resp = http_client()
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(BrokerError::LlmHttp)?;
        let status = resp.status();
        let raw = resp.text().map_err(BrokerError::LlmHttp)?;
        if !status.is_success() {
            return Err(BrokerError::LlmProviderError {
                provider: "openai",
                status: status.as_u16(),
                body: truncate(&raw, 4096),
            });
        }

        let parsed: JsonValue =
            serde_json::from_str(&raw).map_err(|e| BrokerError::LlmProviderError {
                provider: "openai",
                status: status.as_u16(),
                body: format!("response was not JSON: {e}\n{raw}"),
            })?;

        let text = parsed
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let usage = TokenUsage {
            input_tokens: parsed
                .pointer("/usage/prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: parsed
                .pointer("/usage/completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        };

        Ok(LlmResponse { text, usage })
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    pub api_key: String,
    pub base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com/v1".to_string(),
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn complete(&self, req: &LlmRequest<'_>) -> Result<LlmResponse> {
        let url = format!("{}/messages", self.base_url);

        // Anthropic doesn't expose `response_format`; we coerce the model
        // into JSON via the system message + schema, then validate
        // server-side in the dispatch layer.
        let system = if let Some(schema) = req.output_schema {
            format!(
                "{base}\n\nYou MUST respond with a single JSON object that exactly matches this JSON Schema:\n{schema}\n\nReturn ONLY the JSON object — no markdown fences, no prose.",
                base = system_message(req.strict_retry),
                schema = serde_json::to_string(schema).unwrap_or_default()
            )
        } else {
            system_message(req.strict_retry).to_string()
        };

        let body = json!({
            "model": req.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                { "role": "user", "content": req.prompt },
            ],
        });

        let resp = http_client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .map_err(BrokerError::LlmHttp)?;
        let status = resp.status();
        let raw = resp.text().map_err(BrokerError::LlmHttp)?;
        if !status.is_success() {
            return Err(BrokerError::LlmProviderError {
                provider: "anthropic",
                status: status.as_u16(),
                body: truncate(&raw, 4096),
            });
        }

        let parsed: JsonValue =
            serde_json::from_str(&raw).map_err(|e| BrokerError::LlmProviderError {
                provider: "anthropic",
                status: status.as_u16(),
                body: format!("response was not JSON: {e}\n{raw}"),
            })?;

        // Anthropic returns `content` as an array of blocks; concatenate
        // text blocks.
        let text = parsed
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let usage = TokenUsage {
            input_tokens: parsed
                .pointer("/usage/input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: parsed
                .pointer("/usage/output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        };

        Ok(LlmResponse { text, usage })
    }
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

pub struct GeminiProvider {
    pub api_key: String,
    pub base_url: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
        }
    }
}

impl LlmProvider for GeminiProvider {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn complete(&self, req: &LlmRequest<'_>) -> Result<LlmResponse> {
        // Keep credentials out of the URL: reqwest transport errors include
        // request URLs and those errors are persisted in Temporal history.
        let url = gemini_generate_url(&self.base_url, req.model);

        let mut body = json!({
            "systemInstruction": {
                "parts": [{ "text": system_message(req.strict_retry) }],
            },
            "contents": [{
                "role": "user",
                "parts": [{ "text": req.prompt }],
            }],
        });
        if let Some(schema) = req.output_schema {
            body["generationConfig"] = json!({
                "responseMimeType": "application/json",
                "responseSchema": sanitize_gemini_schema(schema),
            });
        }

        let resp = http_client()
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .map_err(BrokerError::LlmHttp)?;
        let status = resp.status();
        let raw = resp.text().map_err(BrokerError::LlmHttp)?;
        if !status.is_success() {
            return Err(BrokerError::LlmProviderError {
                provider: "gemini",
                status: status.as_u16(),
                body: truncate(&raw, 4096),
            });
        }
        let parsed: JsonValue =
            serde_json::from_str(&raw).map_err(|e| BrokerError::LlmProviderError {
                provider: "gemini",
                status: status.as_u16(),
                body: format!("response was not JSON: {e}\n{raw}"),
            })?;
        let text = parsed
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let usage = TokenUsage {
            input_tokens: parsed
                .pointer("/usageMetadata/promptTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: parsed
                .pointer("/usageMetadata/candidatesTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        };
        Ok(LlmResponse { text, usage })
    }
}

fn gemini_generate_url(base_url: &str, model: &str) -> String {
    format!("{base_url}/models/{model}:generateContent")
}

/// Gemini's `responseSchema` rejects `additionalProperties: false` and a
/// handful of other JSON Schema keywords. Strip them recursively.
fn sanitize_gemini_schema(schema: &JsonValue) -> JsonValue {
    match schema {
        JsonValue::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                if matches!(
                    k.as_str(),
                    "additionalProperties" | "$schema" | "$id" | "const"
                ) {
                    continue;
                }
                out.insert(k.clone(), sanitize_gemini_schema(v));
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(a) => JsonValue::Array(a.iter().map(sanitize_gemini_schema).collect()),
        other => other.clone(),
    }
}

/// Return an OpenAI strict schema only when every object is already closed and
/// every property is required. Rewriting an optional Zod property as required
/// would change the workflow contract, so incompatible schemas return `None`
/// and use JSON mode plus local Zod validation instead.
fn sanitize_openai_schema(schema: &JsonValue) -> Option<JsonValue> {
    match schema {
        JsonValue::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                if matches!(k.as_str(), "$schema" | "$id") {
                    continue;
                }
                out.insert(k.clone(), sanitize_openai_schema(v)?);
            }
            // `const` is valid JSON Schema without `type`, but OpenAI's
            // Structured Outputs subset requires a type at every leaf.
            // Infer it defensively so schemas emitted by older cached
            // runners remain usable after a Cori binary upgrade.
            if !out.contains_key("type") {
                if let Some(schema_type) = out.get("const").and_then(json_schema_type) {
                    out.insert(
                        "type".to_string(),
                        JsonValue::String(schema_type.to_string()),
                    );
                } else if let Some(values) = out.get("enum").and_then(JsonValue::as_array)
                    && let Some(first_type) = values.first().and_then(json_schema_type)
                    && values
                        .iter()
                        .all(|value| json_schema_type(value) == Some(first_type))
                {
                    out.insert(
                        "type".to_string(),
                        JsonValue::String(first_type.to_string()),
                    );
                }
            }
            if out.get("type").and_then(|t| t.as_str()) == Some("object") {
                let props = out
                    .get("properties")
                    .and_then(JsonValue::as_object)
                    .cloned()
                    .unwrap_or_default();
                let property_names: std::collections::BTreeSet<&str> =
                    props.keys().map(String::as_str).collect();
                let required_names: std::collections::BTreeSet<&str> = out
                    .get("required")
                    .and_then(JsonValue::as_array)
                    .map(|required| required.iter().filter_map(JsonValue::as_str).collect())
                    .unwrap_or_default();
                if required_names != property_names
                    || out.get("additionalProperties") != Some(&JsonValue::Bool(false))
                {
                    return None;
                }
                out.insert(
                    "required".to_string(),
                    JsonValue::Array(
                        props
                            .keys()
                            .map(|key| JsonValue::String(key.clone()))
                            .collect(),
                    ),
                );
            }
            Some(JsonValue::Object(out))
        }
        JsonValue::Array(a) => Some(JsonValue::Array(
            a.iter()
                .map(sanitize_openai_schema)
                .collect::<Option<Vec<_>>>()?,
        )),
        other => Some(other.clone()),
    }
}

fn json_schema_type(value: &JsonValue) -> Option<&'static str> {
    match value {
        JsonValue::Null => Some("null"),
        JsonValue::Bool(_) => Some("boolean"),
        JsonValue::Number(_) => Some("number"),
        JsonValue::String(_) => Some("string"),
        JsonValue::Array(_) => Some("array"),
        JsonValue::Object(_) => Some("object"),
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn http_client() -> &'static reqwest::blocking::Client {
    static CELL: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("static http client")
    })
}

fn system_message(strict: bool) -> &'static str {
    if strict {
        "You are an automation backend. Your previous response did not match the required JSON schema. You MUST return a single JSON object that conforms to the schema exactly — no markdown fences, no prose, no trailing commentary."
    } else {
        "You are an automation backend invoked by a Cori workflow. Return a single JSON object matching the requested schema. No prose, no markdown fences."
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("\n…(truncated)");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_schema_infers_type_for_literal_const() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "status": { "const": "triaged" },
                "attempt": { "enum": [1, 2] }
            },
            "required": ["status", "attempt"]
        });
        let sanitized = sanitize_openai_schema(&schema).expect("strict-compatible schema");
        assert!(sanitized.get("$schema").is_none());
        assert_eq!(
            sanitized.pointer("/properties/status/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            sanitized.pointer("/properties/attempt/type"),
            Some(&json!("number"))
        );
    }

    #[test]
    fn openai_strict_schema_does_not_make_optional_fields_required() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "status": { "type": "string" },
                "detail": { "type": "string" }
            },
            "required": ["status"]
        });
        assert!(
            sanitize_openai_schema(&schema).is_none(),
            "optional fields must use JSON mode and local Zod validation"
        );
    }

    #[test]
    fn gemini_endpoint_never_contains_the_api_key() {
        let key = "secret-key-that-must-not-enter-errors";
        let url = gemini_generate_url(
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-1.5-flash",
        );
        assert!(!url.contains(key));
        assert!(!url.contains("?key="));
    }
}

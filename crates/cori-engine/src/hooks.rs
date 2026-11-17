// SPDX-License-Identifier: Apache-2.0
//! Extension points of the worker: what a binary's modules add to steps (`cori_server::module::Module::llm_step_hook`),
//! installed once when the binary starts (`install`). Without a module none is installed, and every step runs as this
//! crate defines it.
use crate::activities::StepInput;
use crate::config::Binding;
use crate::sandbox::StepRunner;
use anyhow::Result;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

/// What a hook returns: a future run inside the step's activity.
pub type HookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Called by an `llm` step once its model answered, the answer went through the step's output schema and the call was
/// captured: the step's output is decided, and nothing a hook does changes it. The notes a hook returns are added to the
/// step's (they show with the run).
pub trait LlmStepHook: Send + Sync {
    fn after_call<'a>(&'a self, step: &'a LlmStep<'a>) -> HookFuture<'a, Vec<String>>;
}

static LLM_STEP_HOOKS: OnceLock<Vec<Arc<dyn LlmStepHook>>> = OnceLock::new();

/// Installs the binary's `llm` step hooks, before its worker starts; only the first call counts.
pub fn install(llm_step: Vec<Arc<dyn LlmStepHook>>) { let _ = LLM_STEP_HOOKS.set(llm_step); }

pub(crate) fn llm_step_hooks() -> &'static [Arc<dyn LlmStepHook>] { LLM_STEP_HOOKS.get().map(Vec::as_slice).unwrap_or(&[]) }

/// An answered `llm` step, as a hook sees it.
pub struct LlmStep<'a> {
    pub workflow: &'a str,
    pub name: &'a str,
    pub tier: &'a str,
    /// The step's input, the values under the manifest's `redact:` paths blanked (what a capture records).
    pub input: &'a Value,
    pub(crate) i: &'a StepInput,
    pub(crate) runner: &'a StepRunner,
    pub(crate) file: &'a str,
    pub(crate) prompt: &'a Value,
    pub(crate) server: Option<&'a str>,
}

/// One model's answer to a step's prompt.
pub struct ModelAnswer { pub provider: String, pub model: String, pub raw: Value, pub cost_eur: f64, pub tokens_in: u64, pub tokens_out: u64, pub latency_ms: u64 }

impl LlmStep<'_> {
    /// The model bound to this step in `mode` (or to every step of the app in that mode), from the server's bindings.
    pub fn binding(&self, mode: &str) -> Option<Binding> { crate::activities::binding_for(&self.i.llm, self.workflow, self.name, mode) }

    /// Asks `binding`'s model the step's prompt as the step's own call did: the same provider routing and the same key
    /// lease, so the same monthly budget.
    pub async fn call(&self, binding: &Binding) -> Result<ModelAnswer> {
        let t0 = std::time::Instant::now();
        let a = crate::activities::llm_call(self.i, self.name, self.tier, self.prompt, self.input, Some(binding)).await?;
        Ok(ModelAnswer { provider: a.provider, model: a.model, raw: a.raw, cost_eur: a.cost_eur, tokens_in: a.tokens_in, tokens_out: a.tokens_out, latency_ms: t0.elapsed().as_millis() as u64 })
    }

    /// The step's output schema on an answer: the output it validates to, or why it refuses it.
    pub async fn validate(&self, raw: &Value) -> Result<Value> {
        let v = self.runner.call(json!({ "op": "validate", "file": self.file, "name": self.name, "value": raw })).await?;
        Ok(v["output"].clone())
    }

    /// Records an answer among the app's captured LLM calls (`GET /v1/apps/{app}/llm/calls`) under `mode`, as the step's
    /// own call is recorded; `output` is what was kept of it (the validated output, else the raw answer).
    pub fn capture(&self, mode: &str, answer: &ModelAnswer, output: &Value, schema_valid: bool) {
        crate::events::emit(self.server, crate::workflow::BundleRef::realm_of(&self.i.bundle), self.i.run["id"].as_str().unwrap_or(""), "llm_call", json!({ "workflow": self.workflow, "step": self.name, "tier": self.tier, "provider": answer.provider, "model": answer.model, "mode": mode, "input": self.input, "output": output, "schema_valid": schema_valid, "latency_ms": answer.latency_ms, "tokens_in": answer.tokens_in, "tokens_out": answer.tokens_out, "cost_eur": answer.cost_eur }));
    }
}

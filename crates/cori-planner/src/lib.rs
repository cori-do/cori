use async_trait::async_trait;

#[async_trait]
pub trait Planner: Send + Sync {
    async fn plan(
        &self,
        request: &str,
        allowed_actions: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value>;
}

/// Stub planner: returns an empty plan.
pub struct NoopPlanner;

#[async_trait]
impl Planner for NoopPlanner {
    async fn plan(
        &self,
        request: &str,
        _allowed_actions: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "feasible": false,
            "summary": "noop planner",
            "errors": ["LLM planner not configured"],
            "request": request
        }))
    }
}

use cori_runtime::adapter::{ActionOutcome, DataAdapter};
use async_trait::async_trait;

pub mod introspect;

pub struct PostgresAdapter {
    // In MVP we’ll hold a sqlx::PgPool here.
    // pub pool: sqlx::PgPool,
}

impl PostgresAdapter {
    pub async fn new(_database_url: &str) -> anyhow::Result<Self> {
        // let pool = sqlx::PgPool::connect(database_url).await?;
        Ok(Self {})
    }
}

#[async_trait]
impl DataAdapter for PostgresAdapter {
    async fn load_resource_attrs(
        &self,
        _tenant_id: &str,
        _resource_kind: &str,
        _resource_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({}))
    }

    async fn execute_action(
        &self,
        _tenant_id: &str,
        action: &str,
        inputs: &serde_json::Value,
        preview: bool,
    ) -> anyhow::Result<ActionOutcome> {
        // MVP stub: no real SQL execution yet.
        Ok(ActionOutcome {
            affected_count: 0,
            preview_diff: if preview {
                Some(serde_json::json!({ "note": "preview stub", "action": action, "inputs": inputs }))
            } else {
                None
            },
            output: serde_json::json!({ "ok": true, "action": action, "preview": preview }),
        })
    }
}

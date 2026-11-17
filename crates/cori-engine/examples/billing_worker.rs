// SPDX-License-Identifier: Apache-2.0
//! A stand-in for the billing team's own Temporal worker (Go/C# in real life): registers `IssueRefund`
//! on `cori.acme.native.billing`, reads the Cori envelope, returns the typed output. The run's token comes sealed for it,
//! and the connector's key opens it (`cori connectors key billing`, as CORI_CONNECTOR_KEY): as the SDKs do.
//! Run: cargo run -p cori-engine --example billing_worker
use serde_json::{Value, json};
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::error::ApplicationFailure;
use temporalio_sdk::{Worker, WorkerOptions};

pub struct Billing;

#[activities]
impl Billing {
    #[activity(name = "IssueRefund")]
    pub async fn issue_refund(_ctx: ActivityContext, env: Value) -> Result<Value, ActivityError> {
        let input = &env["input"]; let cori = &env["cori"];
        let roles = cori["user"]["roles"].as_array().cloned().unwrap_or_default();
        if !roles.iter().any(|r| r == "support_agent" || r == "support_lead") {
            return Err(ActivityError::application(ApplicationFailure::builder(anyhow::anyhow!("caller lacks a support role")).type_name("Forbidden".to_string()).non_retryable(true).build()));
        }
        let token = match cori["principal_token"].as_str() {
            Some(t) if t.starts_with("cori-sealed.v1.") => match open(t, cori["run_id"].as_str().unwrap_or("")) { Some(jwt) if jwt.split('.').count() == 3 => "opened", Some(_) => "opened-not-a-jwt", None => "sealed" },
            Some(_) => "clear",
            None => "absent",
        };
        eprintln!("[billing worker] IssueRefund invoice={} amount={} run={} idem={} principal_token={token}", input["invoice_id"], input["amount_eur"], cori["run_id"], cori["idempotency_key"]);
        Ok(json!({ "refund_id": format!("RF-{}", &cori["run_id"].as_str().unwrap_or("x")[..8.min(cori["run_id"].as_str().unwrap_or("x").len())]) }))
    }
}

/// The run's token, opened with CORI_CONNECTOR_KEY (sdks/go `OpenToken`): None without the key, or for another run.
fn open(sealed: &str, run_id: &str) -> Option<String> {
    use aes_gcm::{Aes256Gcm, KeyInit, aead::{Aead, Payload}};
    use base64::Engine;
    let key = hex::decode(std::env::var("CORI_CONNECTOR_KEY").ok()?.trim()).ok()?;
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sealed.strip_prefix("cori-sealed.v1.")?).ok()?;
    if b.len() < 28 || key.len() != 32 { return None; }
    let (nonce, ct) = b.split_at(12);
    let nonce: [u8; 12] = nonce.try_into().ok()?;
    let plain = Aes256Gcm::new_from_slice(&key).ok()?.decrypt(&aes_gcm::Nonce::from(nonce), Payload { msg: ct, aad: run_id.as_bytes() }).ok()?;
    String::from_utf8(plain).ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The activity type Cori dispatches is the declared `name` in activities.yaml: "IssueRefund".
    let rt = cori_engine::runtime::TemporalRuntime::connect("http://127.0.0.1:7233", "default", "cori.acme.native.billing").await?;
    let opts = WorkerOptions::new(rt.task_queue.clone()).register_activities(Billing).build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), opts).map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!("[billing worker] polling cori.acme.native.billing");
    worker.run().await.map_err(|e| anyhow::anyhow!("{e}"))
}

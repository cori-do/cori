// SPDX-License-Identifier: Apache-2.0
//! Records the history of a finished run in the form the replay test reads (tests/histories/, see tests/replay.rs):
//!   cargo run -p cori-engine --example record_history -- <workflow id> <out.json> [temporal address] [namespace]
//! Record runs without real credentials in their input (dev auth).
use cori_engine::workflow::CoriWorkflow;
use temporalio_client::{Client, ClientOptions, Connection, WorkflowFetchHistoryOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(id), Some(out)) = (args.first(), args.get(1)) else { anyhow::bail!("usage: record_history <workflow id> <out.json> [temporal address] [namespace]") };
    let address = args.get(2).cloned().unwrap_or_else(|| "http://127.0.0.1:7233".into());
    let namespace = args.get(3).cloned().unwrap_or_else(|| "default".into());
    let connection = Connection::connect(cori_engine::runtime::connection_options(&address, "cori-record-history")?).await?;
    let client = Client::new(connection, ClientOptions::new(namespace).build())?;
    let json = client.get_workflow_handle::<CoriWorkflow>(id.clone()).fetch_history(WorkflowFetchHistoryOptions::default()).to_json().await?;
    std::fs::write(out, json)?;
    println!("{out}");
    Ok(())
}

// SPDX-License-Identifier: Apache-2.0
//! `cori`: the core, with no module.

#[tokio::main]
async fn main() -> anyhow::Result<()> { cori_cli::run(cori_cli::Distribution::core()).await }

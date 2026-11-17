// SPDX-License-Identifier: Apache-2.0
//! Cori Store's runtime: the request validator (`validation`), the executor that runs a generated tool as a role on
//! Postgres (`executor`), changes parked for approval (`approval`), the compiler of roles and rules into Postgres grants
//! and row level security (`grants`), and schema introspection (`introspect`). Moved from the cori-store repository.

pub mod approval;
pub mod executor;
pub mod grants;
pub mod introspect;
pub mod validation;

pub use approval::ParkedChange;
pub use executor::{ExecutionContext, ExecutionResult, ToolExecutor};

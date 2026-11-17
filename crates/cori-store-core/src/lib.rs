// SPDX-License-Identifier: Apache-2.0
//! Cori Store's definitions and tool generation, shared by the compiler (offline: `cori check`, `cori scaffold`,
//! `cori store tools`) and the runtime (`cori-store-engine`, `cori-store`): the schema, rules and role files of a store,
//! and the typed tools a role gets on it. No database, no async runtime. Moved from the cori-store
//! repository.

pub mod config;
pub mod protocol;
pub mod tool_generator;

pub use config::*;

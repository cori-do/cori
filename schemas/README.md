This folder contain JSON Schemas for:
- MutationIntent
- Plan
- ActionDefinition
- PolicyDecision
- AuditEvent

These schemas represent the **intended contracts** for the Cori protocol.

**Note (current repo state):** the Rust types in `crates/cori-core` and the runtime audit events in `crates/cori-runtime` currently implement only a subset of these schemas, and some fields (e.g. full `AuditEvent` envelope) are not yet present in code.
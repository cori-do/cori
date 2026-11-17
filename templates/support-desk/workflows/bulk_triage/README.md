# Bulk triage

## Goal
A support lead pastes ticket ids; each one runs `support_ticket_triage` as a **child
workflow** (its own Temporal execution, same user, same connectors, visible in the run
index under `<run>:<step>`), four at a time; a code step counts the outcome.

## Verification
- `triage_each` runs once per ticket as child `bulk_triage-…:triage_each#N`
- the child's result values (`category`, `priority`) are what the step exposes

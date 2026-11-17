# Triage support ticket

## Goal
Given a ticket id, classify it, look up the customer's plan, decide the priority with a
rule, ask a support lead before escalating a P1, and draft a reply for the agent to edit.

## Dataflow (derived by `cori check`, not declared)
```
load_ticket ─┬─▶ classify ────────┐
             └─▶ lookup_customer ─┴─▶ decide_priority ─▶ escalate_gate ─▶ [approve_escalation] ─▶ notify ─▶ draft_reply
```
`classify` and `lookup_customer` run in parallel. `notify` is `MayWrite` and keeps file
order after the gate.

## Verification
- 8 rows per run (approve_escalation and notify may be `not_taken`)
- change review: helpdesk `Read` (getTicket readOnlyHint), tickets `Read` (role readable), slack `MayWrite`
- lineage: `ticket.body` reaches `classify` (small) and `draft_reply` (medium); redacted from traces

# Refund request

## Goal
A support agent asks for a refund from the app. The request is recorded in the store
(`refund_requests` table, `requires_approval` on `status: approved` for amounts above
200 € is a **store rule**, not workflow logic), a lead approves, the billing team's own
Temporal worker (C#, official SDK) issues the refund with the run principal's exchanged JWT, and the ticket gets a reply. Failure in
billing is caught by `try` and surfaced to Slack instead of failing silently.

## Dataflow
```
record_request ─▶ needs_approval ─▶ [approve_refund] ─▶ refund_try{ issue_refund } ─▶ confirm_on_ticket ─▶ outcome
                                                          └ on_error ─▶ alert_billing_failure ─┘
```

## Verification
- change review: tickets `MayWrite` (creatable refund_requests), billing `MayWrite` (IssueRefund declared destructive ⇒ approval flag on the connector), helpdesk `MayWrite` (addReply)
- `cori check` warns if no worker polls `cori.acme.native.billing`; at run time `schedule_to_start` fails fast
- an amount ≤ 200 € never creates an approval item; > 200 € always does
- a billing failure produces `outcome.status = failed` and a Slack alert, never a crashed run

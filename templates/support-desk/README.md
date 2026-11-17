# Support Desk — the demo app

One vibe-coded front, four workflows, one store, one SSO. The folder is the app: `cori.toml` at the root,
workflows under `workflows/`, dev fixtures under `fixtures/`, the front source under `front/` (built into `dist/`).
`AGENTS.md` tells an agent the layout and the commands.

| Piece | Kind | Shows |
|---|---|---|
| `cori.toml` | app manifest | front hosting, SSO roles, connectors incl. a company MCP server and a `cli` |
| `workflows/support_ticket_triage` | sync endpoint | typed bindings, automatic parallelism (classify ∥ lookup), branch → human, `mcp` with principal token, `store` read, `llm` ×2 |
| `workflows/refund_request` | async endpoint | `store` write with `requires_approval`, human approval routed to a role, `try`/`on_error`, **native C# activity** (`step.activity`), `http` service call |
| `billing-activities/` | native activities | what the billing team owns: `activities.yaml` and a C#/Go worker on the official SDK |
| `workflows/stale_tickets_digest` | schedule | `schedule_as` service principal, `store.query` guarded SQL, `map` fan-out, `cli` in a sandbox |
| `stores/support_tickets` | store | migrations (DDL), `seed/` demo rows for sandboxes and local runs, rules and roles |
| `workflows/bulk_triage` | sync endpoint | `step.workflow` children (one triage per ticket), `map` fan-out |
| `front/src/lib/cori.ts` | generated client | what the SPA imports (`cori gen client --dir .`) |
| `front/src/App.tsx` | SPA excerpt | calling a sync workflow, starting an async one, rendering approvals, `<Protected>` by role; under `cori up` the hosted front carries a dev widget to impersonate a role |

```bash
cori check .                                   # every workflow
cori run workflows/support_ticket_triage ticket_id=T-1042
cori deploy .                                  # a version with its sandbox stores and preview url
cori promote support-desk <version>            # a human: the default version, live stores migrated
```

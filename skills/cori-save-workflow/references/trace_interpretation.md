# Trace interpretation

Run traces are persisted to `~/.cori/runs/<key>/<utc>.json`.
The types are defined in `crates/cori-protocol/src/trace.rs`.

## RunTrace (top-level)

```
run_id             string     UUID v4, unique per execution
workflow_id        string     manifest `id` field
workflow_content_hash  string?    16-hex of folder content hash at run time
status             string     "succeeded" | "failed"
trigger            string     "cli" | "console" | "schedule"
dry_run            bool       true if --dry-run was passed (default false)
requesting_identity  WorkerIdentity?  who started the run
started_at         DateTime<Utc>
ended_at           DateTime<Utc>
duration_ms        u128
source             WorkflowSource?   where the workflow came from
params             json       user-supplied parameters
result             ResolvedResult?  declared user-facing result, absent on legacy/undeclared runs
activities         ActivityTrace[]
cost               CostSummary
error              string?    top-level error message if status = "failed"
```

## ResolvedResult

`result` is resolved after execution from parameters and successful object
outputs. It is persisted in the trace, so it remains available if the workflow
source later moves. Resolution issues never change run success.

```
headline           string
description        string?
fields             { label, value, format, currency?, tone }[]
sections           { label, value, display }[]
artifacts          { label, url }[]
issues             { item, type, message }[]
```

On failed runs, the result contains whatever earlier successful steps made
available and should be presented as a partial result. Runs written before this
field existed deserialize without migration.

## ActivityTrace (per step)

```
activity_id        string     stable id from manifest (e.g. "step_translate")
step_name          string     human label
kind               StepKind   "cli" | "mcp_tool" | "code" | "llm" | "builtin"
status             string     "succeeded" | "failed" | "skipped"
started_at / ended_at  DateTime<Utc>
duration_ms        u128
attempts           u32        how many Temporal attempts were made
task_queue         string?    queue the activity was dispatched to
worker_identity    WorkerIdentity?  identity derived from task_queue
input_summary      json       truncated view of the activity input
output_summary     json       truncated view of the activity output
output             json       full activity output
cost_eur           f64?       EUR cost for this activity (LLM steps only)
tokens             TokenUsage?  { input_tokens, output_tokens }
error              string?
notes              string?
```

### Builtin step rows

A builtin control-flow step (`branch`, `switch`, `for_each`, `loop`, `wait`)
produces exactly **one** ActivityTrace row; its nested outcomes fold into that
row:

- `attempts` counts every internal dispatch — selector evaluations, nested
  activity dispatches, and reauth retries — not just retries of a single
  activity.
- `notes` carries the control-flow decision: "took `then`", "matched
  `cases.high`", "applied to 12 item(s)", "goal met after 3 iteration(s)",
  "paused 60s", "resumed by event `approved`".
- `cost_eur` and `tokens` aggregate any nested `llm` dispatches.
- Output shapes: `for_each` → `{ items: [...] }`; `loop` → the last
  body output plus `iterations`; `wait` → `{ waited_ms }` or `{ event }`;
  a routed branch/switch → `{ routed_to }` with a note like
  "took `else` → routed to `05_cleanup`".
- Steps a `goto` route jumped past appear with status **`not_taken`**
  (duration 0, attempts 0, a note naming the routing step). They never
  contribute to dataflow and are excluded from median baselines.

## WorkflowSource

```json
{ "kind": "local", "path": "/abs/path/to/workflow" }
{ "kind": "remote", "host": "github.com", "repo": "org/workflows",
  "subpath": "translate_fr", "ref": "v1.2.0", "sha": "abc1234..." }
```

## CostSummary

```
total_eur          f64    sum across all LLM activities
input_tokens       u64
output_tokens      u64
```

## WorkerIdentity

```json
{ "Person": { "user_id": "jean" } }
{ "Service": { "pool": "notion-pool" } }
```

## Reading `error` on a failed `llm` activity

The activity's `error` carries the provider-selection or provider error. Read
the message before touching anything:

- **No active provider / auth / missing-key error** → select or repair the
  machine's active provider in Console → Settings → AI Providers. Cori will
  not fall back to another connected provider.
- **404 / "model not found"** → the provider is authenticated and reachable;
  the model id simply doesn't exist. Plausible-looking ids — including dated
  snapshot names — routinely 404. Reset or correct that level in the active
  provider's **Advanced models** settings. Changing workflow source is not the fix.

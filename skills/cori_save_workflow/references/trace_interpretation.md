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
activities         ActivityTrace[]
cost               CostSummary
error              string?    top-level error message if status = "failed"
```

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

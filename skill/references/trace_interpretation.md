# Trace interpretation — reading `cori run --json` output

When you run a workflow with `cori run <id> --json`, Cori emits a structured JSON trace to stdout describing the run. This file is the shape of that trace and what to do with it.

## Why this file exists

The trace is for *you* (the agent) to reason about. Your job is to synthesize it for the user — surface what matters, omit what doesn't, suggest next actions. Don't dump the raw JSON.

## Top-level shape

```json
{
  "run_id": "run_8f2c1a4e",
  "workflow_id": "translate_product_sheets_fr",
  "workflow_content_hash": "3f9a2c1b",
  "status": "succeeded",
  "trigger": "cli",
  "dry_run": false,
  "requesting_identity": { "kind": "person", "user_id": "adrien" },
  "started_at": "2026-05-24T14:02:11Z",
  "ended_at": "2026-05-24T14:02:14Z",
  "duration_ms": 3284,
  "params": { "spreadsheet_id": "ABC123" },
  "cost": {
    "total_eur": 0.018,
    "input_tokens": 1240,
    "output_tokens": 612
  },
  "activities": [ /* see below */ ],
  "error": null
}
```

`workflow_content_hash` replaced the old `workflow_version` field: there is no registry to version against, so each trace records the 16-hex content hash of the workflow folder (`manifest.md` + every `steps/*` file) at run time. Two runs of the same folder bytes share a hash; touching any file produces a new one.

`requesting_identity` records who started the run — a `Person { user_id }` for local `cori run`, a `Service { pool }` if a shared worker triggered it. The broker scopes credential lookup by this identity (Phase 5+).

`status` is one of:

- `"succeeded"` — every activity returned successfully
- `"failed"` — one activity failed after exhausting retries; the run stopped
- `"partial"` — some activities succeeded, but the user aborted or a non-fatal error stopped progress
- `"running"` — only seen with `cori run --watch`; the run isn't done yet

**Cost framing:** `total_eur` plus `input_tokens` / `output_tokens` are pass-through provider costs (LLM API calls). Cori v1 doesn't add a service fee. `cli`, `code`, `mcp_tool`, `builtin` activities are €0.00 because they execute locally on the user's machine. When surfacing cost to the user, say "LLM cost" or "provider cost," not "Cori cost."

## Per-activity shape

Each entry in `activities` is one execution of one step:

```json
{
  "activity_id": "translate_rows#0",
  "step_name": "translate_rows",
  "kind": "llm",
  "status": "ok",
  "started_at": "2026-05-24T14:02:12Z",
  "ended_at": "2026-05-24T14:02:13Z",
  "duration_ms": 1804,
  "attempts": 1,
  "route": null,
  "task_queue": "cori.user.adrien",
  "worker_identity": { "kind": "person", "user_id": "adrien" },
  "cost_eur": 0.018,
  "input_summary": { "rows_count": 50 },
  "output_summary": { "translations_count": 50 },
  "output": { /* full output of the step */ },
  "error": null,
  "notes": null
}
```

A few things worth knowing:

- **`activity_id`** disambiguates repeated executions (e.g. when a `map` builtin runs the same step over 50 items, there are 50 activities with ids `translate_row#0`, `translate_row#1`, …).
- **`status`** is `"ok"`, `"failed"`, or `"skipped"` (dry-run / branch-not-taken).
- **`task_queue`** + **`worker_identity`** record where the activity actually ran — `cori.user.<id>` means the requesting user's own laptop; `cori.service.<pool>` means a shared service worker. Useful for diagnosing "why did this step fail on machine X". Both are `null` for legacy traces predating Phase 7.
- **`attempts`** > 1 means Cori retried — surface this to the user if it's high; it often signals a flaky upstream.
- **`input_summary`** / **`output_summary`** are *summaries*, not full payloads — typically counts, sizes, sampled keys. Full payload is in `output`. The CLI also exposes both via `cori runs show <run_id> --activity <activity_id> --full`.
- **`error`** is null on success, otherwise a message string.

## What to surface for a successful run

Be concise. Three to five lines is plenty.

> ✓ translate_product_sheets_fr (content 3f9a2c1b) completed in 3.3s.
> 5 activities, all succeeded. 50 rows translated, 47 OK / 3 NOK for GPSR.
> LLM cost: €0.018 (OpenAI gpt-4o-mini).
> Full trace: `cori runs show run_8f2c1a4e`

Pull the user-facing numbers (50 rows, 47/3 OK/NOK) from `output_summary` of the relevant activities, not the raw payload. If `output_summary` doesn't have what you need, run `cori runs show <run_id> --activity <activity_id> --full` for that specific activity.

## What to surface for a failed run

The user needs three things immediately:

1. **Which step failed**
2. **What the error message said**
3. **What to do next**

For (3), check the workflow manifest's prose body (via `cori show <path>`) for anything that addresses the failure mode. If there's relevant guidance, surface it. Don't invent guidance.

Example:

> ✗ translate_product_sheets_fr (content 3f9a2c1b) failed in 4.1s.
> Step 2 (translate_rows, llm) failed after 3 retries: "rate_limit_exceeded: gpt-4o-mini, retry after 60s".
> The manifest notes that batched 50/call is the right size for typical row volumes — your input had 1,200 rows, so this batch hit a per-minute cap. Try splitting the run, or reduce `batch.size` in the step.
> Retry now? (yes / no / show full trace)

If the error is `retriable: true` and was caused by a transient failure (rate limit, transient network), offer to retry. Otherwise just report.

## What to surface for `--dry-run`

A dry run executes the workflow with **mock I/O** — `cli` and `mcp_tool` activities are skipped (return fixture data if available), `llm` activities return their schema's example or a stub, `code` and `builtin` activities run for real.

The trace has the same shape but every external activity will have `status: "skipped"` with `notes` containing `"mocked"`. The top-level `dry_run` flag is also `true`. Surface:

> Dry run completed. The plan is valid.
> No external calls were made. `code` step `check_gpsr` ran against a fixture and produced expected output.
> Run for real with: `cori run <path>`

## Common patterns and what they mean

A few patterns you'll see in traces and what they typically indicate:

- **Most activities have `attempts: 1` but one has `attempts: 3`** — that step has a flaky upstream. Suggest the user check rate limits, network, or the underlying service.
- **`cost.total_eur` is entirely LLM** — expected in v1, since other kinds are free (local execution). When LLM cost is high, suggest batching larger, switching to a smaller model, or caching outputs.
- **A step ran but `output_summary` is empty** — usually a no-op (e.g. `ensure_fr_tab` when the tab already exists). Mention this if relevant ("the FR tab already existed, so step 4 was a no-op").
- **`task_queue` mixes `cori.user.*` and `cori.service.*`** — the run was placed across machines (Phase 4 routing). If a step failed on a service queue, point the user at the operator of that pool, not their own laptop.
- **`trigger: "scheduled"`** — this run was triggered by the cron schedule defined in the manifest. The user wasn't in the loop, so there's no human to ask about retries; report and let the user decide.

## Don'ts

- Don't dump raw JSON unless the user explicitly asks for it.
- Don't invent details that aren't in the trace.
- Don't claim a step succeeded when its `status` says otherwise.
- Don't suggest fixes you can't back up from the manifest notes or the error message.
- Don't apologize for failures. Report them and offer next steps.
- Don't refer to "Cori cost" — costs in v1 are pass-through provider costs. Frame them as "LLM cost" or "provider cost."

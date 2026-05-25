# Trace interpretation — reading `cori run --json` output

When you run a workflow with `cori run <id> --json`, Cori emits a structured JSON trace to stdout describing the run. This file is the shape of that trace and what to do with it.

## Why this file exists

The trace is for *you* (the agent) to reason about. Your job is to synthesize it for the user — surface what matters, omit what doesn't, suggest next actions. Don't dump the raw JSON.

## Top-level shape

```json
{
  "run_id": "run_8f2c1a4e",
  "workflow_id": "translate_product_sheets_fr",
  "workflow_version": 4,
  "status": "succeeded",
  "started_at": "2026-05-24T14:02:11Z",
  "ended_at": "2026-05-24T14:02:14Z",
  "duration_ms": 3284,
  "trigger": {
    "source": "cli",
    "user": "adrien@my_org.com",
    "params": { "spreadsheet_id": "ABC123", "dry_run": false }
  },
  "cost": {
    "total_eur": 0.018,
    "by_kind": { "cli": 0.0, "llm": 0.018, "code": 0.0, "mcp_tool": 0.0, "builtin": 0.0 }
  },
  "activities": [ /* see below */ ],
  "warnings": [],
  "error": null
}
```

`status` is one of:

- `"succeeded"` — every activity returned successfully
- `"failed"` — one activity failed after exhausting retries; the run stopped
- `"partial"` — some activities succeeded, but the user aborted or a non-fatal error stopped progress
- `"running"` — only seen with `cori run --watch`; the run isn't done yet

**Cost framing:** `total_eur` and `by_kind` are pass-through provider costs (LLM API calls). Cori v1 doesn't add a service fee. `cli`, `code`, `mcp_tool`, `builtin` activities are €0.00 because they execute locally on the user's machine. When surfacing cost to the user, say "LLM cost" or "provider cost," not "Cori cost."

## Per-activity shape

Each entry in `activities` is one execution of one step:

```json
{
  "activity_id": "translate_rows#0",
  "step_name": "translate_rows",
  "kind": "llm",
  "status": "succeeded",
  "started_at": "2026-05-24T14:02:12Z",
  "ended_at": "2026-05-24T14:02:13Z",
  "duration_ms": 1804,
  "attempts": 1,
  "cost_eur": 0.018,
  "input_summary": { "rows_count": 50 },
  "output_summary": { "translations_count": 50 },
  "error": null,
  "notes": []
}
```

A few things worth knowing:

- **`activity_id`** disambiguates repeated executions (e.g. when a `map` builtin runs the same step over 50 items, there are 50 activities with ids `translate_row#0`, `translate_row#1`, …).
- **`status`** values are the same set as the top-level status, applied to one activity.
- **`attempts`** > 1 means Cori retried — surface this to the user if it's high; it often signals a flaky upstream.
- **`input_summary`** / **`output_summary`** are *summaries*, not full payloads — typically counts, sizes, sampled keys. Full payloads accessible via `cori runs show <run_id> --activity <activity_id> --full` if the user asks.
- **`error`** is null on success, otherwise an object: `{ message, code, retriable }`.

## What to surface for a successful run

Be concise. Three to five lines is plenty.

> ✓ translate_product_sheets_fr (v4) completed in 3.3s.
> 5 activities, all succeeded. 50 rows translated, 47 OK / 3 NOK for GPSR.
> LLM cost: €0.018 (OpenAI gpt-4o-mini).
> Full trace: `cori runs show run_8f2c1a4e`

Pull the user-facing numbers (50 rows, 47/3 OK/NOK) from `output_summary` of the relevant activities, not the raw payload. If `output_summary` doesn't have what you need, run `cori runs show <run_id> --activity <activity_id> --full` for that specific activity.

## What to surface for a failed run

The user needs three things immediately:

1. **Which step failed**
2. **What the error message said**
3. **What to do next**

For (3), check the workflow manifest's `## Notes` section (via `cori workflows show <id> --field=notes`) for anything that addresses the failure mode. If there's relevant guidance, surface it. Don't invent guidance.

Example:

> ✗ translate_product_sheets_fr (v4) failed in 4.1s.
> Step 2 (translate_rows, llm) failed after 3 retries: "rate_limit_exceeded: gpt-4o-mini, retry after 60s".
> The manifest notes that batched 50/call is the right size for typical row volumes — your input had 1,200 rows, so this batch hit a per-minute cap. Try splitting the run, or reduce `batch.size` in the step.
> Retry now? (yes / no / show full trace)

If the error is `retriable: true` and was caused by a transient failure (rate limit, transient network), offer to retry. Otherwise just report.

## What to surface for `--dry-run`

A dry run executes the workflow with **mock I/O** — `cli` and `mcp_tool` activities are skipped (return fixture data if available), `llm` activities return their schema's example or a stub, `code` and `builtin` activities run for real.

The trace has the same shape but every external activity will have `status: "skipped"` with `output_summary.mocked: true`. Surface:

> Dry run completed. The plan is valid.
> No external calls were made. `code` step `check_gpsr` ran against a fixture and produced expected output.
> Run for real with: `cori run <id>` (or `trigger_workflow <id>`)

## Common patterns and what they mean

A few patterns you'll see in traces and what they typically indicate:

- **Most activities have `attempts: 1` but one has `attempts: 3`** — that step has a flaky upstream. Suggest the user check rate limits, network, or the underlying service.
- **`cost.by_kind.llm` is the entire `total_eur`** — expected in v1, since other kinds are free (local execution). When LLM cost is high, suggest batching larger, switching to a smaller model, or caching outputs.
- **A step ran but `output_summary` is empty** — usually a no-op (e.g. `ensure_fr_tab` when the tab already exists). Mention this if relevant ("the FR tab already existed, so step 4 was a no-op").
- **`trigger.source: "scheduled"`** — this run was triggered by the cron schedule defined in the manifest. The user wasn't in the loop, so there's no human to ask about retries; report and let the user decide.

## Don'ts

- Don't dump raw JSON unless the user explicitly asks for it.
- Don't invent details that aren't in the trace.
- Don't claim a step succeeded when its `status` says otherwise.
- Don't suggest fixes you can't back up from the manifest notes or the error message.
- Don't apologize for failures. Report them and offer next steps.
- Don't refer to "Cori cost" — costs in v1 are pass-through provider costs. Frame them as "LLM cost" or "provider cost."

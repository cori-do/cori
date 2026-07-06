# Manifest schema — YAML frontmatter spec

Every Cori workflow has a `manifest.md` at its root. The file is split into two parts:

- **YAML frontmatter** (between `---` lines at the top) — machine-readable metadata that `cori` parses
- **Prose body** — human-readable goal, preconditions, step descriptions, verification, notes

This file documents the frontmatter. The prose body structure is in the main SKILL.md.

## Required fields

| Field | Type | Notes |
|---|---|---|
| `id` | string (snake_case) | Unique within the user's workflow collection. Derived from `name` if not specified, but always set it explicitly to avoid surprises. Max 64 chars. |
| `name` | string | Human-readable. Title-case-ish, but write it like you'd write a function name aloud. |
| `description` | string | One sentence. Used by `search_workflow` for ranking. Make it descriptive of *what* and *when*, not how. |
| `created` | ISO 8601 date | The day the workflow was first written. |
| `version` | integer | Starts at 1, increments on edits. |

## Optional fields

| Field | Type | Notes |
|---|---|---|
| `updated` | ISO 8601 date | Set on edits. |
| `parameters` | list of parameter objects (see below) | Workflow inputs that can change per run. |
| `tools_required` | list of strings | CLI binaries the workflow expects (e.g. `gws`, `kubectl`, `gh`). Used at registration to fail early if a worker lacks a binary. |
| `mcp_servers` | list of strings | MCP server names the workflow expects (e.g. `slack`, `github`). Used at registration to fail early if the worker isn't configured for a server. |
| `tags` | list of strings | Helps grouping in `list_workflow`. Common tags: `deploy`, `data`, `report`, `compliance`, `support`. |
| `schedule` | string (cron) | If set, registers a scheduled trigger. e.g. `"0 3 * * *"` for daily at 03:00. Cron interpreted in `schedule_tz`. |
| `schedule_tz` | string (IANA tz) | Default `UTC`. Use `"Europe/Paris"`, `"America/Los_Angeles"`, etc. |

## Parameter object

```yaml
parameters:
  - name: target_env             # snake_case, used in step files as {target_env}
    type: enum                   # string | number | boolean | enum | path
    values: [dev, staging, prod] # required for enum
    default: staging             # optional but encouraged
    description: Which environment to deploy to
    required: true               # default true; set false for genuinely optional
```

### Parameter types

| Type | Notes |
|---|---|
| `string` | Freeform text. Use for ids, names, messages. |
| `number` | Integer or float. Add `min` / `max` if meaningful. |
| `boolean` | `true` / `false`. Use for feature flags. |
| `enum` | One of a closed set. **Always provide `values`.** Prefer enum over string when the set is known — gives the user a picker at trigger time. |
| `path` | File or directory path. Validated at trigger time when possible. |

### Conventions

- One parameter per concept. If you find yourself adding `start_date` and `end_date_optional`, you have two parameters, not one weird one.
- Defaults derived from the original run. If the user ran with `target_env=staging` once, that becomes the default.
- `required: false` means the step files must handle the parameter being undefined. Wire this through the step's input schema (`z.string().optional()`).
- Don't over-parametrize. Three good parameters beat ten clever ones.

## Full example

```yaml
---
id: translate_product_sheets_fr
name: Translate Product Sheets to French with GPSR Check
description: Localize EN product rows to FR in a Google Sheets tab and append strict GPSR compliance status per row.
created: 2026-05-24
version: 1
parameters:
  - name: spreadsheet_id
    type: string
    default: 1_i5iOB7t0cW6-OSyQtdOWSiAUrO3bwxjF-tSwjFQRSA
    description: Target Google Sheets spreadsheet ID
  - name: source_tab
    type: string
    default: E-commerce Product Technical Sheets
    description: Source tab with the English rows
  - name: target_tab
    type: string
    default: E-commerce Product Technical Sheets (FR)
    description: Tab to create or update with French rows + GPSR columns
  - name: dry_run
    type: boolean
    default: false
    description: If true, write nothing back to the spreadsheet
    required: false
tools_required: [gws]
mcp_servers: []
tags: [translation, compliance, e-commerce]
schedule: "0 3 * * *"
schedule_tz: Europe/Paris
---

# Translate Product Sheets to French with GPSR Check

## Goal
Produce a French version of the source product tab in the same spreadsheet, preserving identifiers and numeric values, and append a strict GPSR compliance review (Check + Invalid reason columns) for each row.

## Preconditions
- The `gws` CLI is installed on the worker and authenticated with write access to the spreadsheet
- The source tab exists and is non-empty
- The user understands the strict GPSR rule: rows are NOK when responsible operator details or French safety/warning info are missing

## Steps
1. **read_source_rows** (cli) — Read the source tab so downstream steps can translate without re-reading
2. **translate_rows** (llm) — Translate human-readable fields to French; preserve SKUs, dimensions, prices
3. **check_gpsr** (code) — Apply the strict rule; emit OK/NOK + reason per row
4. **ensure_fr_tab** (cli) — Create the target tab if it doesn't exist (idempotent)
5. **write_results** (cli) — Write the translated rows + Check + Invalid reason columns

## Verification
- The target tab exists in the spreadsheet
- Row count in target equals row count in source
- Every row in target has a non-empty Check value (OK or NOK)
- Identifier columns (SKU, UPC) match between source and target row-for-row

## Notes
- The first version of this workflow tried to translate row-by-row in step 2; it was 30× slower and hit rate limits. Batched 50/call is the right size for gpt-4o-mini at typical row sizes.
- "Strict" GPSR check means missing operator contact alone is enough for NOK — don't soften this without explicit instruction.
- `dry_run: true` runs everything except step 5, useful when iterating on translation prompts.
```

## Validation rules `cori` enforces

When you run `cori run <path>` (or `cori check <path>`), the compiler checks:

- All required frontmatter fields are present
- Every parameter has a unique name
- `enum` parameters have `values`
- `tools_required` and `mcp_servers` are arrays of strings
- `schedule` parses as a valid cron expression
- Each TypeScript step file compiles and exports a valid `step.<kind>({…})` default
- Step files referenced in `## Steps` exist in `steps/` and are numbered correctly

If any of these fail, compilation is rejected with a structured error. Surface those errors plainly to the user — they're usually one-line fixes.

## Routing (computed, not authored)

The compiler also infers a `Placement` for each step from its kind and `tools_required` / `mcp_servers` use — `Anywhere` for pure `code`/`llm`, `RequiresLocalFs` for `cli` or `code` that reads/writes the workspace, `RequiresCapability { id }` for `mcp_tool` and known-remote CLIs. The CLI's planner then maps each step to a concrete task queue (`cori.user.<id>` or `cori.service.<pool>`) before the workflow starts; the trace records the chosen `task_queue` + `worker_identity` per activity. **Authors do not write placement directly** — declaring `tools_required` / `mcp_servers` is enough.

## What not to put in frontmatter

- **Secrets, tokens, credentials.** Ever. The worker reads these from its environment at runtime.
- **LLM provider names.** The `model` field in an `llm` step declares the model class; the actual provider is configured at the worker level.
- **Large objects.** If your default for a parameter is a 50-line JSON blob, it's not a default — it's a fixture. Put it in `tests/fixtures/` and reference it from the step.
- **Workflow logic.** Frontmatter is metadata. The logic lives in the TypeScript step files.

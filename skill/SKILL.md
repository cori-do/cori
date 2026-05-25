---
name: cori
description: Capture work done in this conversation as a reusable, executable Cori workflow, list/search/trigger existing Cori workflows, and reason about Cori run traces. Use whenever the user types `save_workflow`, `list_workflow`, `search_workflow`, or `trigger_workflow`. Also use whenever the user says anything like "save this as a Cori workflow", "turn this into a runbook", "make this re-runnable", "run my Cori workflow X", "what Cori workflows do I have", "run that thing we did last time", or refers to Cori, runbooks, or workflows. Be proactive — when the user has just finished a multi-step task that they're plausibly going to do again (data pipeline, deployment, report generation, ticket triage, content processing, file conversion, API workflow), offer to save it as a Cori workflow at the end. The skill writes runbook directories with TypeScript step files and drives the local `cori` CLI to list, search, and execute them.
---

# Cori

Cori turns one-off agent conversations into deterministic, executable workflows. The thesis: **agents at design time, deterministic execution at runtime.** You (the agent) do the hard thinking once during the conversation; Cori captures the *result* as typed TypeScript step files that run on the Cori worker, with Temporal handling durability under the hood — no LLM in the loop at runtime unless the workflow explicitly needs one.

This skill is the bridge between the agent and Cori. It exposes four commands and a proactive offer pattern. Every command maps to a section below.

| Command | Purpose |
|---|---|
| `save_workflow` | Distill the current conversation into a Cori runbook directory |
| `list_workflow` | Show available Cori workflows |
| `search_workflow "<query>"` | Find the workflow that best matches a natural-language description |
| `trigger_workflow <id>` | Run a Cori workflow, collect parameters, show the plan, execute on approval |

There is also a proactive pattern (no command): when the user has just finished a non-trivial repeatable task, offer to save it. See **Proactive save offer** below.

---

## The mental model you need before doing anything

A Cori workflow is a **directory**, not a single file:

```
~/.cori/runbooks/<workflow_id>/
├── manifest.md              YAML frontmatter + prose: what, why, parameters
├── types.ts                 Shared TypeScript types for step I/O
├── steps/
│   ├── 01_<name>.ts         One TS file per step, numeric prefix = execution order
│   ├── 02_<name>.ts
│   └── ...
└── tests/
    ├── <step>.test.ts       Optional unit tests (esp. for `code` kind)
    └── fixtures/*.json      Real I/O captured from the original conversation
```

Each step file declares exactly one of five **activity kinds**:

- **`cli`** — invokes a CLI binary that's installed on the worker (gws, kubectl, gh, custom). Cori captures stdout/stderr/exit code.
- **`mcp_tool`** — calls a specific MCP tool on a connected MCP server.
- **`code`** — runs a sandboxed TypeScript function. Pure computation, no I/O except inputs/outputs.
- **`llm`** — calls an LLM with a typed prompt template and parses the response to a typed schema.
- **`builtin`** — Cori's own primitives: `map`, `for_each`, `branch`, `wait`, `parallel`. The DAG glue.

The agent's job at `save_workflow` is to:

1. Reverse-engineer the productive work from the conversation
2. Decide what kind each step is
3. Write the typed TS files
4. Capture real inputs/outputs as test fixtures
5. Generate `manifest.md`

The full templates for each activity kind, and the full manifest schema, are in `references/activity_kinds.md` and `references/manifest_schema.md`. **Read both of those before writing your first runbook in a session.** They're the actual reference; this body is the workflow.

---

## `save_workflow`

This is the demanding command. Take time. A bad runbook costs the user hours debugging at 3am; a clean runbook runs for years.

### Step 1: Re-read the conversation

Read the conversation top-to-bottom with one question in mind: *what did the user actually accomplish, and what concrete actions made that happen?*

Sort everything you see into three buckets:

- **Productive actions** — tool calls that worked and advanced the goal. These become steps.
- **Dead ends** — things you tried that the user redirected or that didn't work. These do **not** become steps; the *lesson* may go in the manifest's `## Notes` section.
- **Conversational scaffolding** — questions, confirmations, status updates. Skip.

If the conversation is long, branched, or unclear, ask one short clarifying question before drafting:

> "I'm reading this as: you wanted to {goal}, and the working approach was {summary of productive path}. Sound right?"

One question, then proceed.

### Step 2: Decide what's a parameter

Look at every concrete value in the productive actions — spreadsheet IDs, environment names, paths, dates, thresholds, channel names. For each, ask: *would this value change the next time someone runs this workflow?*

- Changes → **parameter**, give it a snake_case name, a TS type, and a default derived from this run.
- Fixed property of the system → **constant**, leave it inline in the step file.

When in doubt, parametrize. It's cheap to accept a default at trigger time; expensive to discover something is hardcoded mid-run.

### Step 3: Decompose into steps with a kind per step

For each productive action, decide which activity kind it is. Use this decision tree:

```
The action was…
├── a successful MCP tool call?       → mcp_tool
├── a successful shell command?       → cli
├── a model call (translate, classify, summarize, extract)?
│                                     → llm
├── pure data transformation (parse, filter, format, validate, math)?
│                                     → code
└── flow control (loop, branch, fan-out, wait)?
                                      → builtin
```

A few rules that matter:

- **Never put external I/O in a `code` activity.** If the step needs to call an API, hit a file system, or talk to a database, it's a `cli` or `mcp_tool` step — wrap the actual call in the right kind and keep the pure transform separate. This keeps `code` activities sandboxable and testable.
- **Prefer `cli` and `mcp_tool` over `code` when you can.** They reuse tools the user already has installed and authenticated. `code` is for the glue logic between them.
- **`llm` steps must declare a typed output schema.** Free-form text returns from an LLM aren't a Cori step — they're a bug. If you used an LLM in conversation to extract structured info, the step's output type is that structure, and the step's prompt enforces it.

Order the steps by execution sequence. Number filenames `01_`, `02_`, `03_`, …, so a reader scanning the `steps/` directory sees them in order.

### Step 4: Write the files

Create the directory layout. Use the `@cori/sdk` import patterns from `references/activity_kinds.md` — each kind has a template. Each step file:

- Imports the right primitive from `@cori/sdk`
- Declares typed input and output (either inline or from `types.ts`) using zod schemas
- Has a one-line `description` (becomes the activity name in the run trace)
- Returns the activity definition as the default export

Capture real inputs/outputs from the conversation as fixtures under `tests/fixtures/`. For `code` activities, generate a vitest-compatible unit test that uses the fixture and checks the expected output. The tests are vitest-compatible — users who want to verify before running can install vitest and run `npx vitest tests/` in the runbook directory. This is the trust layer for the platform engineer reviewing the runbook later.

### Step 5: Write the manifest

`manifest.md` is the human-readable face of the workflow. It has YAML frontmatter (metadata that `cori` reads) and prose (that the user reads). The full schema is in `references/manifest_schema.md`. The minimum is:

```yaml
---
id: <snake_case_id>
name: <Human Readable Name>
description: <one sentence — what it does and when to use it>
created: <YYYY-MM-DD>
version: 1
parameters:
  - name: <param_name>
    type: string | number | boolean | enum | path
    default: <value>
    description: <one line>
tools_required: [<cli names>]
mcp_servers: [<server names>]
tags: [<a few>]
---

# <Human Readable Name>

## Goal
<2–3 sentences on what success looks like>

## Preconditions
- <thing that must be true before running>

## Steps
1. **<step name>** (<kind>) — <one sentence on why this step exists>
2. ...

## Verification
- <how to confirm it worked>

## Notes
- <lessons, gotchas, edge cases — including dead-ends from the original conversation if they're useful warnings>
```

Write the prose for a competent reader who wasn't in the original conversation. Explain *why* each step exists, not just what it does.

### Step 6: Show the user before committing

Before writing anything to disk, show the user the **directory tree** and the **manifest.md content**, and ask:

> "Here's the runbook I'd save. Want me to write it to `~/.cori/runbooks/<id>/`? (yes / edit / cancel)"

If they say edit, ask what to change, re-show, re-ask. If they say yes, write the files, then call:

```bash
cori workflows register ~/.cori/runbooks/<id>
```

This validates the runbook (type-checks the TS, validates the manifest, registers it locally) and prints any errors. Surface errors back to the user in plain language — don't just dump the raw `cori` output. If validation fails, offer to fix.

### Step 7: Confirm and suggest next action

After successful registration, tell the user:

> "Saved as `<id>`. Try a dry run with `cori run <id> --dry-run`, or trigger from here with `trigger_workflow <id>`."

Don't auto-run. Saving and running are separate decisions.

---

## `list_workflow`

Run:

```bash
cori workflows list --json
```

Parse the JSON and present a compact table: **id**, **name**, **description**, **last run** (relative time, e.g. "3h ago"), **success rate (7d)**. Sort by last-run descending.

If the user has no workflows, say so plainly and suggest they run `save_workflow` at the end of their next repeatable task. Don't apologize.

If `cori` isn't on the PATH, surface that clearly and tell the user the exact command to install: `curl -fsSL https://cli.cori.do/install.sh | bash`.

---

## `search_workflow "<query>"`

Semantic search, not keyword match. The user's query is a *description of what they want to do*; your job is to find the workflow whose purpose matches.

1. Run `cori workflows list --json` to get all available workflows with their manifests' name + description + tags.
2. If there are ≤20 workflows, also pull each manifest's `## Goal` section via `cori workflows show <id> --field=goal --json` — it's cheap and gives much better matches.
3. Rank by semantic fit to the query, not lexical overlap. "the thing we do when the API throws 5xx" should match a workflow called "incident_elevated_error_rate" even with zero shared words.
4. Return the top 1–3 matches with a one-line rationale per match ("matches because: handles the same symptom").
5. If nothing is a reasonable fit, say so honestly — don't force a match.

End with: "Run the best match with `trigger_workflow <id>`."

---

## `trigger_workflow <id>`

Load, plan, confirm, execute. **Never skip the confirmation step**, even for trivial workflows. The user-in-the-loop is the whole safety model.

### Step 1: Load

If the user provided params inline (`trigger_workflow translate_sheets spreadsheet_id=ABC123`), capture them. Then:

```bash
cori workflows show <id> --json
```

This returns the manifest, including parameter declarations and defaults. If the id isn't found, Cori suggests close matches — surface them and ask the user which they meant.

### Step 2: Collect parameters

For each parameter in the manifest:

- If provided in the trigger command, use that value.
- Otherwise, prompt the user, showing the description, type, allowed values (for enums), and default. Accept the default if they say "default" or just press enter.

If there are more than 4 parameters, batch the prompts. Use `ask_user_input_v0` for enums where it makes sense — tappable options are faster than typing.

### Step 3: Show the materialized plan

Substitute the parameters into a plan summary — exact values, no `{placeholders}` left visible. Format as a numbered list (one line per step) with the activity kind in parens:

```
1. read_source_rows   (cli)      — gws sheets values get on ABC123!Sheet1
2. translate_rows     (llm)      — gpt-4o-mini, batched 50/call
3. check_gpsr         (code)     — validate against strict rules
4. ensure_fr_tab      (cli)      — create "Sheet1 (FR)" if missing
5. write_results      (cli)      — write translated rows + check columns

Estimated LLM cost: ~€0.018 per run (gpt-4o-mini, pass-through to OpenAI).
```

Frame any cost numbers as **provider pass-through cost**, not Cori cost. Cori v1 doesn't charge for execution — the cost number is the LLM provider's bill.

Then ask explicitly: **"Run this plan? (yes / no / dry-run / edit)"**

- **yes** → execute (Step 4)
- **dry-run** → run with `--dry-run`, show the plan with mock I/O, then ask again
- **no** → stop, confirm nothing was changed
- **edit** → ask what to change, re-materialize, re-ask

### Step 4: Execute

```bash
cori run <id> --json [<param>=<value> …]
```

Cori streams progress to stderr (which the user sees in their terminal) and emits a structured trace to stdout. The trace shape is in `references/trace_interpretation.md` — read that file the first time you handle a trace in a session, because the schema is precise and you need to surface the right things.

In short, after the run completes:

- If success: report the workflow id, total duration, total LLM cost, and a one-line summary of what changed (taken from the workflow's effects, not invented).
- If failure: identify the failed step, surface its error message, and check the manifest's `## Notes` section for relevant guidance. Ask the user whether to retry, skip the failed step, or abort.

Don't dump the raw JSON trace at the user. Synthesize.

If the user wants the full per-activity payload of a past run, use:

```bash
cori runs show <run_id> [--activity <activity_id>] [--full]
```

---

## Proactive save offer (no command)

When the user has just completed a non-trivial task that looks repeatable, offer — once, briefly — to save it as a Cori workflow. Triggers include:

- A multi-step task involving ≥2 tool calls that produced a clear result
- The user said something like "great, that worked", "perfect", "done"
- The task touches a recurring concern: a recurring report, a data pipeline, an integration, a content processing flow, an onboarding/offboarding action

Before offering, silently check if Cori is installed (`cori --version`).

- **If Cori is installed:** Offer with a single line:
  > "Want me to save this as a Cori workflow so you can re-run it? Just say `save_workflow`."
- **If Cori is not installed:** Offer with the install path:
  > "If you install Cori, you can save this as a reusable workflow: `curl -fsSL https://cli.cori.do/install.sh | bash`."

Do not auto-install. Do not insist.

**Do not offer to save** if any of these are true:

- The user asked a single-question information lookup (no automation value)
- The task involved exploratory back-and-forth where no clean procedure emerged
- The user already declined a save offer earlier in the same conversation
- The task was a one-off (account recovery, debugging a specific incident with no general pattern)

One offer per conversation, max. Don't nag.

---

## If the user isn't sure Cori is working

Suggest `cori demo` — it runs a 3-step example workflow in under 5 seconds with no credentials required. Useful as a smoke test after install or when the user reports something feels broken.

---

## When things go wrong

A few common failure modes and what to do:

**`cori` not installed.** Tell the user to install with `curl -fsSL https://cli.cori.do/install.sh | bash`. Don't try to proceed without it. (Note: `cori login` is optional in v1 — the CLI works fully without an account.)

**TypeScript compile error after writing step files.** Read the error, locate the offending step file, fix it (most often a type mismatch, a missing import from `@cori/sdk`, or a wrong zod schema). Then re-register with `cori workflows register <path>`. If the error is in code derived from the conversation, explain what was wrong and the fix.

**CLI binary referenced in a `cli` step is not installed on the worker.** Surface clearly: "step 2 calls `gws`, but it isn't installed on the worker. Run `cori workers status` to see what's available." Suggest the fix (install the binary, or restructure the step).

**MCP server referenced in a step is not connected on the worker.** Same pattern: surface clearly, point at `cori workers status`, suggest the fix (configure the MCP server in worker config, or restructure the step).

**A step kind looks wrong on review.** It's better to re-decompose than to ship a workflow with an `llm` step that should have been `code` (or vice versa). If you spot a misclassification at Step 6 (the review step), fix it before writing to disk.

---

## References — read these when relevant

- `references/activity_kinds.md` — full TypeScript templates for each of the five activity kinds. Read before writing your first step file in a session.
- `references/manifest_schema.md` — full YAML frontmatter spec, all parameter types, validation rules. Read before writing your first manifest in a session.
- `references/trace_interpretation.md` — the JSON shape `cori run --json` emits and what to surface vs. omit. Read before interpreting your first run trace in a session.
- `references/example_runbook.md` — a complete, realistic worked example (translate product sheets with a GPSR check). Read for a concrete model of what good output looks like.

---

## Design notes — internalize these so you don't drift

- **The runbook is documentation first, automation second.** A clean manifest read by a human ten times and executed twice is still a win. Write the prose so it stands alone.
- **Most workflows have zero LLM steps at runtime.** That's the point. The LLM (you) did the work at design time. Only insert an `llm` step where the *runtime data* genuinely needs a model: translating new product descriptions, classifying new tickets, summarizing new documents. Don't habitually reach for `llm` because you used a model to figure out the workflow.
- **Don't over-parametrize.** Three well-chosen parameters beat ten clever ones. If a parameter makes the manifest harder to read, leave the value inline.
- **The user is the safety mechanism.** The Step-6 review (before disk write) and the Step-3 plan confirmation (before execution) are the spine of trust. Never collapse them into one step. Never run a workflow you didn't show the user first.
- **Conversations are messy; runbooks are clean.** When saving, do the work of cleaning up. Don't preserve the meandering; preserve the distilled procedure.
- **Be honest about what failed.** If validation fails, say so plainly. If a step is wrong, say so. The user values truth over polish.
- **Cori v1 is local-only.** There's one worker running on the user's machine. No cloud workers, no on-prem/serverless routing distinctions. If you need to talk about "where a step runs," it runs on the local Cori worker.

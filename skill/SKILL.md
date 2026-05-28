---
name: cori
description: Capture work done in this conversation as a reusable, executable Cori workflow, and drive the local `cori` CLI to run, inspect, and manage workflows. Use whenever the user types `save_workflow`, `list_workflow`, `search_workflow`, or `trigger_workflow`. Also use whenever the user says anything like "save this as a Cori workflow", "make this re-runnable", "run my Cori workflow X", "what Cori workflows do I have", or refers to Cori workflows. Be proactive — when the user has just finished a multi-step task that they're plausibly going to do again (data pipeline, deployment, report generation, ticket triage, content processing, file conversion, API workflow), offer to save it as a Cori workflow at the end. The skill writes workflow directories with TypeScript step files and shells out to the `cori` CLI to validate, run, and inspect them.
---

# Cori

Cori turns one-off agent conversations into deterministic, executable workflows. The thesis: **agents at design time, deterministic execution at runtime.** You (the agent) do the hard thinking once during the conversation; Cori captures the *result* as typed TypeScript step files that run on a Cori worker, with Temporal handling durability under the hood — no LLM in the loop at runtime unless the workflow explicitly needs one.

A Cori workflow is a **folder on disk**. There is no registry. You run a workflow by giving `cori run` a path (or a git ref). The folder can live anywhere — typically inside a git repo the user already owns. The skill's primary job is to *create* that folder cleanly from the conversation.

This skill exposes one demanding command (`save_workflow`) and three thin pass-throughs to the CLI. There's also a proactive offer pattern. Every command maps to a section below.

| Command | Purpose |
|---|---|
| `save_workflow` | Distill the current conversation into a Cori workflow directory |
| `list_workflow` | Show workflows on disk + recent run history |
| `search_workflow "<query>"` | Find the workflow folder that best matches a natural-language description |
| `trigger_workflow <path-or-ref>` | Run a Cori workflow, collect parameters, show the plan, execute on approval |

There is also a proactive pattern (no command): when the user has just finished a non-trivial repeatable task, offer to save it. See **Proactive save offer** below.

---

## The Cori CLI in one screen

Eight verbs. Three take a workflow path *or* remote git ref (`host/owner/repo[/subpath][@ref]`); the rest are machine-scoped.

```
cori run   <path-or-ref> [--json] [--dry-run] [--update] [--yes] [<param>=<value>...]
cori check <path-or-ref> [--update] [--yes]      # validate + preflight only
cori show  <path-or-ref>                         # inspect workflow + recent runs

cori runs  list|show                             # browse run history (JSON traces)
cori work  [--shared <pool>]                     # stay online as a worker
cori login <capability>                          # OAuth/CLI sign-in
cori status                                      # endpoint, identity, workers, caps
cori config get|set                              # ~/.cori/config.toml access
cori skill install                               # install the Cori agent skill
```

Key behaviours to remember:

- `cori run ./my_workflow` resolves the folder, compiles to `~/.cori/cache/`, plans, and executes on a Temporal worker (auto-spawning a local `temporal server start-dev` if no endpoint is configured).
- Remote refs use `go mod`-style syntax: `github.com/acme/workflows/report@v1.2`. Refless picks the highest semver tag; `@v1` / `@v1.2` pick the highest matching prefix; exact tags and 7+ hex shas are immutable. SSH form `git@host:repo[@ref]` also works.
- `--update` re-resolves mutable refs; `--yes` (or env `CORI_ASSUME_YES=1`) skips the first-run consent prompt for a remote ref.
- Workflows are folders, not ids — there is no `cori workflows list`, `register`, `init`, or `save`. Everything is path-based.
- Every run writes a JSON trace to `~/.cori/runs/<key>/<utc>.json`. The key is `<folder>-<8hex(absolute_path)>` for local workflows, or `<repo-leaf>-<8hex(host/repo//subpath)>` for remote.

If `cori --version` fails, surface the install path: `curl -fsSL https://cli.cori.do/install.sh | bash`. Don't try to proceed without the binary.

---

## What a workflow folder looks like

```
<workflow_name>/
├── manifest.md              YAML frontmatter + prose: what, why, parameters
├── types.ts                 (optional) shared TypeScript types for step I/O
├── steps/
│   ├── 01_<name>.ts         one TS file per step, numeric prefix = execution order
│   ├── 02_<name>.ts
│   └── ...
└── tests/                   (optional) vitest tests + captured fixtures
    ├── <step>.test.ts
    └── fixtures/*.json
```

Each step file declares exactly one of five **activity kinds**:

- **`cli`** — invokes a CLI binary on the worker (`gws`, `kubectl`, `gh`, …). Cori captures stdout/stderr/exit code.
- **`mcp_tool`** — calls a specific tool on a connected MCP server.
- **`code`** — runs a sandboxed TypeScript function. Pure computation, no I/O except inputs/outputs.
- **`llm`** — calls an LLM with a typed prompt template and parses the response against a typed schema.
- **`builtin`** — Cori's own primitives (`map`, `for_each`, `branch`, `parallel`, `wait`). The DAG glue. **Note:** the compiler accepts these but the v1 runtime does not yet execute them — avoid emitting builtins unless the user has confirmed they understand it's deferred.

The full TS template for each kind is in [`references/activity_kinds.md`](references/activity_kinds.md). The full manifest schema is in [`references/manifest_schema.md`](references/manifest_schema.md). **Read both before writing your first workflow in a session.**

---

## `save_workflow` — the demanding command

This is the work. Take time. A bad workflow costs the user hours debugging at 3am; a clean workflow runs for years.

### Step 1: Re-read the conversation

Read top-to-bottom with one question: *what did the user actually accomplish, and what concrete actions made that happen?*

Sort everything into three buckets:

- **Productive actions** — tool calls that worked and advanced the goal. These become steps.
- **Dead ends** — things that didn't work or were redirected. Do **not** become steps; the *lesson* may go in the manifest's `## Notes` section.
- **Conversational scaffolding** — questions, confirmations, status updates. Skip.

If the conversation is long, branched, or unclear, ask one short clarifying question before drafting:

> "I'm reading this as: you wanted to {goal}, and the working approach was {summary}. Sound right?"

One question, then proceed.

### Step 2: Decide where the folder lives

Workflows are owned by the user. Ask where to put the folder. Default suggestions, in order of preference:

1. The git repo the user is currently working in, under `workflows/<snake_case_name>/` or similar.
2. A sibling folder of any existing workflow you can see in the repo.
3. `~/cori-workflows/<snake_case_name>/` as a last resort.

Never write to `~/.cori/` — that's Cori's own state directory. Workflow folders are user-owned.

### Step 3: Decide what's a parameter

Look at every concrete value in the productive actions — spreadsheet IDs, environment names, paths, dates, thresholds, channel names. For each, ask: *would this value change the next time someone runs this workflow?*

- Changes → **parameter**: snake_case name, TS type, default derived from this run.
- Fixed property of the system → **constant**: leave inline in the step file.

When in doubt, parametrize. It's cheap to accept a default at trigger time; expensive to discover something is hardcoded mid-run.

### Step 4: Decompose into steps with a kind per step

For each productive action, decide the activity kind:

```
The action was…
├── a successful MCP tool call?       → mcp_tool
├── a successful shell command?       → cli
├── a model call (translate, classify, summarize, extract)?
│                                     → llm
├── pure data transformation (parse, filter, format, validate, math)?
│                                     → code
└── flow control (loop, branch, fan-out, wait)?
                                      → builtin   (deferred in v1 — flag this)
```

Rules that matter:

- **Never put external I/O in a `code` activity.** If it needs a network, filesystem, or DB call, it's a `cli` or `mcp_tool`. Wrap the call in the right kind and keep the pure transform separate.
- **Prefer `cli` and `mcp_tool` over `code` when you can.** They reuse tools the user already has installed and authenticated.
- **`llm` steps must declare a typed output schema.** Free-form text returns aren't a Cori step — they're a bug. If you used an LLM in the conversation to extract structured info, the step's output type *is* that structure, and the prompt enforces it.
- **Capabilities are mandatory.** Any `cli` step that uses `gws` must declare `tools_required: [gws]`. Any `mcp_tool` step must declare its server in `mcp_servers`. The compiler enforces this — placement inference depends on it.

Order the steps. Number filenames `01_`, `02_`, `03_`, … so the `steps/` directory reads in execution order.

### Step 5: Write the files

Create the directory layout. Use the `@cori/sdk` templates from [`references/activity_kinds.md`](references/activity_kinds.md). Each step file:

- Imports the right primitive from `@cori/sdk` (`step.cli`, `step.code`, `step.mcp_tool`, `step.llm`)
- Declares typed `input` and `output` as zod schemas (in the file or imported from `types.ts`)
- Has a one-line `description` (becomes the activity name in the run trace)
- Default-exports the `step.<kind>({…})` call

Capture real I/O from the conversation as fixtures under `tests/fixtures/`. For `code` activities, generate a vitest-compatible unit test that pins the expected output. Users who want to verify before running can `npx vitest tests/` inside the workflow directory. This is the trust layer for whoever reviews the workflow later.

### Step 6: Write the manifest

`manifest.md` is the human-readable face of the workflow. YAML frontmatter for metadata (parsed by `cori`); prose for humans. Full schema: [`references/manifest_schema.md`](references/manifest_schema.md). Minimum:

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
- <lessons, gotchas, edge cases — including useful warnings from dead-ends in the original conversation>
```

Write the prose for a competent reader who wasn't in the original conversation. Explain *why* each step exists, not just what.

### Step 7: Show the user before committing

Before writing anything to disk, show the user the **directory tree** and the **manifest.md content**, and ask:

> "Here's the workflow I'd save to `<chosen_path>/`. Want me to write it? (yes / edit / cancel)"

If they say edit, ask what to change, re-show, re-ask. If they say yes, write the files, then validate:

```bash
cori check <chosen_path>
```

`cori check` parses the manifest, statically analyses every step file, validates that declared `tools_required` / `mcp_servers` match actual usage, and runs preflight. It does **not** execute. Surface any errors back to the user in plain language — don't just dump raw `cori` output. If validation fails, offer to fix.

### Step 8: Confirm and suggest next action

After `cori check` is green:

> "Saved to `<chosen_path>/`. Try a dry run with `cori run <chosen_path> --dry-run`, or trigger from here with `trigger_workflow <chosen_path>`."

Don't auto-run. Saving and running are separate decisions.

---

## `list_workflow`

There is no registry, so "list" means: enumerate workflows the user has on disk + any recent runs.

1. Ask the user where their workflows live, or scan a sensible default (current working directory and any `workflows/` subfolder). Treat any folder containing a `manifest.md` as a workflow folder.
2. For each candidate, parse its frontmatter (id, name, description, tags) and present a compact table: **name**, **path**, **description**, **last run** (relative time).
3. For "last run", check `~/.cori/runs/` — the directory name pattern is `<folder>-<8hex(abs_path)>` for local workflows. Use `cori runs list --json` to enumerate run history.
4. Sort by last-run descending; folders that have never run go last.

If the user has no workflows on disk, say so plainly and suggest they run `save_workflow` at the end of their next repeatable task. Don't apologize.

---

## `search_workflow "<query>"`

Semantic search, not keyword match. The user's query is a *description of what they want to do*; find the workflow folder whose purpose matches.

1. Enumerate workflows on disk as in `list_workflow`.
2. For each, read the manifest frontmatter (`name`, `description`, `tags`) and the prose `## Goal` section.
3. Rank by semantic fit to the query, not lexical overlap. "the thing we do when the API throws 5xx" should match a workflow called `incident_elevated_error_rate` even with zero shared words.
4. Return the top 1–3 matches with a one-line rationale per match.
5. If nothing is a reasonable fit, say so honestly — don't force a match.

End with: "Run the best match with `trigger_workflow <path>`."

---

## `trigger_workflow <path-or-ref>`

Load, plan, confirm, execute. **Never skip the confirmation step**, even for trivial workflows. The user-in-the-loop is the safety model.

### Step 1: Load

If the user provided params inline (`trigger_workflow ./translate_sheets spreadsheet_id=ABC123`), capture them. Then:

```bash
cori show <path-or-ref>
```

This prints the manifest (parameters, tools, mcp_servers, prose) and a summary of recent runs. If the path doesn't resolve, surface the error.

### Step 2: Collect parameters

For each parameter in the manifest:

- If provided in the trigger command, use that value.
- Otherwise, prompt the user, showing the description, type, allowed values (for enums), and default. Accept the default if they say "default" or press enter.

If there are more than 4 parameters, batch the prompts.

### Step 3: Show the materialized plan

Substitute the parameters into a plan summary — exact values, no `{placeholders}` visible. Number the steps with the kind in parens:

```
1. read_source_rows   (cli)  — gws sheets values get on ABC123!Sheet1
2. translate_rows     (llm)  — gpt-4o-mini, batched 50/call
3. check_gpsr         (code) — validate against strict rules
4. ensure_fr_tab      (cli)  — create "Sheet1 (FR)" if missing
5. write_results      (cli)  — write translated rows + check columns

Estimated LLM cost: ~€0.018 per run (gpt-4o-mini, pass-through to OpenAI).
```

Frame any cost as **provider pass-through cost**, not Cori cost. Cori v1 doesn't charge for execution.

Then ask explicitly: **"Run this plan? (yes / no / dry-run / edit)"**

- **yes** → execute (Step 4)
- **dry-run** → `cori run <path> --dry-run …` and show the result, then ask again
- **no** → stop, confirm nothing changed
- **edit** → ask what to change, re-materialize, re-ask

### Step 4: Execute

```bash
cori run <path-or-ref> --json [<param>=<value> …]
```

Cori streams progress to stderr (visible to the user) and emits a JSON trace to stdout. The trace shape is in [`references/trace_interpretation.md`](references/trace_interpretation.md) — read it the first time you handle a trace in a session.

After the run:

- **Success**: report the workflow name, total duration, total LLM cost, and a one-line summary of what changed (taken from the activity outputs, not invented).
- **Failure**: identify the failed step, surface its error message, and check the manifest's `## Notes` for guidance. Ask the user whether to retry, skip, or abort.
- **NeedsReauth**: a step requires sign-in. Tell the user to run `cori login <capability>` in another terminal; the workflow is suspended and will resume automatically (up to 15 min, configurable via `CORI_REAUTH_TIMEOUT_SECS`).

Don't dump the raw JSON. Synthesize.

For the full per-activity payload of a past run, use:

```bash
cori runs show <run_id> [--activity <activity_id>] [--full]
```

---

## Proactive save offer (no command)

When the user has just completed a non-trivial task that looks repeatable, offer — once, briefly — to save it. Triggers:

- A multi-step task involving ≥2 tool calls that produced a clear result
- The user said something like "great, that worked", "perfect", "done"
- The task touches a recurring concern: a recurring report, a data pipeline, an integration, a content processing flow, an onboarding/offboarding action

Before offering, silently check `cori --version`.

- **If Cori is installed:** offer with one line:
  > "Want me to save this as a Cori workflow so you can re-run it? Just say `save_workflow`."
- **If Cori is not installed:** offer with the install path:
  > "If you install Cori, you can save this as a reusable workflow: `curl -fsSL https://cli.cori.do/install.sh | bash`."

Do not auto-install. Do not insist.

**Do not offer** when:

- The user asked a single-question information lookup (no automation value)
- The task involved exploratory back-and-forth where no clean procedure emerged
- The user already declined a save offer earlier in the same conversation
- The task was a one-off (account recovery, debugging a specific incident with no general pattern)

One offer per conversation, max. Don't nag.

---

## When things go wrong

**`cori` not installed.** Install: `curl -fsSL https://cli.cori.do/install.sh | bash`. Don't proceed without it.

**`cori check` fails on a TS step file.** Read the error, locate the offending step, fix it (most often a type mismatch, missing `@cori/sdk` import, or wrong zod schema), re-run `cori check`.

**`cori check` says a CLI binary is missing from `tools_required`.** The compiler enforces the declaration. Add the binary to the manifest's `tools_required` list and re-check.

**`cori run` fails with "no worker on queue …".** A step requires a capability that no worker on this machine provides. Either start one (`cori work`), or ask the user whether someone else operates a shared pool they should target.

**`NeedsReauth` mid-run.** A credential expired. Tell the user to run `cori login <capability>` in another terminal — the suspended workflow will resume automatically.

**A step kind looks wrong on review.** Better to re-decompose than to ship a workflow with an `llm` step that should have been `code` (or vice versa). Fix at Step 7 (review) before disk write.

---

## References — read these when relevant

- [`references/activity_kinds.md`](references/activity_kinds.md) — full TypeScript templates for each activity kind. Read before writing your first step file in a session.
- [`references/manifest_schema.md`](references/manifest_schema.md) — full YAML frontmatter spec, parameter types, validation rules. Read before writing your first manifest in a session.
- [`references/trace_interpretation.md`](references/trace_interpretation.md) — the JSON shape `cori run --json` emits and what to surface vs. omit. Read before interpreting your first run trace in a session.
- [`references/example_workflow.md`](references/example_workflow.md) — a complete, realistic worked example (translate product sheets with a GPSR check). Read for a concrete model of what good output looks like.

---

## Design notes — internalize these so you don't drift

- **A workflow is a folder, not an id.** No registry, no `cori workflows register`. You run by path or git ref. Always.
- **The workflow is documentation first, automation second.** A clean manifest read by a human ten times and executed twice is still a win. Write the prose so it stands alone.
- **Most workflows have zero LLM steps at runtime.** That's the point. The LLM (you) did the work at design time. Only insert an `llm` step where the *runtime data* genuinely needs a model: translating new product descriptions, classifying new tickets, summarizing new documents.
- **Don't over-parametrize.** Three well-chosen parameters beat ten clever ones. If a parameter makes the manifest harder to read, leave the value inline.
- **The user is the safety mechanism.** The Step-7 review (before disk write) and the Step-3 plan confirmation (before execution) are the spine of trust. Never collapse them. Never run a workflow you didn't show the user first.
- **Conversations are messy; workflows are clean.** When saving, do the work of cleaning up. Don't preserve the meandering; preserve the distilled procedure.
- **Be honest about what failed.** If `cori check` rejects the workflow, say so plainly. If a step is wrong, say so. The user values truth over polish.
- **Builtins are deferred in v1.** The compiler accepts `map` / `for_each` / `branch` / `parallel` / `wait`, but the runtime doesn't execute them yet. If the conversation needs branching or fan-out, flag this to the user before emitting the step — they may prefer a linear workaround for now.

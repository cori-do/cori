---
name: cori_save_workflow
description: Capture work done in this conversation as a reusable, executable Cori workflow. Use whenever the user asks to "save this as a Cori workflow", "turn this into a runbook", "make this re-runnable", or any equivalent phrasing. Be proactive — when the user has just finished a multi-step task that they're plausibly going to do again (data pipeline, deployment, report generation, ticket triage, content processing, file conversion, API workflow), offer to save it as a Cori workflow at the end. The skill writes a workflow directory with TypeScript step files and shells out to the `cori` CLI to validate it.
---

# Cori

Cori turns one-off agent conversations into deterministic, executable workflows. The thesis: **agents at design time, deterministic execution at runtime.** You (the agent) do the hard thinking once during the conversation; Cori captures the *result* as typed TypeScript step files that run on a Cori worker, with Temporal handling durability under the hood — no LLM in the loop at runtime unless the workflow explicitly needs one.

A Cori workflow is a **folder on disk**. There is no registry. You run a workflow by giving `cori run` a path (or a git ref). The folder can live anywhere — typically inside a git repo the user already owns. The skill's job is to *create* that folder cleanly from the conversation.

The skill has one job: distill the current conversation into a Cori workflow directory. It also follows a proactive offer pattern — when the user has just finished a non-trivial repeatable task, offer to save it. See **Proactive save offer** below.

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

## Saving a workflow — the procedure

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

> "Saved to `<chosen_path>/`. Try a dry run with `cori run <chosen_path> --dry-run`, or run it for real with `cori run <chosen_path>`."

Don't auto-run. Saving and running are separate decisions.

---

## Proactive save offer (no command)

When the user has just completed a non-trivial task that looks repeatable, offer — once, briefly — to save it. Triggers:

- A multi-step task involving ≥2 tool calls that produced a clear result
- The user said something like "great, that worked", "perfect", "done"
- The task touches a recurring concern: a recurring report, a data pipeline, an integration, a content processing flow, an onboarding/offboarding action

Before offering, silently check `cori --version`.

- **If Cori is installed:** offer with one line:
  > "Want me to save this as a Cori workflow so you can re-run it?"
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

**A step kind looks wrong on review.** Better to re-decompose than to ship a workflow with an `llm` step that should have been `code` (or vice versa). Fix at Step 7 (review) before disk write.

---

## References — read these when relevant

- [`references/activity_kinds.md`](references/activity_kinds.md) — full TypeScript templates for each activity kind. Read before writing your first step file in a session.
- [`references/manifest_schema.md`](references/manifest_schema.md) — full YAML frontmatter spec, parameter types, validation rules. Read before writing your first manifest in a session.
- [`references/example_workflow.md`](references/example_workflow.md) — a complete, realistic worked example (translate product sheets with a GPSR check). Read for a concrete model of what good output looks like.

---

## Design notes — internalize these so you don't drift

- **A workflow is a folder, not an id.** No registry, no `cori workflows register`. You run by path or git ref. Always.
- **The workflow is documentation first, automation second.** A clean manifest read by a human ten times and executed twice is still a win. Write the prose so it stands alone.
- **Most workflows have zero LLM steps at runtime.** That's the point. The LLM (you) did the work at design time. Only insert an `llm` step where the *runtime data* genuinely needs a model: translating new product descriptions, classifying new tickets, summarizing new documents.
- **Don't over-parametrize.** Three well-chosen parameters beat ten clever ones. If a parameter makes the manifest harder to read, leave the value inline.
- **The user is the safety mechanism.** The Step-7 review (before disk write) is the spine of trust. Never skip it. Never write a workflow to disk you didn't show the user first.
- **Conversations are messy; workflows are clean.** When saving, do the work of cleaning up. Don't preserve the meandering; preserve the distilled procedure.
- **Be honest about what failed.** If `cori check` rejects the workflow, say so plainly. If a step is wrong, say so. The user values truth over polish.
- **Builtins are deferred in v1.** The compiler accepts `map` / `for_each` / `branch` / `parallel` / `wait`, but the runtime doesn't execute them yet. If the conversation needs branching or fan-out, flag this to the user before emitting the step — they may prefer a linear workaround for now.

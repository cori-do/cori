---
name: cori-save-workflow
description: Capture work done in this conversation as a reusable, executable Cori workflow. Use whenever the user asks to "save this as a Cori workflow", "turn this into a runbook", "make this re-runnable", or any equivalent phrasing. Be proactive — when the user has just finished a multi-step task that they're plausibly going to do again (data pipeline, deployment, report generation, ticket triage, content processing, file conversion, API workflow), offer to save it as a Cori workflow at the end. The skill writes a workflow directory with TypeScript step files and validates it via the `cori` CLI — or via Cori's MCP tools (`cori__check` / `cori__run` under any client prefix) when no CLI is available, e.g. in cloud sandboxes.
---

# Cori

Cori turns one-off agent conversations into deterministic, executable workflows. The thesis: **agents at design time, deterministic execution at runtime.** You (the agent) do the hard thinking once during the conversation; Cori captures the *result* as typed TypeScript step files that run on a Cori worker, with Temporal handling durability under the hood — no LLM in the loop at runtime unless the workflow explicitly needs one.

A Cori workflow is a **folder on disk**. There is no registry. You run a workflow by giving `cori run` a path (or a git ref). The folder can live anywhere — typically inside a git repo the user already owns. The skill's job is to *create* that folder cleanly from the conversation.

The skill has one job: distill the current conversation into a Cori workflow directory. It also follows a proactive offer pattern — when the user has just finished a non-trivial repeatable task, offer to save it. See **Proactive save offer** below.

---

## The Cori CLI in one screen

Nine verbs. Three take a workflow path *or* remote git ref (`host/owner/repo[/subpath][@ref]`); the rest are machine-scoped.

```
cori run   <path-or-ref> [--json] [--dry-run] [--update] [--yes] [<param>=<value>...]
cori check <path-or-ref> [--update] [--yes]      # validate + preflight only
cori show  <path-or-ref>                         # inspect workflow + recent runs

cori runs  list|show                             # browse run history (JSON traces)
cori work  [--shared <pool>]                     # stay online as a worker
cori login <capability>                          # OAuth/CLI sign-in
cori status                                      # endpoint, identity, workers, caps
cori config get|set                              # ~/.cori/config.toml access
cori mcp                                         # serve check/run/show/runs/status as MCP tools
```

(The Cori agent skill itself is installed via `npx skills add cori-do/cori`.)

Key behaviours to remember:

- `cori run ./my_workflow` resolves the folder, compiles to `~/.cori/cache/`, plans, and executes on a Temporal worker (auto-spawning a local `temporal server start-dev` if no endpoint is configured).
- Remote refs use `go mod`-style syntax: `github.com/acme/workflows/report@v1.2`. Refless picks the highest semver tag; `@v1` / `@v1.2` pick the highest matching prefix; exact tags and 7+ hex shas are immutable. SSH form `git@host:repo[@ref]` also works.
- `--update` re-resolves mutable refs; `--yes` (or env `CORI_ASSUME_YES=1`) skips the first-run consent prompt for a remote ref.
- Workflows are folders, not ids — there is no `cori workflows list`, `register`, `init`, or `save`. Everything is path-based.
- Every run writes a JSON trace to `~/.cori/runs/<key>/<utc>.json`. The key is `<folder>-<8hex(absolute_path)>` for local workflows, or `<repo-leaf>-<8hex(host/repo//subpath)>` for remote.

**If `cori --version` fails, check for Cori MCP tools before giving up.** Cori may be reachable even where the CLI isn't: `cori mcp` on the user's machine exposes `check` / `run` / `show` / `runs_list` / `runs_show` / `status` as MCP tools (possibly namespaced by your client, e.g. `cori__check` or `mcp__…__cori__check`). They take the same arguments and return the same data — use them as drop-in replacements for the CLI calls in this skill. Two rules when working through them:

- The `source` path you pass must exist **on the machine where Cori runs** (the user's device), not in your sandbox. Write the workflow folder into a location the user owns and that machine can see, and pass *that* path.
- If neither the CLI nor MCP tools exist, surface the install path: `curl -fsSL https://cli.cori.do/install.sh | bash`. But if that install fails with a 403 / blocked network (typical in cloud sandboxes — `cli.cori.do` is rarely allowlisted), **don't retry it**; say so honestly, author and review the workflow folder anyway, and hand validation off to the user's own machine (Cori desktop app or terminal). An unvalidated-but-reviewed folder is still a deliverable; a retry loop is not.
- **Never simulate validation.** If `cori check` / `cori run` could not actually execute, say exactly that — do not reason about what they "would" report, and never present an expected result as an observed one. A field session that simulated instead of running shipped subtly wrong conclusions; "I could not validate this here" is the honest, useful output.

---

## What a workflow folder looks like

```
<workflow_name>/
├── manifest.md              YAML frontmatter + prose: what, why, parameters
├── deno.json                import map (@cori-do/sdk + zod) + `deno task test`
├── types.ts                 (optional) shared TypeScript types for step I/O
├── steps/
│   ├── 01_<name>.ts         one TS file per step, numeric prefix = execution order
│   ├── 02_<name>.ts
│   └── ...
└── tests/                   (optional) `deno test` files + captured fixtures
    ├── <step>.test.ts
    └── fixtures/*.json
```

**Test and tool with Deno, not Node.** Cori's runtime already *requires* Deno — `cori run` executes every `code` step inside a Deno sandbox, and the broker refuses to run without a Deno binary. So Deno is guaranteed present, and Node/npm is not otherwise needed. **Do not emit a `package.json` / `node_modules` / `vitest` harness** — it adds a second toolchain purely for tests and, worse, resolves imports differently from the runtime (Node walks `node_modules`; the Deno runtime uses a fixed import map). A `package.json`-based test can pass while the step fails at runtime.

Instead emit a workflow-root **`deno.json`** whose import map mirrors the runtime's, and write tests as `Deno.test(...)`. Because tests then run on the *same engine with the same resolution rules* as production, a passing test is a faithful proxy for a passing run — it even catches a bad bare import that the runtime would reject. See Step 5 for the `deno.json` template and the test command.

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

First recover the procedure's public input contract from the conversation. An
explicit `Parameters`, `Inputs`, or run-arguments list is authoritative: copy
those parameter names and types exactly. Do not add another parameter merely
because a fixed requirement could be made configurable. Output destinations,
tab or sheet names, recipients, statuses, titles, policy thresholds, and safety
behavior remain constants when the task specifies them as fixed values. A
default does not turn a fixed requirement into a caller input.

If the conversation has no explicit input contract, look at every concrete
value in the productive actions — spreadsheet IDs, message IDs, run tags,
environment names, paths, dates, thresholds, channel names. For each, ask:
*is the caller expected to choose this value on each run?*

- Changes → **parameter**: snake_case name and TS type. Omit the default for run-scoped or externally allocated values.
- Fixed property of the system → **constant**: leave inline in the step file.

When there is no explicit contract and the distinction is unclear, ask before
previewing instead of silently expanding the workflow's interface.

Treat opaque external identifiers and fixture selectors as ephemeral unless the user explicitly says they are stable: spreadsheet/document/message/event IDs, run tags, tag-scoped queries, generated filenames, timestamps such as `as_of`, and values returned by an API. Declare these as required parameters or derive them from an earlier step; **never copy their captured value into `manifest.md`, `types.ts`, helper modules, or runtime step files.** If an explicit contract omits a required ephemeral value that cannot be derived, ask the user to expand the contract; do not add it silently. When no explicit contract exists, genuine caller preferences such as a locale or `dry_run` choice may have defaults.

Before previewing, compare the manifest parameter names with any explicit input
contract. They must match exactly unless the user approved a contract change.

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
- **Google Workspace goes through the `gws` CLI — enforce this as a pre-write lint, not a preference.** Before writing any step file, scan your draft decomposition: **if a step calls an MCP tool against a Google Drive / Gmail / Sheets / Calendar / Docs server, rewrite it as a `gws` `cli` step first.** The pull of the source conversation is strong (it probably used the MCP tool) and `cori check` will happily pass the MCP version — which then fails at run time on the worker unless that server is declared in `~/.cori/mcp-servers.json`. The `gws` path avoids that whole class of failure, and it's usually *simpler*: the Sheets API returns value arrays directly, which in a real field session deleted an entire download-and-parse step. Subcommands mirror the Google APIs (e.g. `gws sheets spreadsheets values get …`), so translating is mechanical. Declare `tools_required: [gws]` as usual; see `references/example_workflow.md` for real `gws` steps.
- **Discover Cori-blessed capabilities before hand-rolling a step — the registry is dynamic, so query it, don't memorize it.** Before designing steps that touch documents, the web, or any external system, run `cori capability list --json` (or read `capability_registry` in the MCP `status` tool's output). Every entry carries a `use_for` line saying when to reach for it, install/auth state, and — when not ready — the one `remedy` command (e.g. `cori capability install anydoc`). Prefer a registry capability over hand-rolled code in an interpreter snippet (a Python document parser, a `curl` pipeline against a JavaScript-rendered site), and prefer proposing the remedy command over designing around a missing capability. Declare each one used in `tools_required` as usual. Capabilities added to the registry after this skill was written will appear there — trust the registry output over any capability list in prose, including this file's.
- **A `cli` command's first argv element must be the real, statically named executable.** It is the capability Cori discovers, validates, and spawns. Do not use generic dispatchers such as `env`, `sh`, `bash`, or `xargs` to launch a dynamic executable path. If a prior step creates a runtime-specific interpreter or executable, keep a stable declared tool as argv[0] and use a small argument-safe wrapper; see `references/activity_kinds.md`.
- **GWS is always invoked directly.** Every Google Workspace CLI activity must return an argv beginning with the literal `"gws"`. Never invoke `gws` through Deno, Node, Python, a shell, or another wrapper. Before previewing, verify every literal GWS method with `gws schema <service.resource.method> --resolve-refs` and inspect method help when flags are uncertain.
- **Gmail raw messages use real RFC line breaks.** When a GWS step creates a Gmail draft or message through `message.raw`, assemble headers and body with `"\r\n"` in TypeScript source and include `"\r\n\r\n"` between them. Never use `"\\r\\n"`: that emits literal backslash characters, folds the payload into a malformed header, and can fail with `Invalid To header`. Use the UTF-8-safe Base64url pattern and malformed-header guard in `references/activity_kinds.md`.
- **A `cli` `parse` callback never receives workflow state.** Its signature is
  `parse(stdout, { stderr, exitCode })`. Derive output only from CLI stdout or
  return a fixed acknowledgement. If later steps need a value already present
  in the command input, keep using its existing flat-state key; do not try to
  re-emit it from `parse`.
- **Syntax-check generated inline interpreter programs as their final assembled string.** In particular, never join multiline Python containing compound statements (`if`, `for`, `while`, `with`, `try`, `def`, `class`) with `"; "`; Python rejects compound statements after semicolons. Join those lines with `"\n"` and validate the resulting snippet before saving.
- **`llm` steps must declare a typed output schema.** Free-form text returns aren't a Cori step — they're a bug. If you used an LLM in the conversation to extract structured info, the step's output type *is* that structure, and the prompt enforces it.
- **Pick `llm` models from a provider the machine is actually signed into.** Before writing an `llm` step, run `cori status` (or the MCP `status` tool) and read the capabilities list: choose a model id from a family showing `authed: true`. Never default to a habitual model id — a step targeting an unauthenticated provider is the single most common reason `cori check` comes back not-ready, and swapping the `model` field yourself is a one-line fix, whereas asking the user to `cori login` a whole new provider is a much bigger ask. Record in `## Notes` which provider the step was validated against.
- **Capabilities are mandatory.** Any `cli` step that uses `gws` must declare `tools_required: [gws]`. Any `mcp_tool` step must declare its server in `mcp_servers`. The compiler enforces this — placement inference depends on it.

Order the steps. Number filenames `01_`, `02_`, `03_`, … so the `steps/` directory reads in execution order.

#### Execution dataflow contract

Cori passes one flat state object through the numbered steps. Design every
schema and output around these exact rules:

- State begins with the manifest parameters as top-level keys.
- After a successful step, an object output is shallow-merged into that same
  flat object. Outputs are not nested under the step name.
- A duplicate top-level output key overwrites the value produced earlier.
- Every required key in a step's `input` schema must exactly match a manifest
  parameter or a top-level key produced by an earlier step.
- Every value discovered at runtime — record IDs, resource contents, classifications,
  counts, summaries — must reach its consumer through this state. A read/query step
  whose output is ignored is not a valid substitute for hardcoded captured data.
- Preserve multiple records and side-effect IDs in arrays or under unique
  wrapper keys. Never emit repeated generic keys such as `message`, `id`, or
  `label_id` when later steps need more than one of those values.

Runtime Zod parsing is the enforcement boundary: declared input schemas parse
before a callback or external side effect, and declared output schemas parse
before a successful result returns. Object parsing applies Zod's unknown-key
policy, defaults, and transforms. Schemas remain optional only for compatibility
with older workflows; every newly captured step should declare both.

For support-inbox workflows, fetched messages and created label IDs must remain
uniquely addressable from later steps. Use a `messages` array, a
`label_ids_by_message` array/map, or explicit unique wrapper keys; do not let
successive fetch/create steps overwrite a shared `message` or `label_id` key.
Never copy captured message IDs, subjects, senders, timestamps, classifications,
summaries, or counts into runtime source. If v1's missing `for_each` requires a
fixed-cardinality layout, add a pure `code` step that validates the expected
cardinality and expands runtime-derived IDs into unique keys, then use explicit
GWS lanes. If the cardinality is not fixed by the task contract, stop and explain
that the workflow cannot yet be captured safely.

**Pre-write lint — run this checklist on every step of your draft decomposition, before Step 5. Answer each item explicitly; do not skip it because the decomposition "looks done":**

- [ ] **Google Workspace via MCP?** If the step calls an MCP tool against a Google Drive / Gmail / Sheets / Calendar / Docs server → rewrite it as a `gws` `cli` step now, using the template below. (`cori check` will warn on this too, but fix it before writing.)
- [ ] **Any other `mcp_tool` step?** It will only run on workers where that server is declared in `~/.cori/mcp-servers.json`. Confirm that's true, or say so at the review gate.
- [ ] **I/O hiding in a `code` step?** Network/filesystem/DB access belongs in a `cli` or `mcp_tool` step; `code` stays pure.
- [ ] **`cli` steps use the static template.** `command` is a function returning an **argv array** with a literal binary name first, plus `parse`. If you wrote `Deno.run`, `exec`, or any subprocess call inside a step body, the step is wrong — re-read `references/activity_kinds.md` and use the template. (A real field session produced exactly that bug.)

The minimal correct `gws` shape, for reference at the moment you need it:

```ts
export default step.cli({
  description: "Read source rows from Google Sheets",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, range }) => [
    "gws", "sheets", "spreadsheets", "values", "get",
    "--params", JSON.stringify({ spreadsheetId: spreadsheet_id, range }),
    "--format", "json",
  ],
  parse: (stdout) => ({ values: JSON.parse(stdout).values ?? [] }),
});
```

**When you rename or re-decompose a step during iteration, delete the old numbered file — superseding is not enough.** Two files sharing a number fail `cori check` with `duplicate step number NN`. In write-only environments (some sandboxed sessions can write and overwrite but not delete), you cannot remove the orphan yourself: tell the user explicitly which stale file to delete before re-checking, as a required manual step.

### Step 5: Write the files

Create the directory layout. Use the `@cori-do/sdk` templates from [`references/activity_kinds.md`](references/activity_kinds.md). Each step file:

- Imports the right primitive from `@cori-do/sdk` (`step.cli`, `step.code`, `step.mcp_tool`, `step.llm`)
- Declares typed `input` and `output` as zod schemas (in the file or imported from `types.ts`)
- Has a one-line `description` (becomes the activity name in the run trace)
- Default-exports the `step.<kind>({…})` call

Capture real I/O from the conversation as fixtures under `tests/fixtures/`. For `code` activities, generate a `Deno.test` unit test that pins the expected output. Users who want to verify before running can `deno task test` inside the workflow directory. This is the trust layer for whoever reviews the workflow later.

Captured records belong only in `tests/fixtures/`; runtime modules must consume
parameters and earlier outputs. Tests must exercise a step or shared pure helper
against a fixture. Do not generate tautologies such as comparing a literal array
with the same literal array.

**Write a `deno.json` at the workflow root** so the `@cori-do/sdk` and `zod` imports in every step and test resolve — the same way the runtime resolves them. The SDK and zod are published on the public npm registry; the import map points at them with `npm:` specifiers, exactly mirroring the runtime's own import map. No `npm install`, no `node_modules` — Deno fetches and caches on first run. Template:

```json
{
  "imports": {
    "@cori-do/sdk": "npm:@cori-do/sdk@^0.2.4",
    "zod": "npm:zod@^4.4.3"
  },
  "tasks": {
    "test": "deno test --no-check --allow-read --allow-env --allow-net=registry.npmjs.org,esm.sh,jsr.io tests/"
  }
}
```

Notes:

- **Mirror the runtime's resolution.** The runner runs `code` steps with an import map of exactly `@cori-do/sdk` + `zod` and network limited to `registry.npmjs.org,esm.sh,jsr.io`. The `test` task uses the same allow-net allowlist, so a `code` step that legitimately imports an `npm:`/`jsr:`/`esm.sh` package (see `references/activity_kinds.md`) resolves in tests just as it will at runtime — and a *bad* bare import fails the test with the same error the runtime would raise. That parity is the whole point of testing with Deno.
- **`--no-check`** skips type-checking at test time (the SDK types `run`'s return loosely, so a strict check trips on `result.foo`). The test still executes the real `run` logic. This matches how a JS test runner behaves.
- **Pin versions to current.** Check with `npm view @cori-do/sdk version`. `zod` must satisfy the SDK's peer range (`^4.x` at time of writing); a mismatched major (e.g. `zod@3`) is a silent break.
- **Test files** import the step with an explicit `.ts` extension (`import step from "../steps/03_x.ts"`) and import fixtures with `import data from "./fixtures/x.json" with { type: "json" }`. **Assert with a zero-dependency helper you emit at `tests/assert.ts`** — not `jsr:@std/assert` by default: `jsr.io` is blocked in many authoring sandboxes (`403 host_not_allowed`), and a test that dies on an *import* proves nothing about the step. Template:

  ```ts
  // tests/assert.ts — self-contained; no network needed to run tests.
  export function assertEquals(actual: unknown, expected: unknown, msg?: string) {
    const a = JSON.stringify(actual), e = JSON.stringify(expected);
    if (a !== e) throw new Error(msg ?? `assertEquals failed:\n  actual:   ${a}\n  expected: ${e}`);
  }
  export function assert(cond: unknown, msg = "assertion failed") {
    if (!cond) throw new Error(msg);
  }
  ```

  `jsr:@std/assert` remains a fine *upgrade* when the environment can reach `jsr.io` — treat it as optional, never as the thing `deno task test` depends on to even start. See `references/example_workflow.md` for a full test.
- **Deno is assumed present** (Cori can't run without it). If `deno --version` fails, surface `curl -fsSL https://deno.land/install.sh | sh` — but if Cori is installed at all, Deno already is.

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

`cori check` validates the manifest and static capability declarations, statically parses and type-checks every workflow module with Deno, then runs capability/preflight checks. It does **not** execute step callbacks, execute Zod schemas against runtime values, or perform cross-step dataflow analysis. Runtime schema enforcement fails safely at the first consuming activity. Surface any check errors back to the user in plain language — don't just dump raw `cori` output. If validation fails, offer to fix.

Before asking for approval, build a short inventory of captured run-scoped
literals (resource/record IDs, run tags, tag-scoped queries, timestamps) and
search for each across `manifest.md` and every non-test runtime source file.
Any match is a capture bug unless the user explicitly designated that value as
stable. Also compare the manifest parameter set with any explicit `Parameters`,
`Inputs`, or run-arguments list from the conversation; fixed task requirements
must not appear as extra parameters. Verify that every read/query output needed
by the procedure is consumed by a later step.

### Step 8: Confirm and suggest next action

After `cori check` is green:

> "Saved to `<chosen_path>/`. Try a dry run with `cori run <chosen_path> --dry-run`, or run it for real with `cori run <chosen_path>`."

Don't auto-run. Saving and running are separate decisions.

**If the user does run it and it succeeds, close the loop in `## Notes`:** record the validation date, the model/provider that actually ran, and any dead ends hit on the way there (e.g. "first tried `gpt-4o-mini`; openai isn't authed on this machine — validated with anthropic instead"). A workflow whose Notes say when and with what it last worked is trustworthy to the next person — or the next agent session — that picks it up.

Before reporting success, repeat the run-scoped-literal and dataflow audit. Inspect every generated `cli` step once more: argv[0] must be a string literal naming the actual executable, the set of those literal executables must exactly match `tools_required`, and no interpreter or dispatcher may hide a second declared command. For GWS, every argv[0] is `"gws"`. In Gmail draft/message steps, reject `"\\r\\n"` source joins and confirm the raw message builder uses real `"\r\n"` separators plus the guard from `references/activity_kinds.md`. Syntax-check any inline interpreter snippet after assembling it, because `cori check` parses the TypeScript module but cannot parse a Python or shell program stored inside a string. Also re-scan for `mcp_tool` steps targeting Google Workspace servers — if `cori check` printed a warning about one, treat it as a review-gate item, not as noise: propose the `gws` rewrite to the user before shipping.

---

## Proactive save offer (no command)

When the user has just completed a non-trivial task that looks repeatable, offer — once, briefly — to save it. Triggers:

- A multi-step task involving ≥2 tool calls that produced a clear result
- The user said something like "great, that worked", "perfect", "done"
- The task touches a recurring concern: a recurring report, a data pipeline, an integration, a content processing flow, an onboarding/offboarding action

Before offering, silently check for Cori: `cori --version`, and if that fails, look for Cori MCP tools (`check`/`run`/`status` under a `cori` server, however your client namespaces them).

- **If Cori is reachable (CLI or MCP):** offer with one line:
  > "Want me to save this as a Cori workflow so you can re-run it?"
- **If Cori is not reachable at all:** offer with the install path:
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

**`cori` not installed.** First check for Cori MCP tools (see "The Cori CLI in one screen" — they replace every CLI call in this skill). Otherwise install: `curl -fsSL https://cli.cori.do/install.sh | bash`. If the install is network-blocked (sandbox), author + review the folder anyway and hand validation to the user's machine — never retry a blocked install in a loop.

**`cori check` fails on a TS step file.** Read the error, locate the offending step, fix it (most often a type mismatch, missing `@cori-do/sdk` import, or wrong zod schema), re-run `cori check`.

**`cori check` says a CLI binary is missing from `tools_required`.** The compiler enforces the declaration. Add the binary to the manifest's `tools_required` list and re-check.

**`cori check` not ready, or an `llm` step fails at run time, on a provider/model problem.** Two failure modes look alike but need opposite fixes — the trace's `error` field distinguishes them (see `references/trace_interpretation.md`):

- **Auth/permission error** → the provider capability isn't signed in on that machine. Prefer switching the step's `model` to a family that `cori status` shows as `authed: true`; only suggest `cori login <provider>` if no authed family can do the job.
- **404 / "model not found"** → the provider is fine; the model id doesn't exist (plausible-looking ids, including dated snapshots, routinely don't). Pick a valid id from the *same* family. Do not respond to a 404 by switching providers or asking for a login.

After the fix, loop: edit → re-check → (if running) re-run, until green.

**`deno test` reports `Import "<pkg>" not a dependency and not in import map`.** A step or test imports a bare package name that isn't `@cori-do/sdk` or `zod`. This is the runtime telling you the step would *also* fail under `cori run` — Deno tests resolve exactly like the runtime. Fix the import: use `@cori-do/sdk`/`zod`, a no-import global, or (if a third-party library is genuinely needed) an explicit `npm:<pkg>@<ver>` / `jsr:` / `https://esm.sh/` specifier. Do not "fix" it by adding a `package.json` — that only hides the failure until runtime. If `@cori-do/sdk` itself isn't resolving, the workflow is missing its `deno.json` (Step 5) or has no network to `registry.npmjs.org`.

**`deno test` fails with a type error (`TS…`).** Run via `deno task test` (the template task passes `--no-check`). The SDK types a step's `run` return loosely, so strict type-checking trips on field access in assertions; `--no-check` runs the real logic without type-gating, matching how a JS test runner behaves.

**No `node_modules`, no `package.json`, nothing to gitignore.** Deno caches `npm:`/`jsr:` modules in its own global cache, not in the workflow folder. The folder stays clean — just `deno.json` plus the workflow files. `cori run` ignores the workflow's `deno.json` anyway (it uses the runtime's own import map); that file exists only for local `deno test` and editor IntelliSense.

**A step kind looks wrong on review.** Better to re-decompose than to ship a workflow with an `llm` step that should have been `code` (or vice versa). Fix at Step 7 (review) before disk write.

**The two Google Workspace shapes, side by side** (this specific mistake has now been made by more than one agent in the field — the wrong shape compiles and passes `check` on the authoring machine, then fails on any worker without the MCP server declared):

```ts
// ❌ ANTI-PATTERN — fails at run time unless every worker declares the
// server in ~/.cori/mcp-servers.json:
export default step.mcp_tool({
  server: "Google_Drive",
  tool: "read_file_content",
  /* … */
});

// ✅ PATTERN — runs anywhere `gws` is installed and signed in:
export default step.cli({
  command: ({ spreadsheet_id, range }) => [
    "gws", "sheets", "spreadsheets", "values", "get",
    "--params", JSON.stringify({ spreadsheetId: spreadsheet_id, range }),
    "--format", "json",
  ],
  parse: (stdout) => ({ values: JSON.parse(stdout).values ?? [] }),
  /* … */
});
// (Note `command` returns a static argv array — never spawn subprocesses
// inside a step body with Deno.run/exec; that's a different, equally
// real anti-pattern.)
```

---

## References — read these when relevant

- [`references/activity_kinds.md`](references/activity_kinds.md) — full TypeScript templates for each activity kind. Read before writing your first step file in a session.
- [`references/manifest_schema.md`](references/manifest_schema.md) — full YAML frontmatter spec, parameter types, validation rules. Read before writing your first manifest in a session.
- [`references/example_workflow.md`](references/example_workflow.md) — a complete, realistic worked example (translate product sheets with a GPSR check). Read for a concrete model of what good output looks like.
- [`references/trace_interpretation.md`](references/trace_interpretation.md) — how to read a persisted `RunTrace` (`~/.cori/runs/<key>/<utc>.json`): per-step status, attempts, duration, cost. Read when a run fails and you need to diagnose which step broke and why.

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

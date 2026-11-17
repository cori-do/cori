# Reading `cori check` and `cori run`

## `cori check` output

```
workflow      support_ticket_triage (sha256:7f3a…)        # content hash = the version identity of this workflow
context.ts    generated (7 steps, 1 nested)               # .cori/context.ts rewritten; `Ctx` now knows every step
deno check    ok                                           # every binding type-checks (skipped with --no-deno)
dataflow      load_ticket → classify ∥ lookup_customer → decide_priority → escalate_gate → notify → draft_reply
effects       helpdesk · getTicket:Read   tickets · getCustomer:Read   slack · POST /api/chat.postMessage:MayWrite
endpoint      sso · sync · allow {"roles":["support_agent","support_lead"]}
warnings      1
  - steps/load_ticket.ts:12: step `load_ticket` exposes `ticket.body`, tagged pii in stores/support_tickets/rules.yaml, with no `redact: [steps.load_ticket.ticket.body]` in workflow.yaml (or leave the column out of the role's readable list)
```

- `dataflow` is derived from the `ctx.steps.*` references. `∥` marks steps that run in parallel. If two steps you
  expected in sequence appear parallel, one does not read the other: add the data reference, or `after: ["x"]` when
  there is an ordering constraint without data.
- `effects` is what the change review shows. A new `MayWrite` line means the version needs an approver before promotion.
- `--json` prints the full object: `manifest` (effective, with defaults applied), `dag`, `effects`, `lineage`,
  `warnings`. Use it to confirm the effective `idempotency`, `ordering` and `parallel`.

`cori check .` compiles every workflow and adds the stores: role columns vs migrations, step SQL vs both, `pii` columns
exposed without `redact:`.

## Errors and their fixes

Layout and manifest:

| Message | Fix |
|---|---|
| `workflow.yaml: id \`x\` must equal the folder name \`y\`` | rename one of them |
| `workflow.yaml: \`steps:\` is empty; list the top-level steps in execution order` | list the step names |
| `workflow.yaml: step \`n\` is listed in \`steps:\` but steps/n.ts is missing` | write the file or drop the name |
| `steps/f.ts is not listed in workflow.yaml \`steps:\` (add \`n\` at its position in the list, or delete the file)` | list it at its execution position; a nested step must move into its parent's file instead |
| `step name \`n\` must equal the file name (rename the file to steps/n.ts or the step to \`stem\`)` | rename |
| `step name \`n\` must be snake_case` | rename |
| `duplicate step name \`n\`` | names are unique across the workflow, nested steps included |
| `step uses connector \`c\` which is not listed in manifest \`connectors\`` | add it to `connectors:` (and the app's `cori.toml` if it is a store or an override) |
| `step \`s\` calls tool \`t\`, which no role of stores/<store> exposes (tools: …)` | use a listed name (`cori store tools <store>` shows them per role), or grant the table in `stores/<store>/roles/<role>.yaml` |
| `connector \`c\` is declared but no step uses it` (warning) | remove it from `connectors:` |
| `cori.toml [app] description: one line …` / `at most 140 characters …` (warning) | the apps portal shows one short line under the app's name: shorten it |
| `cori.toml [auth] roles lists \`a\` before \`b\`, but in stores/<s> \`b\` may do everything \`a\` may, and more (…): … List roles strongest first` (warning) | swap them in `[auth] roles`: strongest first, as a person holding both acts as the first listed |
| `step \`s\`'s query is UPDATE: a store query only reads (one SELECT) …` | change rows with the store's tools (`cori store tools <store>`: create…, update…, delete…); a query only reads |
| `\`n\` is not a declared parameter` / `\`n\` is not a step of this workflow` (in `result`) | fix the `result` path |
| `result.fields \`p\`: \`f\` is not a field of step \`s\`'s output (fields: …)` | use a field of the step's `output` schema |
| `result.headline \`p\`: \`f\` follows an array; templates cannot index arrays or read .length (compute it in a code step)` | add a `code` step that returns the count or the element |
| `not a folder; pass the workflow folder <app>/workflows/<id>/` | `cori check workflows/<id>`, not `steps/` or the file |

Step files:

| Message | Fix |
|---|---|
| `expected \`export default step.<kind>({ ... })\`` / `missing \`import { step } from "@cori-do/sdk"\`` | one default export per file, built by `step.<kind>` |
| `bindings must be inline arrow functions` / `binding has no body` | `input: (ctx: Ctx<"n">) => ({ … })`; no helper functions passed by name |
| `the whole context escapes the binding (\`ctx\`); reference fields explicitly` / `\`steps\` as a whole escapes the binding` | return an object of explicit `ctx.params.x` / `ctx.steps.a.b` reads |
| `dynamic access on the context (\`…\`) cannot be proven; use static member access` | no `ctx.steps[name]`, no destructuring with computed keys |
| `unknown context member \`m\`` | only `params`, `user`, `run`, `steps`, `item` |
| `non-deterministic value in a binding (\`new Date()\`): bindings are replayed, use ctx.run.started_at / ctx.run.id` (warning) | bindings (`input`, `if`, `on`, `over`, `until`) run in replayed workflow code: `new Date()`, `Date.now()`, `Math.random()`, `crypto.randomUUID()`, `performance.now()` differ on replay; read `ctx.run.started_at` / `ctx.run.id`, or produce the value in a `code` step |
| `step \`a\` reads \`steps.b\` which does not exist` | typo, or `b` is nested in a `map` (not addressable) |
| `step \`a\` reads \`steps.b\` which runs after it (only earlier steps may be read)` | move `b` earlier in `steps:` or invert the dependency |
| `\`after: [x]\` must name an earlier step` | same |
| `llm step \`n\` must declare an \`output\` schema` | add `output: z.object({ … })` |
| `\`activity\` steps need \`activity\` and \`version\` literals` / `\`cli\` steps must name a \`connector\`` / `\`mcp\` steps must name a \`connector\`` | add the literal fields; no variables |
| `\`node:\` imports are not allowed in step files` / `a step imports a file outside the app folder` | move shared code to `lib/`; steps are pure and sandboxed |
| `[fatal] \`then\` must be an inline \`step.<kind>({...})\` or \`goto("…")\`` | branch/switch/try bodies are inline declarations, never references |
| `steps/x.ts:12: TS2339 Property 'plan' does not exist on type …` | a binding reads a field the predecessor's `output` does not declare, or the wrong step; re-run `check` after changing an `output`, since `Ctx` is regenerated |
| `could not run \`deno check\`` | Deno 2 missing on the machine: install it, or `--no-deno` for a quick pass (never before deploy) |

Registry and connectors:

| Message | Fix |
|---|---|
| a `mcp` step's `input` fails `deno check` against `ConnectorTools` | the tool's input schema changed or the argument is wrong; `cori connectors find <tool> --json` shows the schema, `cori scaffold … --force` rewrites the step |
| `no worker polls <task_queue>` (warning) | the activity connector's worker is not running; fine locally with the fixture, blocking in production |
| `not signed in to <url>` / `session expired` / 401 | human: `cori login <url>`; agent: call the MCP `login` tool again and rewrite `.cori/session` |
| `<url> refused the connector registry, so the tool inputs of its connectors are not typed …: 403 Forbidden` (warning) | the token is a member's who builds no app: sign in as the app's owner or one of its builders, or `check` types no tool input and `run` finds only the connectors `cori.toml` declares |

## `cori run` output

```
$ cori run workflows/support_ticket_triage ticket_id=T-1042 --decide 'approve_escalation={"escalated":true}'
store: using the embedded Postgres of `cori up` (fixtures ignored; --no-store to use them)
user: dev:support_agent (the endpoint's first allow role; --user <sub>:<roles> to change)
run support_ticket_triage-8c2f… · support_ticket_triage · succeeded
  ● load_ticket              mcp           41 ms  
  ● classify                 llm           12 ms  
  ● lookup_customer          store          9 ms  
  ● decide_priority          code           1 ms  
  ● escalate_gate            branch         0 ms  
  ● approve_escalation       human          0 ms  
  ● notify                   http           6 ms  
  ● draft_reply              llm            8 ms  
result: Ticket T-1042 → billing (P2)
  Category     "billing"
  Priority     "P2"
```

- One row per step: `●` ok, `○` not_taken (a branch arm not entered), `✕` failed (the error replaces the notes).
  Notes show what the step did: the SQL of a query, `(mock: fixture rows, the SQL is not executed)` when a store
  fixture answered, the tool or request of a connector.
- The two `store:` / `user:` lines go to stderr: they tell you which database and which identity the run used.
- A `human` step without `--decide` blocks the command: nothing prints after the `store:`/`user:` lines until the
  step's `timeout`. For local runs always pre-record decisions with `--decide '<step>=<json>'` (repeatable, one per
  gate).
- `--json` prints `{ run_id, status, error, steps: { <name>: <output> }, trace: [{ name, kind, status, attempts, ms,
  cost_eur, error, notes }], result: { headline, fields: [{ label, value }], sections, values } }`. `steps` holds the
  outputs by name, which is what a downstream step reads; `values` keys the result fields by label in snake_case.
- `--user alice:support_lead` runs as another identity (roles decide store tools and connector exchange); the default
  is `dev` holding the endpoint's first `allow` role. A step refused by a store role fails with the rule that refused it.
- A failed run prints `error: …` on stderr and exits 1 (with `--json`: the document, then exit 1).

## Change review (at `cori deploy`)

```
$ cori deploy .
app support-desk · v3 deployed · version 3b9d12ab34cd · commit 0f1e2d3c4b5a
  sandbox  support_tickets      created · 1 migration(s) · 1 seed file(s)
  preview  http://127.0.0.1:8080/apps/support-desk/@v3/  (its workflows run on the sandbox; nothing reaches the live stores)
  console  http://127.0.0.1:8080/#/versions/support-desk  (versions, change review, approve, promote)
  workflow support_ticket_triage      endpoint sync  schedule -
  workflow refund_request             endpoint async  schedule -
  change review: new/changed writes 1 · added 1 · removed 0 · new writes: an approver (it_admin, org_admin, approver) approves before it is promoted
    write  refund_request.record_request: tickets · createRefundRequest MayWrite
  next     cori promote support-desk v3
           (a human; going live applies this release's pending migrations to the live stores; preview data stays behind)
           without an approver role the request waits in the console inbox; an approver accepts it there, or runs `cori promote support-desk v3`
```

On a server with a fronts domain the preview is the root of the release's own host (`https://support-desk--v3.apps.example.com/`,
on Cori Cloud `https://support-desk--v3--<org>.cori.run/`).
`(dirty)` after the commit means uncommitted changes went into the bundle (`--require-clean` refuses that). `--json`
prints the server's response (`hash`, `short`, `status`, `review.delta`, `sandbox`, `preview_url`, `promote`).
Relay the version, the preview url, every `write` line and the `next` command to the user. The agent session cannot
run `promote` or `approve`.

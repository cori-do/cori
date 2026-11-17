# Step kinds

Every step is an inert declaration: `export default step.<kind>({ … })`. The compiler reads the file statically,
generates `.cori/context.ts` and proves every binding with `deno check`; the runner evaluates `input`, `run`,
`prompt`, `request`, `command`, `parse` and the control-flow selectors in a sandbox.

## The file

```ts
import { step } from "@cori-do/sdk";            // plus `goto` for branch/switch targets
import type { Ctx } from "../.cori/context";    // generated per workflow; Ctx<"<name>"> only exposes predecessors
import { z } from "zod";
import { Ticket } from "../types";              // the workflow's shared schemas (optional)

export default step.mcp({
  name: "load_ticket",                          // = the file stem, snake_case
  description: "Load the ticket from the helpdesk",
  connector: "helpdesk",
  tool: "getTicket",
  input: (ctx: Ctx<"load_ticket">) => ({ id: ctx.params.ticket_id }),
  output: z.object({ ticket: Ticket }),
  parse: (result) => ({ ticket: Ticket.parse(result) }),
});
```

Fields every kind shares (`BaseOpts`): `name`, `description`, `input(ctx)`, `output` (zod), optional `retries:
{ max }` (1 to 10 attempts; waits double from 1 s to 60 s, and `backoff` changes nothing), `timeout_ms`, `after: ["<earlier step>"]` (an ordering edge without data).

`description` is the step's title wherever a run is inspected: the console lists a run's steps by it, for the app's users
as for its builders, with what each step received and gave back. Write what the step does for the people of the app,
in their words, as a short phrase: "Record the request as pending", "Ask a manager when the rule does not approve", not
"createRequest on requests" or "branch on needs_approval".

## The context (`Ctx<"name">`)

| Member | Type | Notes |
|---|---|---|
| `ctx.params.<name>` | from `workflow.yaml` `parameters` | optional parameters are `T \| undefined` |
| `ctx.user` | `{ sub, roles, email?, …principal_claims }` | the run principal (endpoint caller, or the schedule's service account) |
| `ctx.run` | `{ id, workflow, version, attempt, started_at, last_error? }` | `last_error: { message, step, type }` inside `try … on_error` |
| `ctx.steps.<step>.<field>` | that step's `output` | only steps that run **before** this one; a step nested in `branch`/`switch`/`try` is `T \| undefined` |
| `ctx.item` | element type of the enclosing `map`'s `over` | only inside `each` |

Binding rules the compiler enforces: bindings are inline arrow functions; only static member chains on `ctx`
(`ctx.steps.a.b.c`); the whole `ctx`, the whole `ctx.steps`, or dynamic access (`ctx.steps[name]`) is an error; a
step may not read a step that runs after it. The references are the dataflow: `cori check` derives the DAG, runs
independent steps in parallel, keeps writes in `steps:` order, and traces field-level lineage through pure steps.

`ctx.steps.x` may be **undefined** when `x` sits in a `branch`, `switch` or `try` body: TypeScript forces you to
check (`decision?.escalated`). Steps in a `map`'s `each` are not addressable from outside; the map's output is.

## Kinds

| Kind | Purpose | Effect (derived) | Attempts |
|---|---|---|---|
| `code` | pure TypeScript over typed inputs: rules, decisions, shaping | None (pure) | 3 |
| `llm` | one model call, fixed schema, cost tier | Prompt (its inputs reach a model: prompt lineage) | 3 |
| `mcp` | one tool of a declared MCP connector | Read when the tool's name starts with a read verb (`get`, `list`, `search`, `find`, `query`, …), else MayWrite | 3 for reads, else 1 |
| `store` | a Cori Store tool or a guarded SQL query | Read for a read-verb tool (`getX`, `listX`) and a SELECT query, else MayWrite; a `requires_approval` column parks the change for an approver | reads 3, writes 1 |
| `http` | an HTTP API behind a declared connector | GET/HEAD ⇒ Read, else MayWrite | GET/HEAD or `idempotent: true` 3, else 1 |
| `activity` | a native Temporal activity (Go, C#, Java…) on a team's own worker | MayWrite | the declaration's `retry.max_attempts` (default 3); its `timeouts`, `retry.non_retryable` and heartbeat apply too; the step's `retries` / `timeout_ms` win |
| `cli` | a command on the sandbox worker (allow-listed binary) | MayWrite | 1 |
| `human` | approval or input routed to a role; pauses the run | None (the run waits for the decision) | n/a |
| `workflow` | a sibling workflow as a child | MayWrite (the child's own review lists its steps' effects) | not started again; the child's steps retry as their kinds do |

### `code`
```ts
export default step.code({
  name: "decide_priority",
  description: "Decide priority from category and plan",
  input: (ctx: Ctx<"decide_priority">) => ({
    category: ctx.steps.classify.category,
    confidence: ctx.steps.classify.confidence,
    plan: ctx.steps.lookup_customer.plan,
  }),
  output: z.object({ priority: z.enum(["P1", "P2", "P3"]), escalate: z.boolean() }),
  run: ({ category, confidence, plan }) => {
    if (category === "bug" && plan === "enterprise") return { priority: "P1", escalate: true };
    if (category === "bug" || (category === "billing" && plan !== "free")) return { priority: "P2", escalate: false };
    if (confidence < 0.5) return { priority: "P2", escalate: false };
    return { priority: "P3", escalate: false };
  },
});
```
`run(input)` may be async but must be pure (no I/O, no `node:` imports, no globals with state). Literal types are
preserved, so `z.enum` outputs type-check without casts. This is where every decision lives.

### `llm`
```ts
export default step.llm({
  name: "classify",
  description: "Classify the ticket",
  tier: "small",                                        // small | medium | large — bound to models in cori.toml [llm].tiers
  input: (ctx: Ctx<"classify">) => ({ subject: ctx.steps.load_ticket.ticket.subject, body: ctx.steps.load_ticket.ticket.body }),
  output: z.object({ category: z.enum(["billing", "bug", "feature", "other"]), confidence: z.number().min(0).max(1) }),
  prompt: ({ subject, body }) => `Classify this support ticket into exactly one category: billing, bug, feature, other.
Return JSON matching the schema with a confidence between 0 and 1.

Subject: ${subject}
Body:
${body}`.trim(),
  examples: [{ input: { subject: "Charged twice", body: "…" }, output: { category: "billing", confidence: 0.9 } }],   // optional few-shot
  fallback: { default: { category: "other", confidence: 0 } },   // "fail" | "human" | { default }
  // batch: { size: 20, by: "subject" }                           // optional batching over a list field
});
```
The answer is parsed by `output`; a mismatch retries, then applies `fallback`. Under `cori run`,
`fixtures/llm.json["classify"]` answers. Keep the prompt free of decisions: classify, extract, summarise, draft;
decide in the next `code` step. Small tier, high volume, fixed schema is what the distillation loop targets.

Files the model reads (a `file` or `files` parameter, [layout.md](layout.md)): `files` is a function of the step's
input, like `prompt`, and gives the files the call sends besides the prompt.
```ts
export default step.llm({
  name: "extract",
  description: "Read the invoice's fields",
  tier: "medium",
  input: (ctx: Ctx<"extract">) => ({ invoice: ctx.params.invoice }),          // a CoriFile: { id, name, size, content_type, sha256 }
  files: ({ invoice }) => [invoice],
  output: z.object({ supplier: z.string(), total_eur: z.number(), due: z.string() }),
  prompt: ({ invoice }) => `Read the invoice ${invoice.name}. Return its supplier, its total in euros and its due date (YYYY-MM-DD).`,
});
```
PDFs reach the model as documents, PNG, JPEG, GIF and WebP as images, text files (`text/*`, JSON, XML, CSV) as text;
any other type fails the step before the call (`accept` the types the model reads). One call sends at most 20 MiB of
files: for many files, a `map` over them with a child `workflow` per file (one call each). A `code` step reads a file's
reference only (`content_type`, `size`, `name`), never its bytes; check what the model read in the next `code` step.
Only `llm` steps take `files`. The file's bytes stay out of the run's record and its captured LLM calls: they hold the
reference.

### `mcp`
```ts
export default step.mcp({
  name: "confirm_on_ticket",
  description: "Reply on the ticket",
  connector: "helpdesk",
  tool: "addReply",
  input: (ctx: Ctx<"confirm_on_ticket">) => ({ id: ctx.params.ticket_id, internal: true, text: `Refund issued.` }),
  output: z.object({ reply_id: z.string() }),
  // parse: (result, input) => ({ … })   // optional; default: the tool's structuredContent validated by `output`
});
```
`input` is typed against the tool's input schema snapshotted from the registry (`ConnectorTools` declaration merging
in `.cori/context.ts`): a wrong argument fails `check`. `cori scaffold <connector> <tool> --workflow <id>` writes this
file for you with the schema already turned into zod. Authentication is the connector's: `auth = principal`
connectors receive a token exchanged for the run user, `auth = service` connectors their own credential.

### `store`
Tool form (generated by the Store from the role's rules: `get<Table>`, `list<Table>`, `create<Table>`, `update<Table>`…):
```ts
export default step.store({
  name: "record_request",
  description: "Record the refund request",
  connector: "tickets",                                 // omitted ⇒ the app's default store
  tool: "createRefundRequest",
  input: (ctx: Ctx<"record_request">) => ({ ticket_id: ctx.params.ticket_id, invoice_id: ctx.params.invoice_id, amount_eur: ctx.params.amount_eur, requested_by: ctx.user.email, status: "pending" }),
  output: z.object({ id: z.string() }),
});
```
Query form (guarded SELECT: readable columns only, row and time caps from the role; anything else rejected at deploy):
```ts
export default step.store({
  name: "stale_tickets",
  description: "Tickets open for more than 5 days in one department",
  connector: "tickets",
  input: (ctx: Ctx<"stale_tickets">) => ({ department: ctx.params.department }),
  query: ({ department }) => ({                          // a function of the input binds the positional parameters
    text: `select ticket_number, subject, department, created_at
           from tickets where status in ('open','in_progress') and created_at < now() - interval '5 days' and department = $1
           order by created_at`,
    params: [department],
  }),
  output: z.object({ rows: z.array(z.object({ ticket_number: z.string(), subject: z.string(), department: z.string(), created_at: z.string() })) }),
  // parse: (rows, input) => ({ rows })                 // optional; default wraps the rows as { rows }
});
```
A query without parameters may be the static object `query: { text: "…", params: [] }`. Do not put input field
names or `"$1"` strings in a static `params` array: the array is sent to the store as it is, unbound. See
[stores.md](stores.md) for rules, roles, migrations and fixtures.

### `http`
```ts
export default step.http({
  name: "notify",
  description: "Notify #support-escalations",
  connector: "slack",
  input: (ctx: Ctx<"notify">) => ({ ticket_id: ctx.params.ticket_id, decision: ctx.steps.approve_escalation }),   // T | undefined: nested in a branch
  output: z.object({ notified: z.boolean() }),
  request: ({ ticket_id, decision }) => ({
    method: "POST",                                     // GET | HEAD | POST | PUT | PATCH | DELETE
    path: "/api/chat.postMessage",                      // appended to the connector's base_url
    body: { channel: "#support-escalations", text: decision?.escalated ? `P1 ticket ${ticket_id} escalated.` : `Ticket ${ticket_id} not escalated.` },
    // query: { … }, headers: { … }
  }),
  parse: (res) => ({ notified: res.json.ok === true }), // res: { status, headers, json, text }
  // idempotent: true                                   // lets a non-GET retry 3 times
});
```
The credential (`auth_env`, or the broker's secret) is injected by the worker; never put it in the step. In a hosted
run it is the org registry's connector's, and a step's `path` must start with `/`: it stays on the connector's base_url.

### `activity`
```ts
export default step.activity({
  name: "issue_refund",
  description: "Issue the refund in billing",
  connector: "billing",
  activity: "IssueRefund",                              // literal name and version, frozen at compile time
  version: 2,
  heartbeat: true,
  input: (ctx: Ctx<"issue_refund">) => ({ invoice_id: ctx.params.invoice_id, amount_eur: ctx.params.amount_eur, reason: `ticket ${ctx.params.ticket_id}` }),
  output: z.object({ refund_id: z.string() }),
});
```
The team that owns the worker declares the activity in `activities.yaml` (input/output schemas, effects, retries);
`cori check` warns when no worker polls the connector's task queue.

### `cli`
```ts
export default step.cli({
  name: "append_sheet",
  description: "Append today's counts to the digest sheet",
  connector: "gws",
  input: (ctx: Ctx<"append_sheet">) => ({ groups: ctx.steps.by_department.groups, date: ctx.run.started_at.slice(0, 10) }),
  output: z.object({ appended: z.number() }),
  command: ({ groups, date }) => ["gws", "sheets", "append", "--spreadsheet", "1AbC…", "--range", "Digest!A:C", "--values", JSON.stringify(groups.map(g => [date, g.department, g.tickets.length]))],
  parse: (stdout) => ({ appended: JSON.parse(stdout).updates.updatedRows }),   // parse(stdout, { stderr, exitCode }, input)
});
```
`argv[0]` is frozen at compile time and must be the connector's allow-listed binary. Runs in a sandbox (or on a named
pool), never next to the platform's credentials.

### `human`
```ts
export default step.human({
  name: "approve_escalation",
  description: "Approve escalation to P1",
  kind: "approval",                                     // approval | input
  to: { role: "support_lead" },                         // or { user: "initiator" }
  input: (ctx: Ctx<"approve_escalation">) => ({ ticket_id: ctx.params.ticket_id, subject: ctx.steps.load_ticket.ticket.subject }),
  output: z.object({ escalated: z.boolean(), note: z.string().optional() }),
  timeout: "4h",                                        // "30m", "4h", "3d"
  on_timeout: { default: { escalated: false, note: "timeout" } },   // or "fail"
});
```
The run pauses; the item appears in the approvals inbox of the role (the app's `useApprovals`, the console, Slack or
email when configured); the decision is validated by `output` and audited with the decider. Locally: `cori run …
--decide approve_escalation='{"escalated":true}'`. Usually nested in a `branch` so it only runs when needed.

### `workflow`
```ts
export default step.workflow({
  name: "triage_each",
  description: "Run support_ticket_triage for one ticket",
  id: "support_ticket_triage",                          // a sibling under workflows/
  input: (ctx: Ctx<"triage_each">) => ({ ticket_id: ctx.item }),
  output: z.object({ category: z.string(), priority: z.string(), escalated: z.unknown().optional() }),   // the child's result values (its `result.fields`, keyed by label in snake_case)
});
```
A real Temporal child: its effects, gates and hash are the child's. The child needs no endpoint to be called.

## Builtins (control flow)

Builtins are steps too (they take a `name` and a `description`, live in their own file, and are listed in `steps:`),
but their bodies are inline step declarations or `goto("<step>")`.

| Builtin | Shape | Output |
|---|---|---|
| `branch` | `{ if: (ctx) => boolean, then: <step \| goto>, else?: <step \| goto> }` | nothing; nested steps keep their name in `ctx.steps`, typed `T \| undefined` |
| `switch` | `{ on: (ctx) => "<label>", cases: { label: <step \| goto> }, default?: <step \| goto> }` | same as branch |
| `map` | `{ over: (ctx) => T[], each: <step>, max_concurrency?, on_item_error?: "fail_fast" \| "collect" }` | `{ items: O[] }`, in order; with `collect`, failed items are `null` |
| `loop` | `{ body: <step>, until: (ctx) => boolean, max_iterations? }` | the body's last output |
| `try` | `{ body: <step>, on_error: <step \| goto> }` | the body's output, `undefined` when `on_error` ran; `ctx.run.last_error` inside `on_error` |
| `wait` | `{ for: { timeout_ms } }` (at most 30 days) | nothing |
| `goto("<step>")` | a jump target for `then`/`else`/`cases`/`on_error` | — |

```ts
// branch → human, else skip ahead
export default step.branch({
  name: "escalate_gate",
  description: "Ask a support lead before escalating",
  if: (ctx: Ctx<"escalate_gate">) => ctx.steps.decide_priority.escalate,
  then: step.human({ name: "approve_escalation", /* … */ }),
  else: goto("draft_reply"),
});

// map → child workflow, 4 at a time, keep going on item failure
export default step.map({
  name: "triage",
  description: "Triage every ticket",
  over: (ctx: Ctx<"triage">) => ctx.params.ticket_ids,
  max_concurrency: 4,
  on_item_error: "collect",
  each: step.workflow({ name: "triage_each", /* … input: (ctx) => ({ ticket_id: ctx.item }) */ }),
});

// try → activity, alert on failure
export default step.try({
  name: "refund_try",
  description: "Issue the refund, alert on failure",
  body: step.activity({ name: "issue_refund", /* … */ }),
  on_error: step.http({
    name: "alert_billing_failure",
    /* … */
    input: (ctx: Ctx<"alert_billing_failure">) => ({ error: ctx.run.last_error?.message ?? "unknown" }),
  }),
});
```

Throwing inside an `input` binding (`throw new Error("refund not approved")`) fails that step; inside a `try` body the
failure is caught and `on_error` runs. Downstream steps read the body's output as `T | undefined`.

## Effects, retries, placement

- Effects are **derived**, never declared: from the kind, the tool annotations, the store role, the HTTP method, the
  activity declaration. `cori check` prints them per connector (`helpdesk · getTicket:Read`, `slack · POST:MayWrite`).
  The change review at deploy lists new writes; a version that adds writes needs an approver before promotion.
- Retries follow the table above; `retries: { max }` overrides per step; `timeout_ms` bounds one attempt.
- Placement: `code`, `llm`, `mcp` (HTTP), `store`, `http` run on the shared worker; stdio MCP and `cli` on the sandbox
  pool or a named pool (`worker = "…"` on the connector); `activity` on the connector's task queue.
- Lineage: `check` traces which fields reach which `llm` prompt and which write; `redact:` cuts a path out of stored
  outputs and captured LLM calls.

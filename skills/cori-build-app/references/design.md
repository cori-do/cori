# From a request to a workflow

## 1. Name the trigger and the caller
- Someone clicks in the app → `endpoint`. Short and interactive (a lookup, a triage) → `mode: sync`; anything with a
  human gate, an external write or more than a few seconds → `mode: async` and the front polls with `useRun`.
- Runs on its own → `schedule` with `schedule_as` (a service account whose roles decide what stores and connectors allow).
- Called from another workflow → no trigger needed; the parent uses `step.workflow`.
- `allow.roles`: the app roles that may start it. The caller's roles are what stores and `auth = principal`
  connectors see; a step a role may not perform fails at run time and shows in `cori run --user`.
- Rank the app's roles before writing them down: `[auth] roles` lists them **strongest first**, because a person
  holding several acts in the stores as the first listed. Make each role able to do everything the roles after it may
  (a manager also files what an employee files), so a person never loses an ability by holding one more role.

## 2. Parameters
Only what the caller knows at click time. Everything else is looked up in a step. Optional inputs get `required:
false`; enumerations get `type: enum` with `values`; ids are `string`. The description is the form label.

## 3. One step per fact or effect
Write the list of top-level steps in execution order, then pick a kind for each:

| Need | Kind |
|---|---|
| read or write a company system through its MCP server | `mcp` (scaffold it) |
| read or write the app's own data | `store` (tool for one row by key, `query` for a filtered set) |
| an HTTP API the org declared | `http` |
| a rule, a threshold, a mapping, a merge, a count | `code` |
| classify, extract, summarise, draft free text | `llm` with a fixed zod `output` and the smallest tier that works |
| a person must decide or provide something | `human` inside a `branch` so it only runs when needed |
| the same thing for each element | `map` (child workflow when each element is a whole workflow) |
| an action that may fail and needs a fallback | `try` |
| existing Go/C#/Java code on a team's worker | `activity` |
| a CLI on the sandbox | `cli` |

Rules of thumb:
- **Decisions in `code`, never in prompts.** An `llm` step labels or drafts; the next `code` step decides from the
  label, the confidence and typed data. The decision is then auditable, testable and free.
- **One effect per step.** A step that writes twice cannot be retried safely. Split writes; let `steps:` order them.
- **Read early, write late.** Reads have no order constraint and run in parallel; writes follow `steps:`. Put every
  read before the first write so a failure leaves nothing half-done.
- **Gate before the write it protects**, and give the gate a `timeout` and an `on_timeout` default that is the safe
  choice (usually "not approved").
- **Idempotency.** Writes started from a form: `idempotency: parameters` or `header` with one key per form
  instance. Reads, and writes a user repeats on purpose (a status toggled back and forth): `none`. Unset, reads get
  `none` and writes `parameters`; set it explicitly on every write. A store `create` on an existing key fails: `try` it when a replay is possible.
- **Parallelism is free.** Two steps that only depend on earlier steps run together; do not merge them into one step
  to "save a call".
- **Child workflows** for reuse and fan-out; a child's gates and effects are its own and show in the parent's review.

## 4. Data shapes
- Put schemas used by several steps in `types.ts`; keep step-local ones inline.
- Narrow `output` schemas: `z.enum`, `z.number().min().max()`, `.max(2000)` on drafts. Downstream types and the LLM's
  contract both come from them.
- A `map` yields `{ items }`; a `code` step after it computes counts and groups for `result`.
- `result.headline` reads dotted paths only: precompute anything else in a `code` step named for the reader
  (`outcome`, `summary`).

## 5. Privacy and effects
- Tag `pii` columns in `rules.yaml`; keep them out of `readable` for roles that do not need them; `redact:` the
  fields a step must carry (a ticket body that feeds an `llm` step, for instance).
- Check the `effects` line of `cori check`: every `MayWrite` is a line in the change review and a reason for an
  approver. If a write surprises you, the tool annotation or the store role says so.
- Budget: `budget: { llm_eur_per_run, max_duration }` when the workflow calls a model or may wait on a human.

## 6. Fixtures and verification
- Write the fixture with the step: the tool result you expect, the rows a query returns, the JSON an `llm` step should
  produce. The first `cori run` then exercises the real dataflow.
- Cover both arms of every `branch` (`--decide`, different parameters) and one failure of every `try`.
- Store steps: one run against the real store (`cori up`) before deploying, because fixtures return rows without
  running SQL.
- Put the goal, the dataflow sketch and the verification checklist in the workflow's `README.md`; that is what the
  next person (or agent) reads.

## 7. Hand-off
`cori deploy .` gives a preview url on the version's own sandbox stores. Tell the user what to try there, what the
change review lists, and the `cori promote <app> <hash>` command. Promotion, approval and rollback are theirs.

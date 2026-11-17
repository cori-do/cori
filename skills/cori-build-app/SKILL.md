---
name: cori-build-app
description: "Author a Cori app end to end — create or extend the app folder, write workflows (workflow.yaml with an ordered steps list plus one typed TypeScript step file per step), bind org connectors and Cori Stores, get through `cori check`, run locally with fixtures, deploy a version and hand it to a human to promote. Also the first-time path: install the cli, `cori up`, the first app from the template. Use whenever the user wants to try Cori or asks to build, change, debug or ship a Cori workflow, store, front or app, or mentions cori.toml, workflow.yaml, `cori up`, `cori check`, `cori run`, `cori deploy`."
---

# Building an app on Cori

Cori gives an internal app a backend of durable, typed, permissioned workflows on Temporal, governed data stores and
hosting for its front. **The repository is the app**: `cori.toml` at the root, one folder per workflow under
`workflows/`, stores under `stores/`, dev fixtures under `fixtures/`, the front in `dist/`. You author with your own
file tools and the `cori` cli in that folder. Nothing you write travels through an MCP tool; the Cori MCP server
answers what the folder cannot know (which app, a session, the org's connectors). **When it is connected, it is the
server you work against** (section 0).

The thesis: **you decide at design time, the worker executes deterministically at run time.** Rules and decisions go
in `code` steps; `llm` steps have a fixed schema; effects, parallelism and lineage are derived by `cori check` from
the references your steps make, never declared.

Read the app's `AGENTS.md` first when one exists: it is the canonical conventions text (`cori check --conventions`
prints the same, `--json` for a machine-readable object). The references below go deeper than it does.

## References
- [references/layout.md](references/layout.md) — the folder, `cori.toml`, `workflow.yaml` field by field, fixtures, sessions, git hygiene
- [references/steps.md](references/steps.md) — every step kind with its exact options and a complete example, builtins, the `Ctx` rules, effects and retries
- [references/stores.md](references/stores.md) — Cori Store: migrations vs seed, rules and roles, store steps (tool and guarded query), fixtures, PII and redaction
- [references/cli.md](references/cli.md) — the authoring commands and their flags, what each one writes and prints
- [references/check.md](references/check.md) — reading `cori check` and `cori run` output; every error message and its fix
- [references/front.md](references/front.md) — the front: the Vite + React + Tailwind recipe, what a finished front has, `__cori/config.json`, the generated client, the run API
- [references/mcp.md](references/mcp.md) — the remote Cori MCP: apps, `login` and the session file, discovery tools, hosts without a shell
- [references/design.md](references/design.md) — turning a request into a workflow: which kind for which job, gates, idempotency, PII, parallelism

## Setup (once per machine)

```bash
npm install -g @cori-do/cli                   # macOS, Linux (Windows: WSL); `npx @cori-do/cli …` where nothing can be installed; from the repository: cargo install --path ./crates/cori-cli
cori up                                       # everything local (needs Deno 2, the Temporal cli, Postgres): server :8080, worker, Temporal dev server, embedded Postgres for stores, dev auth, connector mocks, console at /
cori login http://127.0.0.1:8080             # your dev login (dev:<you>:it_admin), the default outside app folders; `cori login` alone signs in to Cori Cloud, `cori login <url>` to another server
cori doctor                                   # deno, temporal, ports, server, database, store, auth (issuer, exchanges), secrets
claude mcp add --transport http cori http://127.0.0.1:8080/mcp   # optional: apps, sessions and connectors for the agent (references/mcp.md)
```

Dev identity under `cori up` is the bearer `dev:<sub>:<role,role>`; the console and the hosted front's dev widget take
the same token. A first app in one minute, on the org's server (section 0) or `cori up`: `cori init my-app --template
team-requests` (its front is already built: no npm needed), `cd my-app && cori deploy`, then open the preview URL it
prints. It runs as it is: a store, an `llm` step on the org's model (or the server's starter model), a rule, an approval
and three screens. Without a server, the demo of every step kind: `cori init my-desk --template support-desk`,
`cori check my-desk`, `cori run my-desk/workflows/support_ticket_triage ticket_id=T-1042`. The rest of this skill is the
loop inside such a folder.

## 0. Before writing

0. **Which server?** Look for the Cori MCP tools first (`list_apps`, `create_app`, `login`; hosts that load tools on
   demand list them only by name, e.g. `mcp__cori__list_apps`: search for them). If they are there, that is the org's
   server, whatever runs locally: take the app from `list_apps` / `create_app`, the session from `login`, and do not
   start `cori up` or use `--no-register`. Only with no Cori MCP server and no `.cori/session` is the server a local
   `cori up` (or pure local work with `--no-register`); say which one you picked.
1. **Which app?** An existing one: `cori apps list` / `cori apps get <id>` (or the MCP `list_apps` / `get_app`), then
   `cori pull <id> ./<id>` if the folder is not on this machine. A new one: `cori apps create <id> --roles a,b` (or
   the MCP `create_app`), then `cori init <id> --app <id> [--template team-requests|support-desk]`. Without a server at
   all (pure local work): `cori init <dir> --roles a,b --no-register`. Give the roles **strongest first**
   (`--roles manager,employee`): that order goes into `cori.toml` and decides how a person holding several acts (rule
   below). A new app gets a React front in `front/`, and so does `team-requests`; with `support-desk` add
   `--front react` (an existing app without `front/package.json`: init a scratch app and copy its `front/`).
2. **A session.** Every server-touching command reads `<app>/.cori/session`: the folder's server, and an agent's token.
   A human signs in once per server with `cori login <url>` (kept in `~/.cori/config.toml`; run in the folder, it also
   makes the folder work with that server); an agent gets it from the MCP `login` tool and writes `session_toml`
   verbatim to `.cori/session` (or runs the printed `bootstrap` when the folder does not exist yet). The agent session
   is app-scoped and short-lived: it deploys and pulls versions and reads their sandbox stores
   (`cori store query|tools … --version <hash>`); it cannot approve or promote, nor reach live data or live runs. When
   the cli says the session expired, call `login` again.
3. **Which connectors?** `cori connectors list` (names, kinds, tools, effects) and `cori connectors find "post a
   message"` (ranked tools with schemas; `--json` for the full schema). Never invent a connector or configure a server
   in the app: steps name org connectors, `cori.toml` only overrides or mocks them. If nothing fits, the MCP
   `request_connector` asks an admin.
4. **Read a finished app once** before the first step of a new app: `cori init /tmp/ref --template support-desk
   --no-register --no-git`. It holds one step of every kind, a store with rules and roles, every fixture shape, a
   human gate in a `branch`, a `map`, a child workflow, a schedule, and a front that calls an endpoint.

## 1. Writing

```
cori workflow new <id> --params ticket_id:string,note:string? --connectors helpdesk,slack --endpoint sync --roles support_agent
cori scaffold helpdesk getTicket --workflow <id> --name load_ticket            # typed step file + listed in steps:
cori scaffold slack postMessage --workflow <id> --name notify --after load_ticket
<write steps/<name>.ts for code, llm, human, store and control-flow steps; list each name in steps:>
cori store new <name>                                                          # when the app needs its own data
cori store tools <name> [--role <r>]                                           # its tool names per role (offline); then cori scaffold <name> <tool> …
```

- A workflow is `workflows/<id>/workflow.yaml` (pure YAML, `id` = folder name, **`steps:` ordered**), `README.md`
  (goal and verification, prose), `types.ts` (shared zod schemas), `steps/<name>.ts` (one top-level step per file,
  file name = step `name`). Nested steps (inside `branch`, `map`, `try`…) live in their parent's file and are not
  listed in `steps:`.
- A step file is `export default step.<kind>({ name, description, input: (ctx: Ctx<"<name>">) => ({ … }), output:
  z.object({ … }), …kind options })` with `import { step } from "@cori-do/sdk"`, `import type { Ctx } from
  "../.cori/context"`, `import { z } from "zod"`. Its `description` titles the step when a run is inspected, for
  everyone: what it does for the app's people, in their words ("Record the request as pending"), not its mechanism.
- `input` may only read `ctx.params.*`, `ctx.user.*`, `ctx.run.*`, `ctx.steps.<earlier step>.<field>`, `ctx.item`
  (inside `map`), as static member chains. Those references are the dataflow: independent steps run in parallel,
  writes keep `steps:` order. The whole `ctx` or `ctx.steps` escaping the binding, or dynamic access, is a compile error.
- Decisions in `code`; `llm` with a fixed `output` and a `tier`; `human` gates routed to a role with `timeout` and
  `on_timeout`; one declared connector per `mcp` / `store` / `http` / `activity` / `cli` step; `workflow` runs a
  sibling workflow as a child. See [references/steps.md](references/steps.md).
- **Roles, strongest first.** `[auth] roles` in `cori.toml` lists the app's roles from the most able to the least
  (`roles = { manager = "group:managers", employee = "group:employees" }`), whatever their alphabetical order. A person
  holding several (a manager who also logs time, an app's owner who holds them all) acts in every store as the first
  of them listed, never as a mix: give that role's files in `stores/<s>/roles/` everything the roles after it may do,
  so holding a second role never takes an ability away. Endpoints (`allow.roles`) and the front check every role a
  person holds. `cori check` warns when a role listed later may do everything an earlier one may, and more.
- Shared code lives in `lib/` at the app root (from `workflows/<id>/steps/*.ts`: `import … from "../../../lib/x.ts"`; from `workflows/<id>/types.ts`: `"../../lib/x.ts"`); nothing above `cori.toml` can be imported.
- `result:` gives the run a `headline` (`{{ params.x }}`, `{{ steps.s.field }}`, dotted paths only) and `fields`.
  `redact:` lists `steps.<step>.<field>` paths that never reach stored outputs or captured LLM inputs.
- Fixtures answer connectors under `cori run`: `fixtures/<connector>.json` per connector, `fixtures/llm.json` keyed by
  step name. Write them as you write the steps so the first local run passes.

## 2. The loop

```
cori check workflows/<id>            # dataflow proof, effects, lineage, result paths; `cori check .` = every workflow + stores vs migrations
cori run workflows/<id> k=v          # local Temporal + fixtures; --decide <step>='<json>' for human gates; --user <sub>:<role>
git add -A && git commit -m "…"      # the version records the commit; --require-clean refuses a dirty tree
cori deploy .                        # tar.zst → compile → change review → seeded sandbox stores → preview url
```

- Iterate `check` until clean. It regenerates `.cori/context.ts`: run it after adding or renaming a step, before
  editing bindings against `Ctx`. Errors name the file, the line and the fix ([references/check.md](references/check.md)).
- `cori run` prints one row per step (`ok | failed | not_taken`), the effects and the result. `store` steps use the
  embedded Postgres when `cori up` runs (the app's own local database, migrated and seeded once), else the fixture rows.
  After editing a migration or a seed file, `cori run … --reset-store` drops that local database and rebuilds it.
  `--mock-llm` keeps `llm` steps on `fixtures/llm.json` whatever `[llm]` says.
- `cori deploy` never changes what users see. It produces a **deployed** release (`v<n>`) with its own sandbox database
  per store, its preview url (the root of its own host, `<app>--v<n>…`, or `/apps/<app>/@v<n>/` under `cori up`, where
  the front runs that release's workflows on the sandbox) and a change review against the promoted version. Tell the user the preview url and what the review says (new writes,
  new connectors, schema drift).
- **Promotion is a human action**: `cori promote <app> <hash>` with a human session, or the console. A version that
  adds writes, or any version of a protected app, waits for an approver (the console inbox, or the approver's own
  `cori promote`). The agent session is refused on promote, approvals, rollback, live data, live runs and admin
  endpoints; say so instead of retrying.

## 3. Front
**Build a real front by default, in the same pass as the workflows**: `front/` with Vite, React, TypeScript and
Tailwind, built into `dist/`. The template's single `dist/index.html` with plain `fetch` is a smoke test, not a
deliverable; use it only when the user asks for no build step. Do not wait to be asked to "improve the frontend".

- `cori init` writes it ready to build: Vite config (`base: "./"`, `outDir ../dist`, dev proxy), `@cori-do/client`
  vendored, an app shell with hash routing (`VIEWS` in `src/views/index.tsx`), `src/components/ui.tsx`, `RunResult` (renders
  any run) and `src/lib/api.ts` (`must`, dev identity, config). Start with `cd front && npm install`; build screens in
  `src/views/`, reuse the kit rather than restyling from scratch.
- `npm run build` regenerates `src/lib/cori.ts` from the workflows (`cori gen client`), type-checks and writes
  `dist/`: run it after every workflow change and before every `cori deploy` (deploy warns when `dist/` is stale).
  Keep hash routes: the server's SPA fallback breaks path routes, and `cori up` serves the front under `/apps/<app>/`.
  The front's own paths stay out of `/v1/`, `/__cori/` and `/apps/`, which are the platform's on its host.
- The bar: a form for every workflow a role can start, pending / success / error / empty states on every call, the
  run's `result.headline` and `fields` rendered (never raw JSON), `useUser` and `<Protected>` shaping what each role
  sees, an approvals inbox when a workflow has a `human` step, a consistent layout that works at phone width.
- The organization's apps portal lists the app with its `[app] name`, `description` (one line) and the front's
  `favicon.svg` (`front/public/`): give it all three.
- Open the preview url in a browser and use it as each role before reporting.

The config, file-by-file recipe, pitfalls and the run API are in [references/front.md](references/front.md).

## 4. What to report
When you finish: the workflows touched and their `steps:`, the connectors and store roles they need (in their order,
strongest first), what `check`
proved (effects per connector), how you ran it locally and with which identity, the front's screens and which role sees each, the preview url and the change review
summary, and the exact `cori promote` command a human should run. Never claim a version is live; only a human promotes.

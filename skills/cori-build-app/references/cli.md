# The `cori` cli for authoring

Run every command inside the app folder unless noted (`--dir <app>` where accepted). Server-touching commands (`check`
and `run` included, for the connector registry) read `.cori/session` (see layout.md, Sessions); `--server` / `--token`
and `CORI_SERVER` / `CORI_TOKEN` override it; without any, they use a local `cori up` with your dev identity.

## Apps

| Command | Does |
|---|---|
| `cori apps list` | apps you own or build (admins: all): id, owner, promoted version, under a header row; `no apps on <server>` when there is none |
| `cori apps get <id>` | owner, builders, roles, promoted and last version, urls, the `cori pull` command |
| `cori apps create <id> [--name N] [--roles a,b] [--builders u1,u2]` | reserves the id; you become owner (builder or admin role). `--roles` strongest first (`manager,employee`): `cori init` writes them into cori.toml in that order |
| `cori apps protect <id> [--off]` | every promotion needs an approver |
| `cori apps sync-client <id>` | (owner or admin) registers the app's own sign-in client again through the server's client hook (idempotent) |
| `cori apps delete <id> [--confirm <id>]` | **human, owner or admin, gated**: deletes the app for good (every version, live and sandbox store databases, schedules, runs, approvals; the audit stays). Refused on a protected app; an approver's request deletes at once, anyone else's waits in the inbox |
| `cori init <dir> [--app <id>] [--name "Display name"] [--template team-requests\|support-desk] [--roles a,b] [--front react\|plain\|none] [--no-git] [--no-register]` | creates the folder: `cori.toml` (`[app] name` from `--name`, else the name the app was reserved with, else the id or the template's; `[auth] roles` from `--roles` in their order, strongest first, else the reservation's, else `admin, app_user`; a template keeps its own), `AGENTS.md`, `README.md`, `.gitignore`, `fixtures/llm.json`, `workflows/` and the front; `git init` + first commit. `--front react` (the default for a new app) writes `front/`: Vite + React + TypeScript + Tailwind, the generated `src/lib/cori.ts`, `@cori-do/client` vendored in `vendor/cori-client/`, an app shell with hash routing, a UI kit and `RunResult`, `dist/` in `.gitignore` and a placeholder `dist/index.html` until `npm install && npm run build`. `--front plain` writes one committed `dist/index.html`; `--front none` no front and no `[front]`. A template keeps its own front unless `--front` is given. With a connection (flags, `CORI_SERVER`/`CORI_TOKEN`, `cori login`), or when the local dev server answers at `http://127.0.0.1:8080`, it registers the app (with the display name) and writes `.cori/session`; otherwise it prints a note saying neither happened (`cori up`, then `cori apps create <id>` and `cori login <url>`). `--template team-requests` copies a small finished app that runs on any server as it is, its React front included and built in `dist/` (deploy it as it is; rebuild after a change); `--template support-desk` copies the demo of every step kind. A template's id becomes the app's everywhere in its files, and its AGENTS.md is the same conventions text as every app's |
| `cori pull <app> [dir] [--hash H] [--force]` | restores the promoted (or given) version into `dir` as a git repository with `.cori/session`; refuses a non-empty folder without `--force` |
| `cori login [url] [--org <slug>] [--token T] [--device] [--global]` | your login to a server: browser + PKCE (or the device grant), kept in `~/.cori/config.toml` (one per server and organization, refreshing itself; the last one is the default outside app folders); in an app folder, the folder now works with that server too (`--global`: your login only). On a server hosting several organizations, `--org` signs in to that one (the browser asks otherwise), and an app folder belongs to one: its commands use your login to it, never another organization's, and a folder of another organization keeps its own. `--list` shows your logins; `--forget [--org <slug>]` removes one |

## Connectors

| Command | Does |
|---|---|
| `cori connectors list [--json]` | declared connectors: name, kind, tools with effects (one line each); `--json` adds schemas |
| `cori connectors find "<words>" [--json]` | ranked tools over names, descriptions, schema fields; `--json` includes input schemas |
| `cori connectors export --to .mcp.json` | writes the declared servers into an MCP host config so the agent can call them directly at authoring time (credentials as env var names) |
| `cori connectors requests` | (admin) open `request_connector` requests |
| `cori connectors add <url> --name <n>` | (admin) declare a connector (`add activities --yaml …` for native activities, `--from .mcp.json` to import a host config) |

## Workflows and steps

| Command | Does |
|---|---|
| `cori workflow new <id> [--name] [--description] [--params n:type[:desc],…] [--connectors a,b] [--endpoint sync\|async] [--roles a,b] [--schedule "<cron>"] [--schedule-tz TZ]` | `workflows/<id>/` with `workflow.yaml` (`steps: []`), `README.md`, `types.ts`, `steps/`, `.cori/context.ts`. Types: `string`, `number`, `boolean`, `enum`, `list`, `object`; a trailing `?` makes the parameter optional. `--schedule` replaces the endpoint |
| `cori scaffold <connector> <tool> --workflow <id> [--name <step>] [--after <step>\|start] [--force] [--print]` | fetches the registry, writes `steps/<step>.ts` typed from the tool's schema (input skeleton as the tool expects, required fields first, output as zod from the output schema or a fixture), inserts the name in `steps:` after `--after` (default: last). `--print` to stdout instead; refuses an existing file without `--force`. A store connector's tools come from `stores/<ref>/` (works with the server down); an unknown tool name is an error listing the valid ones |
| `cori store new <name> [--roles a,b] [--tenant <column>]` | `stores/<name>/` (rules, roles, migration, seed), `fixtures/<name>.json`, the `cori.toml` binding; a shared table, or rows per user with `--tenant` |
| `cori check [workflows/<id>…] [--json] [--no-deno]` | compiles: manifest, `steps:` vs files, bindings, dataflow DAG, effects, lineage, result paths; regenerates `.cori/context.ts`; runs `deno check`. `cori check .` (or the app folder) compiles every workflow and validates stores against migrations. Fetches the registry itself |
| `cori check --conventions [--json]` | prints the conventions (`AGENTS.md` text, step kinds with option shapes, result templates) |
| `cori run workflows/<id> [k=v…] [--decide <step>=<json>] [--user <sub>:<role,role>] [--mock-llm [file]] [--no-store] [--reset-store] [--store-database-url U] [--no-mock <connector>] [--record] [--json]` | local run on a Temporal dev server with the fixtures. `--reset-store` drops the app's local store databases (`cori_store_<app>_<store>_local`) before provisioning, so edited migrations and `seed/*.sql` apply again from scratch (a migration or seed file is otherwise applied once); `--decide` pre-records a human decision; `--user` sets the principal (default: `dev` holding the endpoint's first `allow` role; a `dev:` prefix is stripped); `--record --no-mock <c>` calls `<c>` live and writes its fixture |
| `cori gen client --dir <app> [--out front/src/lib/cori.ts]` or `--app id` | the typed client: from the folder, or the promoted version's from the server (the app's builders) |

## Versions

| Command | Does |
|---|---|
| `cori deploy [dir] [-m "message"] [--require-clean] [--json]` | a new numbered release (`v9`; the message defaults to the last commit's subject; identical content is the same release, deployed again). Bundles the folder (`tar.zst`, skipping `.cori`, `.git`, `node_modules`, `target`, `.DS_Store`), records the git commit/branch/dirty flag, uploads; the server compiles, reviews against the promoted version, provisions a seeded sandbox per store and prints the version hash, the review and the preview url. Users see nothing new. The deploy runs to its end on the server even when the connection is cut on the way (a proxy's time limit): the cli then waits for the release to appear (5 minutes at most) rather than failing; deploy again only if it says none came |
| `cori releases <app> [--json]` | the live release, those newer than live with what each needs to go live (ready, needs approval and why, requested, approved) and the migrations it would run on live data, older ones, the go-live history |
| `cori promote <app> <v9\|hash>` | **human**: makes the release live: applies its pending migrations live, flips the default (what the app's live front runs), syncs schedules. A new release waits for an approver on a protected app or when the review adds writes (an approver's promote goes live at once and closes the request waiting for it); a release that was live before goes back without approval and without migration (warns when the live data ran migrations it lacks) |
| `cori rollback <app>` | **human**: `cori promote` with the release live before this one |
| `cori reseed <app> <v9\|hash>` | resets that release's preview data (its sandbox stores, from the migrations and seed files) |
| `cori undeploy <app> <v9\|hash> [--force]` | **human**: deletes a release that is not live, with its sandbox databases, bundle and preview url; refused while runs are going on it unless `--force` terminates them |

## Local platform

| Command | Does |
|---|---|
| `cori up` | everything on the laptop: Temporal dev server, the server on :8080 with dev auth (`dev:<sub>:<roles>`), embedded Postgres for stores, an in-process worker, connector mocks, the console at `/` |
| `cori doctor` | deno, the temporal CLI and a Temporal answering on 7233; the connection in use (environment, the folder's own token, your login to the server, or the local dev default) and whether its server answers (`http://127.0.0.1:8080` by default); the server's database, store, auth, secrets checks and, under builtin auth, the identity checks: the issuer's discovery and keys, your token, one token exchange per connector audience (admin role) |
| `cori login <url> --global` | your login only: the app folder you are in keeps working with its own server |
| `cori connectors add <url> --name <n>` · `cori connectors refresh\|remove` · `cori secrets set\|list` · `cori pools create\|list` · `cori llm bind\|calls` | org administration (admin roles, human session): declare connectors, broker secrets, worker pools, LLM step bindings and their captured calls; on a server with LLM Ops, also shadow bindings and `cori llm eval\|dataset\|finetune` (the distillation loop) |
| `cori store tools <store> [--role <r>] [--dir .]` | offline, in an app folder: each store role's typed tools (`getItem`, `listItems`, `createItem`, …) with Read/Write and parameters, from the migrations and role files. `--online`, `--version <hash>` or `--app` ask the server instead (what your session's role gets) |
| `cori store list \| query <store> "<sql>" [--app] [--version <hash>]` | the app's store databases; a read-only query on live or a sandbox (an agent session: `--version` only) |

## What the agent session may do
Its own app only: `apps list|get`, `pull`, `deploy`, `releases`, `reseed`, `gen client`, `store list`,
`store tools|query --version <hash>` (sandboxes), preview runs of a deployed version, and the connector registry
(`check`, `run`, `scaffold`, `connectors list|find`). Everything else (`promote`, `approve`, `rollback`, `undeploy`,
`apps create|protect|delete|sync-client`, live data and live runs, `connectors add|refresh|remove`, `secrets`, `pools`, `llm`, the audit
log) answers 403 naming the human path: report the command for a person to run instead of retrying.

## Typical outputs

```
$ cori workflow new onboard --params "employee_id:string:HR id,laptop:boolean?" --connectors helpdesk --endpoint async --roles ops
created workflows/onboard/ (workflow.yaml with `steps: []`, README.md, types.ts, steps/, .cori/context.ts)
next: `cori scaffold <connector> <tool> --workflow onboard` per connector step, or write steps/<name>.ts and list it in `steps:`; then `cori check workflows/onboard`

$ cori store new onboarding
created stores/onboarding/ (rules.yaml, roles/{ops,ops_lead}.yaml, migrations/0001_init.sql, seed/0001_demo.sql) and fixtures/onboarding.json
cori.toml: added [connectors.onboarding] kind = "store", ref = "onboarding" (steps use `connector: "onboarding"`, workflow.yaml lists it in `connectors:`)

$ cori check workflows/onboard
workflow      onboard (sha256:138124430984)
context.ts    generated (2 steps, 0 nested)
deno check    ok
dataflow      load_items → count
effects       onboarding · query:Read
endpoint      sso · async · allow {"roles":["ops"]}
warnings      0
```

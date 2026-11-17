# Cori

Cori builds your team's internal apps: expense requests, approvals, onboarding checklists, anything your team tracks
today in spreadsheets and email threads. You describe the app to your AI agent; Cori checks it, runs it and hosts it,
with roles, approvals and a record of who did what.

Under the hood, Cori is a durable, typed, permissioned backend for apps built by agents. An app is a folder of workflows
and data stores: Cori proves each workflow before it runs, runs it on Temporal and Postgres, and puts every change
behind a person's decision.

## How Cori works

- **An app is a folder**, kept in git like any code: `cori.toml` (the app's roles, connectors and front),
  `workflows/<id>/` (a `workflow.yaml` listing its steps, and one TypeScript file per step), `stores/<name>/` (Postgres
  migrations, demo rows, rules and one file per role), `fixtures/` (answers for local runs) and `front/`, built into
  `dist/`. `cori init` writes an `AGENTS.md` into it, so that any coding agent knows the conventions.
- **Steps have kinds**: `code` (pure TypeScript), `llm` (one model call that answers a fixed schema), `store`, `mcp`,
  `http`, `cli`, `activity` (a native Temporal activity in Go or .NET, on a team's own worker), `human` (an approval or an
  input, routed to a role) and `workflow` (a child), with `branch`, `switch`, `map`, `loop`, `try` and `wait` for control
  flow. Rules and decisions are code, written at design time; a model only fills in a schema.
- **`cori check` proves a workflow before it runs.** It type-checks every step, then derives from the references steps
  make the data flow, each step's effects (reads, writes, what reaches a model), what may run in parallel and where
  every value comes from. None of it is declared by hand, so none of it drifts.
- **Runs are durable.** Each run is a Temporal workflow: it survives restarts, retries what may be retried and waits days
  for a person's decision. Builders' code runs in sandboxes under gVisor that hold no credential and
  reach no network.
- **Stores are permissioned Postgres.** Role files and rules are compiled into Postgres grants and row-level security:
  every call runs as a database role that holds only its columns and sees only its rows, whatever the SQL. A change can
  wait for an approver before it is applied.
- **Releases are tried before they go live.** `cori deploy` uploads a release with its own seeded sandbox databases and
  a preview link, while the live app stays as it is. `cori promote` makes it live and runs its migrations on the live
  data: a person does it, never an agent, and a protected app also needs an approver. `cori rollback` goes back.
- **People and agents.** People sign in with OpenID Connect and hold an app's roles. An agent gets a short-lived session
  for one app from Cori's MCP server: it builds, deploys and tries releases on their sandboxes, while going live,
  approvals and live data stay with people.
- **Everything is recorded.** A run keeps what each step received and gave back; deploys, promotions, approvals and
  access changes go to the audit log, and usage (runs, deploys, model calls priced by the server, storage) to a ledger
  with limits.

The guide an agent follows to write a Cori app is the plugin's skill:
[skills/cori-build-app](skills/cori-build-app/SKILL.md).

## Run it on your machine

`cori up` runs a whole Cori locally, with dev sign-in and mocked connectors: a server, a worker, a Temporal dev server
and Postgres. It needs Deno 2, the Temporal CLI and Postgres (Postgres.app, or `brew install postgresql@17`).

```bash
cori up &                                    # the console at http://127.0.0.1:8080/
cori login http://127.0.0.1:8080             # your dev identity, the default server outside app folders
cori init my-desk --template support-desk    # the demo app: every kind of step
cori check my-desk                           # type-check and verify every workflow
cori run my-desk/workflows/support_ticket_triage ticket_id=T-1042
cori deploy my-desk                          # release v1, with its own sandbox data and preview link
cori promote my-desk v1                      # live at http://127.0.0.1:8080/apps/my-desk/
```

To build the command line from source: `cargo build -p cori-cli` (the Rust toolchain in `rust-toolchain.toml`, and
`protoc`). [CONTRIBUTING.md](CONTRIBUTING.md) has the tests.

## Repository

| Path | Contents |
|---|---|
| `crates/cori-cli` | the `cori` command line, which also runs the server, the workers and the sandbox |
| `crates/cori-server` | the HTTP API, the MCP server, the console and the apps portal: releases, approvals, people |
| `crates/cori-engine` | the Temporal workflow and its activities, the step runner's host, the sandbox |
| `crates/cori-compiler` | `cori check`: types, data flow, effects, lineage |
| `crates/cori-store`, `cori-store-core`, `cori-store-engine` | Cori Store: permissioned Postgres per app |
| `crates/cori-vault` | each app's credentials under a key of its own in HashiCorp Vault, off unless `CORI_VAULT_ADDR` is set |
| `packages/` | `@cori-do/sdk` (steps), `@cori-do/client` (fronts), the Deno step runner, `@cori-do/cli` (the command line on npm) |
| `sdks/` | Go and .NET helpers for native Temporal activities |
| `templates/` | the apps `cori init` starts from: `team-requests`, `support-desk` |
| `skills/`, `.claude-plugin/`, `plugin.json`, `mcp.json`, `.mcp.json` | the plugin for AI agents: its skills, its manifests and Cori's MCP server |
| `scripts/` | the end-to-end tests and the release tooling |

## Contributing, security, licence

- Building, testing and releasing: [CONTRIBUTING.md](CONTRIBUTING.md).
- Found a vulnerability? Report it privately: [SECURITY.md](SECURITY.md).
- Apache 2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). What `cori init` copies into your apps, and the skills, are MIT-0 ([LICENSING.md](LICENSING.md)).

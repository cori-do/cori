# AGENTS.md — Working on the Cori codebase

**Audience:** code assistants (Claude Code, Cursor, Copilot) editing Cori itself, not authoring workflows with Cori. This file is the single source of truth for how Cori is structured today and what conventions to follow. Read it end-to-end before touching code.

If you're trying to design a Cori workflow `cori_save_workflow`, you want [skills/cori_save_workflow/SKILL.md](skills/cori_save_workflow/SKILL.md), not this file.

---

## What Cori is, in one paragraph

Cori turns one-off agent conversations into deterministic, executable TypeScript workflows. The agent writes the workflow at design time; the worker executes it at runtime with no LLM in the loop unless an `llm` step explicitly asks for one. Workflows are **folders run by path** (`cori run ./translate_fr`) or by **git ref** (`cori run github.com/org/workflows/translate@v1.1.12`); there is no registry. Execution is Temporal-backed: every `cori run` resolves the folder (fetching remote refs into `~/.cori/cache/remote/<host>/<repo>/<sha>/` when needed), compiles to `~/.cori/cache/`, runs the **planner** to map each step to an identity-derived task queue, starts the single generic `CoriWorkflow`, dispatches each step to the chosen queue, and writes a JSON trace to `~/.cori/runs/<key>/`. Disk is the only truth: there is no SQLite anywhere in the codebase.

---

## Locked architectural decisions

Do not re-litigate these without explicit human approval.

1. **One workflow type.** `CoriWorkflow` (in [crates/cori-worker/src/workflow.rs](crates/cori-worker/src/workflow.rs)) handles every compiled DAG. The DAG is data passed in `WorkflowInput`, not code. Mid-run re-auth is **new control flow inside this single workflow** (Phase 6 signal/wait), never a second workflow type.
2. **Four activity kinds, closed set.** `cori_cli`, `cori_mcp_tool`, `cori_code`, `cori_llm` — all in [crates/cori-worker/src/activities.rs](crates/cori-worker/src/activities.rs). Builtins (`map`, `for_each`, `branch`, `parallel`, `wait`) are workflow code, **not** activities, and are not implemented yet in v1.
3. **Single execution path.** The old in-process executor was deleted during the Temporal migration. Do not reintroduce it, do not feature-flag a parallel path. Temporal is the only runtime.
4. **DAG in `WorkflowInput`.** The full compiled DAG (including per-step `task_queue` assigned by the planner) is serialized into `WorkflowInput.compiled_dag` at workflow start. The workflow body never reads disk — that would break determinism on replay.
5. **Broker is the trust boundary.** Every external side effect (`std::process::Command`, HTTP, MCP, OAuth) goes through [crates/cori-broker](crates/cori-broker/src/lib.rs). Activity handlers are thin wrappers over broker functions via `tokio::task::spawn_blocking` (the broker is sync; the Temporal worker is async).
6. **Disk is truth, files-only.** Workflows live in user-owned folders (anywhere on disk, typically in a git repo). Cori writes nothing into them. Cori's own state is in `~/.cori/`: `cache/` (compiled DAGs, rebuildable), `runs/<folder>-<pathhash>/*.json` (trace history), `credentials/` (token metadata; real tokens go in the OS keychain), `cluster/<queue>.json` (worker capability reports), `schedules/<id>.json` (Console-registered cron schedules — fired by the cron driver inside `cori work`), `state/console.json` (runtime location of the Console — port + pid, never the token), `config.toml`. **No SQLite. Do not reintroduce `rusqlite`.**
7. **Identity-derived task queues.** Queue names are `cori.user.<user_id>` (`Person` identity, from `OsUser` in v1) or `cori.service.<pool>` (`Service`, from `cori work --shared <name>`). Helpers live in [crates/cori-protocol/src/lib.rs](crates/cori-protocol/src/lib.rs) (`task_queue_for`, `identity_from_queue`). **Cross-user dispatch is impossible by construction** — Temporal's matching layer physically separates queues. The old `cori-default` constant is gone; do not reintroduce a default queue.
8. **Two-place ownership enforcement (defense in depth).** (a) The planner routes each step to a queue derived from authenticated identity — physical isolation. (b) The broker resolves credentials keyed by `user_id` in `WorkflowInput` — token isolation. Both checks must stay; do not collapse to one.
9. **Worker presence is Temporal-native.** Use `DescribeTaskQueue` only on human-frequency paths (`cori status`, `cori check`). Never per-step. The v1 fallback for cluster presence is reading `~/.cori/cluster/<queue>.json` files published by `cori work`. **Do not** use Temporal Worker Versioning / Build IDs for capability routing — versioning is reserved for future Cori-binary rollout. **Do not** introduce Nexus in v1 (noted as v2 possibility).
10. **TypeScript only for user step files.** Zod is the schema library. Static parsing of step files is regex-based today (see [crates/cori-compiler/src/step_parser.rs](crates/cori-compiler/src/step_parser.rs)) — a swc/oxc migration is planned but not in v1.
11. **Local Temporal auto-spawn, no bundling.** If `temporal.host` isn't configured and `127.0.0.1:7233` isn't already serving, `cori run` shells out `temporal server start-dev` as a supervised child (see [crates/cori-run/src/temporal_endpoint.rs](crates/cori-run/src/temporal_endpoint.rs)). Production deployments override via `temporal.host` in `~/.cori/config.toml`. No Temporal Cloud support.

---

## CLI surface (the complete set)

Eight verbs. Three take a workflow path *or* remote git ref; the rest are machine-scoped.

```
cori run <path-or-ref> [--json] [--dry-run] [--update] [--yes] [<param>=<value>...]
cori check <path-or-ref> [--update] [--yes]                # preflight only
cori show <path-or-ref>                                    # inspect workflow + recent runs
cori runs list|show                                        # read run history
cori work [--shared <name>] [--no-console] [--console]     # put this machine in the loop +
          [--console-port <port>] [--console-open]         #   serve Cori Console on 127.0.0.1
cori login <capability>                                    # OAuth/CLI sign-in
cori status                                                # machine: endpoint + identity + caps + workers + pinned remotes
cori config get|set                                        # ~/.cori/config.toml access
```

The Cori agent skill is installed via `npx skills add cori-do/cori` (not a `cori` subcommand).

Remote refs follow `go mod`-style syntax: `host/owner/repo[/subpath][@<ref>]`. Refless picks the highest `vX.Y.Z` tag; `@v1` / `@v1.2` pick the highest matching prefix; exact tags and 7+ hex shas are immutable. SSH form `git@host:repo[@ref]` is supported. `--update` re-resolves mutable refs; `--yes` (or `CORI_ASSUME_YES=1`) skips the first-run consent prompt. See [remote-workflows.md](remote-workflows.md) for the full design.

No `init`, `start`, `register`, `workflows`, `save`, `ls`, `rm`, `serve`, `workers`, `demo`. These were deleted in Phase 1 of the redesign; do not reintroduce.

---

## Repository layout

```
crates/                                Rust workspace (edition 2024, MSRV 1.94)
  cori-cli/        binary `cori`; clap-derived commands only (thin wrappers over cori-run)
  cori-run/        run pipeline library: planner, temporal_endpoint, remote-ref resolver,
                   workflow_loader, paths, config, ConsentCallback, ProgressSink, run_workflow()
  cori-worker/     Temporal worker: workflow + activities + runtime + runner
  cori-compiler/   manifest + step parsing → CompiledWorkflow (computes Placement)
  cori-broker/     capability broker (cli, code, mcp, llm, oauth, cli_auth) — trust boundary
  cori-ledger/     placeholder for cost analytics (mostly empty in v1)
  cori-manifest/   YAML schema + parser (manifest.md frontmatter + body)
  cori-protocol/   wire types (CompiledWorkflow, Placement, WorkerIdentity, RunTrace,
                   ActivityTrace, TokenUsage, task_queue_for, …)
packages/                              pnpm workspace (Node ≥ 20)
  sdk/             @cori-do/sdk — what user step files import (`step.cli`, `step.code`, …)
  runner/     Deno script that hosts `code` activities
  console/         @cori-do/console — React Router v7 SPA (`ssr: false`).
                   `pnpm --filter @cori-do/console build` → build/client/,
                   embedded into cori-console by rust-embed at compile time.
skills/            Cori agent skills (authored via `npx skills add cori-do/cori`)
examples/          Reference workflows (hello_world, code_only, translate_product_sheets_fr)
scripts/install.sh
```

The remote-ref pipeline lives in [crates/cori-run/src/remote/](crates/cori-run/src/remote/mod.rs): `refspec` (parsing + semver), `git` (system `git` subprocess wrappers), `pins` (`pins.json`), `trust` (`trust.json` + consent prompt).

---

## Build, lint, test

```bash
cargo build --workspace
pnpm install && pnpm build

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm -r lint && pnpm -r typecheck && pnpm -r test
```

**Build ordering for the Console**: `pnpm --filter @cori-do/console build` must run **before** `cargo build -p cori-console` so the SPA assets exist at the path `cori-console/build.rs` embeds via `rust-embed`. The build script will emit a `cargo:warning` and write a placeholder `index.html` if the assets are missing — the binary still compiles, but `/` serves a "console not built" splash until you run the SPA build.

CI builds on Linux/Mac/Windows; tests run on Linux only.

### Running Temporal integration tests

Tests that touch a real Temporal server are `#[ignore]`-gated. To run them:

```bash
temporal server start-dev --port 7233 &
cargo test -p cori-worker -- --ignored --nocapture
```

The smoke / end-to-end tests live in [crates/cori-worker/tests/temporal_smoke.rs](crates/cori-worker/tests/temporal_smoke.rs) and the compiler golden test in [crates/cori-compiler/tests/translate_product_sheets_fr.rs](crates/cori-compiler/tests/translate_product_sheets_fr.rs).

### Running the binary end-to-end

Solo flow (no other process required — Cori auto-spawns the dev Temporal):

```bash
# Optional terminal A: persistent worker that exposes this machine's local
# files and CLIs to other runs. Skip this and `cori run` uses an ephemeral
# in-process worker on `cori.user.<you>`.
cargo run -p cori-cli -- work

# Terminal B: run a workflow by path.
cargo run -p cori-cli -- run examples/hello_world
```

Multi-machine flow (e.g. an org notion pool):

```bash
# Machine A (a service worker on org infra, configured for org Temporal)
cargo run -p cori-cli -- work --shared notion-pool

# Machine B (a user laptop)
cargo run -p cori-cli -- run ./some_workflow_using_notion
```

Override the Temporal endpoint with `CORI_TEMPORAL_TARGET=http://host:port` or `cori config set temporal.host …`. Override the Deno binary with `CORI_DENO=/path/to/deno`. Override the re-auth wait timeout with `CORI_REAUTH_TIMEOUT_SECS=…`.

---

## Execution model

```
cori run <path-or-ref>
  → remote::resolve(arg, update)  — local-wins; otherwise classify as git ref,
       resolve `@ref` to sha (using `pins.json` cache unless --update),
       fetch into ~/.cori/cache/remote/<host>/<repo>/<sha>/ via system `git`,
       prompt first-run consent (trust.json), then hand off a local folder
  → workflow_loader::load(folder)
       ├─ canonicalise, hash folder, look up `~/.cori/cache/<key>.json`
       └─ on miss: cori_compiler::compile(path) and persist atomically
  → resolve LLM credentials, discover capabilities, build Capabilities snapshot
  → preflight auth (per-capability `authed` from CapabilityReport)
  → temporal_endpoint::resolve() — configured / 127.0.0.1 / auto-spawn dev
  → preflight_check (TCP) the endpoint
  → identity = OsUser.resolve() → WorkerIdentity::Person { user_id }
  → planner::assign_queues(&mut compiled, &identity, &ClusterView)
       fills every CompiledStep.task_queue from its Placement
  → CoriTemporalRuntime::connect(target, "default", task_queue)
  → run_workflow_once(...) — register CoriWorkflow + CoriActivities,
       start the workflow, await result
  → workflow loop (workflow.rs):
       for each CompiledStep:
         build ActivityInput (carries user_id, run_id, source_path)
         ctx.start_activity(..., ActivityOptions { task_queue: step.task_queue, … })
         merge ActivityOutput into the accumulator
         on ApplicationFailure type=="NeedsReauth": ctx.wait_condition(reauth_completed, 15min)
  → promote WorkflowOutput → RunTrace (with requesting_identity, per-activity
       task_queue + worker_identity)
  → persist to ~/.cori/runs/<key>/<utc>.json (atomic tempfile + rename)
```

Per-activity flow inside `crates/cori-worker/src/activities.rs`:

```
ActivityInput → set_broker_ctx() (Deno runtime, caps, llm opts, source_root,
                                  credentials_dir via task-local)
             → tokio::task::spawn_blocking(|| broker_fn(...))
             → broker resolves tokens via cori_broker::credentials::for_user
                                       and cori_broker::oauth::token_for
             → wrap as ActivityOutput or convert BrokerError → ApplicationFailure
```

### Retryable vs non-retryable taxonomy

Apply this consistently when classifying `BrokerError → ApplicationFailure`:

- **Retryable** (Temporal will retry): network timeouts, 5xx HTTP, transient LLM rate limits, MCP server temporarily unreachable.
- **Non-retryable** (`ApplicationFailure::non_retryable`): schema validation failure, missing capability (CLI not on PATH), authentication failure (`NeedsReauth`), malformed step metadata, invalid input shape. `NeedsReauth` uses the type tag `"NeedsReauth"` so the workflow body can catch it specifically and suspend on a signal.

Default `max_attempts` per kind:

| Kind | Default `max_attempts` | Why |
|---|---|---|
| `cori_code` | 3 | Pure, no side effects, safe to retry |
| `cori_llm` | 3 | Paid but idempotent; cost ledger keys on `(run_id, activity_id, attempt)` |
| `cori_cli` | 1 | May mutate external state |
| `cori_mcp_tool` | 1 | May mutate external state |

`schedule_to_start_timeout = 30s` on routed activities makes "no worker on this queue" fail fast with an actionable error.

A step can opt into different retries via `retries.max` in its metadata.

---

## Determinism rules — workflow body only

Anything inside `CoriWorkflow::run` (and any helper it calls) MUST follow these rules. Violations desync on replay and fail silently.

- ❌ No `std::time::Instant::now`, `chrono::Utc::now`, or any wall-clock function. Use `ctx.workflow_time()`.
- ❌ No `rand::*`, `tokio::time::sleep`, `tokio::spawn`. Use `ctx.timer(Duration)` and the SDK's child-future spawning.
- ❌ No file, network, or capability-discovery I/O. All side effects go in activities. The planner runs in the CLI before workflow start; the workflow never re-plans.
- ❌ No new thread spawning, no `Mutex`-based shared state across awaits.
- ✅ Workflow body is a pure function of `WorkflowInput` + activity outputs + incoming signals. Period.

The only nondeterministic input added in Phase 6 is "a `reauth_completed` signal arrived" — Temporal records that in history, so it is deterministic on replay.

Activity bodies (`activities.rs`) are free from these constraints — they're the place where I/O happens. They DO run wall clocks (`Utc::now` for `started_at`/`ended_at`) and DO call into the broker.

---

## Adding a new feature: where does it go?

| You want to add… | It belongs in… |
|---|---|
| A new step kind | Start with `StepKind` in [cori-protocol](crates/cori-protocol/src/lib.rs), then the SDK ([packages/sdk](packages/sdk/src/index.ts)), then the compiler parser ([cori-compiler/src/step_parser.rs](crates/cori-compiler/src/step_parser.rs)), then a broker module + an activity handler. Update the workflow dispatch loop last. |
| A new CLI verb | [crates/cori-cli/src/commands/](crates/cori-cli/src/commands) + wire it in [main.rs](crates/cori-cli/src/main.rs). |
| A new LLM provider | [crates/cori-broker/src/llm/providers.rs](crates/cori-broker/src/llm/providers.rs). Add credential resolution to the same module, pricing to `pricing.rs`. |
| A new manifest field | [crates/cori-manifest/src/lib.rs](crates/cori-manifest/src/lib.rs). Update [skills/cori_save_workflow/references/manifest_schema.md](skills/cori_save_workflow/references/manifest_schema.md) in lockstep. |
| Trace shape changes | The trace types live in [crates/cori-protocol/src/trace.rs](crates/cori-protocol/src/trace.rs) (`RunTrace`, `ActivityTrace`, `TokenUsage`). Update `skills/cori_save_workflow/references/trace_interpretation.md` too. |
| A new `Placement` variant or routing rule | [crates/cori-protocol/src/lib.rs](crates/cori-protocol/src/lib.rs) for the enum, [crates/cori-compiler/src/lib.rs](crates/cori-compiler/src/lib.rs) for how it's inferred, [crates/cori-run/src/planner.rs](crates/cori-run/src/planner.rs) for how it maps to a queue. |
| A new CLI auth adapter | [crates/cori-broker/src/cli_auth/](crates/cori-broker/src/cli_auth) — one tiny adapter per known CLI. |
| A new OAuth flow | [crates/cori-broker/src/oauth/](crates/cori-broker/src/oauth) (`flow/pkce.rs`, `flow/device.rs`, `flow/client_credentials.rs`, `flow/dcr.rs`, `metadata.rs`). |
| A new known git host for remote workflows | The default allowlist (`github.com`, `gitlab.com`, `bitbucket.org`) is in [crates/cori-run/src/remote/mod.rs](crates/cori-run/src/remote/mod.rs); custom hosts go in `~/.cori/config.toml` under `[remotes].hosts`. |
| Builtin step support (`map`/`for_each`/`branch`/`parallel`/`wait`) | This is the largest known gap. Implement in workflow code in [workflow.rs](crates/cori-worker/src/workflow.rs); the compiler already accepts the kind but the runtime short-circuits with "deferred" notices. Keep all builtin logic deterministic (no activity dispatch inside `wait`, etc.). |

---

## On-disk layout (`~/.cori/`)

```
~/.cori/
  config.toml              # temporal.host (optional), llm.<provider>.api_key, [remotes].hosts, …
  cache/                   # rebuildable compiled DAGs, keyed by sha(path + content_hash)
    remote/                # fetched remote workflows (system `git` clones)
      pins.json            # <host/repo//subpath@ref> → sha (source of truth for resolution)
      trust.json           # (repo, sha) pairs the user has consented to run
      <host>/<repo>/<sha>/ # working tree at <sha>
  runs/<key>/*.json        # run traces; key = <folder>-<short(abs_path)> (local) or
                           #              <repo_or_subpath_leaf>-<short(host/repo//subpath)> (remote)
  credentials/             # OAuth/CLI TOKEN METADATA only (expiry, owner). Tokens → OS keychain.
  cluster/<queue>.json     # capability reports published by `cori work` (v1 cluster-presence hack)
  schedules/<id>.json      # schedule intent — read by the cron driver inside `cori work`
                           #   (id = sha256(source + schedule)[..12]). Console CRUDs this.
  runtime/                 # cached Deno binary + node_modules
  state/                   # temporal-dev.pid, dev-engine-announced marker,
                           # console.json (port + started_at + pid; NO token)
```

No SQLite, no schema, no migrations. Cross-trace queries are filesystem walks. If a hot path ever needs an index, add a rebuildable file under `cache/` — never promote files back to a system of record.

Run-history key shape: `<folder_name>-<8-hex of sha256(absolute_resolved_path)>` for local workflows, or `<repo-or-subpath-leaf>-<8-hex of sha256(host/repo//subpath)>` for remote workflows (the key ignores `@ref` so different versions of the same workflow share history). Same folder name in two repos → two distinct directories.

Cache key shape: 12 hex of `sha256(absolute_path + content_hash_of_folder)`. Touching any file in the workflow folder invalidates cache automatically.

Remote-workflow auth: Cori never stores git credentials. SSH (`git@host:repo`) uses the OS SSH agent / `~/.ssh/config`; HTTPS uses the user's existing git credential helper. Auth failures print the underlying `git` stderr verbatim with the hint to debug via `git clone <url>` directly.
---

## What v1 does NOT include

Push back if asked to add any of these during v1 work:

- Multi-user web management plane, cost dashboards, RBAC, audit logs, multi-user orgs. **Carve-out:** Cori Console (`crates/cori-console` + `packages/console`) is a single-user local web UI served by `cori work` on `127.0.0.1` only — it is a read-and-trigger surface, not a multi-user management plane. Don't widen it beyond that.
- Cori cloud workers; only self-hosted workers
- Serverless adapter (Lambda/Cloudflare)
- Hub / marketplace for shared workflows
- Wasm sandboxing (planned for v3)
- Temporal Cloud, multi-region Temporal
- Temporal Worker Versioning / Build IDs for capability routing (reserved for Cori-binary rollout)
- Temporal Nexus (possible v2 seam)
- Python `code` activities (TS only)
- Git sync, `cori push` / `cori pull`
- A general-purpose secrets vault (the OAuth/CLI token store under `cori-broker::oauth` is scoped to that purpose only)
- Builtin steps (`map`, `for_each`, `branch`, `parallel`, `wait`) — accepted by the compiler, deferred at runtime
- Full AST step parsing via swc/oxc — current implementation is regex-based
- Bundled TypeScript compiler (`tsc --noEmit`) — deferred; compile-time type checking is currently skipped
- A workflow registry, `cori init`, `cori save`, `cori start`, `cori ls`, `cori rm`, the `cori-default` task queue — removed in the Phase 1 strip and not coming back

---

## Coding conventions

- **Errors.** Library crates use `thiserror` enums with structured fields. The CLI uses `anyhow::Result` and converts library errors via `From`/`with_context`. Never `unwrap()` outside tests.
- **Diagnostics.** Compile errors carry `{ file, line, field, reason }` (see `CompileError`). User-facing CLI errors include actionable hints — every "missing capability" must say how to install it; every "needs sign-in" must say `cori login <id>`; every Temporal connection error must say to start `temporal server start-dev` or set `temporal.host`.
- **Async.** Only `cori-worker` (and the small async parts of `cori-cli`) is async. The broker is sync; bridge with `tokio::task::spawn_blocking` inside activities.
- **Serde naming.** `#[serde(rename_all = "snake_case")]` on enums; `snake_case` field names everywhere on the wire.
- **Tracing.** Use the `tracing` crate. The CLI initializes a subscriber driven by `RUST_LOG`.
- **Tests.** Prefer golden-file tests for the compiler (see [crates/cori-compiler/tests/](crates/cori-compiler/tests)). Integration tests that need Temporal must be `#[ignore]`-gated.
- **No `println!` from library crates.** Only the CLI prints. Library crates return data or `tracing::info!`.
- **Capability declarations are mandatory.** A workflow that uses CLI `gws` must declare `tools_required: [gws]`. The compiler enforces this; do not weaken the check — placement inference depends on it.
- **Identity tokens.** `user_id` / `pool` strings must pass `validate_identity_token` (lowercase alnum + `-_`). Queue names are derived only from validated identity, never from arbitrary user input.

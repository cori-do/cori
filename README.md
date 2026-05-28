<div align="center">
<img src="https://assets.cori.do/cori-logo.png" alt="Cori Logo" width="140" />

### Turn one-off agent conversations into deterministic, re-runnable workflows.


</div>


**You describe a task to an agent once. Cori captures it as a typed TypeScript
workflow you can run from your terminal — every time, the same way, with a
full trace of what happened.**




```bash
cori run ./translate_product_sheets_fr
cori run cori-do/workflows/code_only
```

> Status: `v0.2.3-dev` — APIs may change.

---

## Why Cori

- **No registry, no UI.** Workflows are folders. Run them by path or by git ref.
- **Deterministic.** The agent writes the workflow at design time. At runtime
  there's no LLM in the loop unless a step explicitly asks for one.
- **Typed end to end.** Steps are TypeScript with [Zod](https://zod.dev) schemas.
  Inputs and outputs are validated at every boundary.
- **Safe by default.** Every external call (CLI, HTTP, MCP, OAuth) goes through
  a broker. Credentials never touch workflow code.
- **Reliable.** Backed by [Temporal](https://temporal.io) — retries, resumes,
  and a JSON trace of every run.

---

## Install

```bash
curl -fsSL https://cli.cori.do/install.sh | bash
```

Or build from source:

```bash
cargo build --release --workspace
pnpm install && pnpm build
```

---

## Quickstart

Run the bundled demo — no credentials, no setup:

```bash
cori run examples/hello_world
```

That's it. Cori auto-spawns a local Temporal dev server the first time, compiles
the workflow, runs three steps, and writes a trace to `~/.cori/runs/`.

---

## The CLI

```text
cori run <path-or-ref>     Run a workflow
cori check <path-or-ref>   Validate without running
cori show <path-or-ref>    Inspect a workflow + recent runs
cori runs list|show        Browse run history
cori work                  Stay online as a worker
cori login <capability>    Sign into a CLI or OAuth provider
cori status                Show machine identity, workers, endpoint
cori config get|set        Edit ~/.cori/config.toml
```

Pass parameters inline:

```bash
cori run ./translate_product_sheets_fr sheet_id=abc123 locale=fr-CA
```

Pin a remote workflow by tag, range, or sha:

```bash
cori run github.com/acme/workflows/report@v1        # latest v1.x.y
cori run github.com/acme/workflows/report@v1.2.3    # exact
cori run github.com/acme/workflows/report@a1b2c3d   # immutable sha
```

---

## What a workflow looks like

A workflow is a folder:

```text
translate_product_sheets_fr/
├── manifest.md            # frontmatter + goal + verification
└── steps/
    ├── 01_read_source_rows.ts
    ├── 02_translate_rows.ts
    └── 03_write_results.ts
```

Each step is a typed function:

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

export default step.cli({
  description: "Fetch a random quote",
  input: z.object({}),
  output: z.object({ quote: z.string(), author: z.string() }),
  command: () => ["curl", "--silent", "https://zenquotes.io/api/random"],
  parse: (stdout) => {
    const [first] = JSON.parse(stdout);
    return { quote: first.q, author: first.a };
  },
});
```

Four step kinds, covering everything:

| Kind   | What it does                                  |
| ------ | --------------------------------------------- |
| `cli`  | Run a command-line tool                       |
| `code` | Run a sandboxed TypeScript function           |
| `mcp`  | Call a tool on an MCP server                  |
| `llm`  | Ask an LLM (only when you explicitly opt in)  |

See [examples/](examples/) for full working workflows, or [skills/cori_save_workflow/SKILL.md](skills/cori_save_workflow/SKILL.md)
to teach your agent how to author them. To install the skill into your agent, run
`npx skills add cori-do/cori`.

---

## Where things live

```text
~/.cori/
├── config.toml          # endpoint, API keys, trusted git hosts
├── cache/               # compiled workflows + remote checkouts
├── runs/<key>/*.json    # every run's full JSON trace
├── credentials/         # token metadata (real tokens → OS keychain)
└── cluster/             # presence info from `cori work`
```

No SQLite, no hidden state. Everything Cori knows is a file you can read.

---

## Multi-machine

Need a step to run on a machine with a specific CLI or credential? Start a
worker there:

```bash
# On the machine with the right tools / accounts
cori work --shared notion-pool

# On your laptop
cori run ./pull_notion_pages
```

Cori routes each step to the right machine via Temporal task queues derived
from authenticated identity. Cross-user dispatch is impossible by construction.

---

## Repo layout

```text
crates/        Rust workspace
  cori-cli         the `cori` binary
  cori-worker      Temporal worker (workflow + activities)
  cori-compiler    manifest + step parsing → compiled DAG
  cori-broker      trust boundary for all side effects
  cori-protocol    wire types
  cori-manifest    manifest YAML schema
packages/      pnpm workspace
  sdk              @cori/sdk — what step files import
  deno-runner      runtime for sandboxed `code` steps
skills/        agent skills for authoring workflows
examples/      reference workflows
```

---

## License

MIT

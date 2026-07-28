<div align="center">
<img src="https://assets.cori.do/cori-logo.png" alt="Cori Logo" width="140" />

### Turn one-off agent conversations into deterministic, re-runnable workflows.

[![Version](https://img.shields.io/badge/dynamic/toml?url=https://raw.githubusercontent.com/cori-do/cori/main/Cargo.toml&query=$.workspace.package.version&label=version&prefix=v&color=blue)](https://github.com/cori-do/cori/blob/main/Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

</div>


Describe a task to an agent once. Cori captures it as a typed TypeScript
workflow you can re-run from your terminal — same way every time, with a
full trace.

```bash
cori run ./translate_product_sheets_fr
cori run cori-do/workflows/code_only
```

---

## Why Cori

- **Folders, not a registry.** Run workflows by path or git ref.
- **No LLM at runtime.** The agent authors once; execution is deterministic.
- **Typed end to end.** TypeScript + [Zod](https://zod.dev) at every boundary.
- **Brokered side effects.** Credentials never touch workflow code.
- **Temporal-backed.** Retries, resumes, JSON trace per run — see [Temporal](https://temporal.io).

---

## Install & try it

```bash
curl -fsSL https://cli.cori.do/install.sh | bash
cori run cori-do/workflows/hello_world
```

First run auto-spawns a local Temporal dev server, compiles the workflow,
and writes a trace to `~/.cori/runs/`. No credentials needed.

<details>
<summary>Build from source</summary>

```bash
cargo build --release --workspace
pnpm install && pnpm build
```

</details>

---

## Documentation

Learn more at [docs.cori.do](https://docs.cori.do).

---

## The CLI

```text
cori run <path-or-ref>     Run a workflow
cori check <path-or-ref>   Validate without running
cori work                  Stay online as a worker
```

Full command reference at [docs.cori.do](https://docs.cori.do).

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
import { step } from "@cori-do/sdk";
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

See [examples/](examples/) for full working workflows, or [skills/cori-save-workflow/SKILL.md](skills/cori-save-workflow/SKILL.md)
to teach your agent how to author them. To install the skill into your agent, run
`npx skills add cori-do/cori`.

---

## On-disk layout

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

## Run steps on other machines

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

## License

MIT

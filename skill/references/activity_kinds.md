# Activity kinds — TypeScript templates and rules

Every step in a Cori workflow is exactly one of five activity kinds. This file is the reference for what each kind looks like, when to use it, and the TypeScript template the agent generates.

The Cori SDK exposes these primitives:

```ts
import { step } from "@cori/sdk";
```

`step` has five constructors — one per kind — plus shared options (description, retries, timeout). Every step file's default export is a `step.<kind>({ … })` call.

## Decision recap

| Action observed in the conversation | Kind |
|---|---|
| Successful MCP tool call | `mcp_tool` |
| Successful shell/CLI command | `cli` |
| Model call (translate, classify, summarize, extract) | `llm` |
| Pure data transform (parse, filter, format, validate, math) | `code` |
| Flow control (loop, branch, parallel, wait) | `builtin` |

A few non-negotiable rules across all kinds:

- One step per file. The file's default export is the step.
- Numeric prefix on the filename declares execution order: `01_*.ts`, `02_*.ts`, …
- Every step has typed input and output, expressed as zod schemas.
- Every step has a one-line `description` — this is what appears in the run trace.
- No external I/O in `code` steps. If a transform needs a network or filesystem call, it's not `code`.

---

## `cli` — invoke an installed CLI binary

Use when the conversation ran a shell command that succeeded. The worker executes the binary; Cori captures stdout, stderr, and exit code.

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({
  spreadsheet_id: z.string(),
  range: z.string(),
});

const Output = z.object({
  values: z.array(z.array(z.string())),
});

export default step.cli({
  description: "Read source rows from Google Sheets",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, range }) => [
    "gws", "sheets", "spreadsheets", "values", "get",
    "--params", JSON.stringify({ spreadsheetId: spreadsheet_id, range }),
    "--format", "json",
  ],
  parse: (stdout) => {
    const raw = JSON.parse(stdout);
    return { values: raw.values ?? [] };
  },
});
```

Key fields:

- **`command`** is a function from input → string array. Cori passes it to the OS as argv — no shell, no injection. Always return an array, never a single string.
- **`parse`** is required for non-trivial output. If the CLI returns plain text and the step's `Output` is `{ stdout: string }`, you can omit `parse` and Cori uses the raw stdout.
- **`env`** (optional) lets you inject env vars without leaking them into logs.
- **`timeout_ms`** (optional) default 60_000.

When the CLI doesn't exist on the worker, Cori fails the activity with a clear "binary not found" error before scheduling. Workflow registration also fails early if `tools_required` lists a binary the worker doesn't have — surface this with the suggestion `cori workers status`.

---

## `mcp_tool` — call a tool on a connected MCP server

Use when the conversation invoked an MCP tool (e.g. via a Claude Code MCP connection) and it succeeded. The worker calls the same tool on the same server.

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({
  channel: z.string(),
  text: z.string(),
});

const Output = z.object({
  ts: z.string(),
  channel: z.string(),
});

export default step.mcp_tool({
  description: "Post status update to Slack",
  server: "slack",
  tool: "chat_postMessage",
  input: Input,
  output: Output,
  args: ({ channel, text }) => ({ channel, text }),
});
```

Key fields:

- **`server`** is the registered MCP server name. The worker must have this server configured — verify with `cori workers status`.
- **`tool`** is the exact tool name as exposed by the MCP server.
- **`args`** is a function from input → the JSON object the tool expects.

Cori validates at register time that the server + tool pair is reachable from the worker. If not, registration fails with a clear message and a list of available servers.

---

## `code` — sandboxed TypeScript transform

Use for pure data transformation: parsing, filtering, formatting, validation, math. Runs in a Deno sandbox with no network and no filesystem access.

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({
  rows: z.array(z.object({
    sku: z.string(),
    description_fr: z.string(),
    safety_info_fr: z.string().nullable(),
    operator_contact: z.string().nullable(),
  })),
});

const Output = z.object({
  results: z.array(z.object({
    sku: z.string(),
    check: z.enum(["OK", "NOK"]),
    invalid_reason: z.string().nullable(),
  })),
});

export default step.code({
  description: "Strict GPSR compliance check on translated rows",
  input: Input,
  output: Output,
  run: ({ rows }) => {
    const results = rows.map((row) => {
      const missing: string[] = [];
      if (!row.operator_contact) missing.push("operator contact");
      if (!row.safety_info_fr) missing.push("French safety info");
      return missing.length === 0
        ? { sku: row.sku, check: "OK" as const, invalid_reason: null }
        : { sku: row.sku, check: "NOK" as const, invalid_reason: `Missing: ${missing.join(", ")}` };
    });
    return { results };
  },
});
```

Key fields:

- **`run`** is a pure function from input → output. No `fetch`, no `Deno.readFile`, no `process.env`. If you find yourself needing those, the step is `cli` or `mcp_tool`, not `code`.
- **`run` can be async** if the transform itself is async (e.g. parsing a large blob with a streaming parser), but the sandbox blocks all network/disk syscalls. The async-ness is for CPU-bound async libraries, not for I/O.

`code` steps are the easiest to test. Always generate a `tests/<step>.test.ts` alongside, using a captured fixture from the conversation. The tests are vitest-compatible — the user can run `npx vitest tests/` to verify before triggering the workflow.

---

## `llm` — a model call with a typed output schema

Use only when the *runtime* data genuinely needs a model. Translating new descriptions every day → `llm`. Re-deriving the workflow logic → no, that's what design-time was for.

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({
  rows: z.array(z.object({
    sku: z.string(),
    description_en: z.string(),
    material_en: z.string(),
  })),
});

const Output = z.object({
  translations: z.array(z.object({
    sku: z.string(),
    description_fr: z.string(),
    material_fr: z.string(),
  })),
});

export default step.llm({
  description: "Translate product rows EN → FR",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  batch: { size: 50, by: "rows" },
  prompt: ({ rows }) => `
You are translating e-commerce product copy from English to French.
Preserve product names exactly. Translate descriptions and material names naturally for French shoppers.
Return JSON matching the schema.

Rows:
${JSON.stringify(rows, null, 2)}
`.trim(),
});
```

Key fields:

- **`model`** is a model identifier. At runtime, Cori uses the org's configured provider for that model class. The first time an `llm` step runs without configured credentials, Cori prompts the user just-in-time.
- **`batch`** (optional) lets Cori batch the input list into chunks, parallelize the calls, and merge results. Use whenever the input is a list and items are independent.
- **`prompt`** returns a string. The output schema is enforced — Cori parses the model response against `Output` and fails the step if it doesn't match.

If you find yourself writing an `llm` step whose prompt is "decide what to do next based on these inputs", stop. That's design-time reasoning, not runtime data processing. The decision should be a `code` step with explicit logic, or a `builtin` branch.

---

## `builtin` — DAG flow control

Use for the glue logic between data-bearing steps: looping, branching, parallel fan-out, waiting.

The five most common builtins:

### `map` — transform a list by applying another step to each element

```ts
import { step } from "@cori/sdk";
import translateRow from "./02_translate_row";

export default step.map({
  description: "Translate every row in parallel",
  over: (input: { rows: Row[] }) => input.rows,
  apply: translateRow,
  concurrency: 10,
});
```

### `for_each` — sequential iteration with side effects between iterations

Use when iterations are not independent (e.g. each iteration appends to a state passed to the next).

### `branch` — conditional execution

```ts
import { step } from "@cori/sdk";
import nokStep from "./04_handle_nok";
import okStep from "./04_handle_ok";

export default step.branch({
  description: "Route based on GPSR check result",
  on: (input: { check: "OK" | "NOK" }) => input.check,
  cases: {
    OK: okStep,
    NOK: nokStep,
  },
});
```

### `parallel` — fan out to multiple independent steps, collect their results

### `wait` — pause until a condition is met (a webhook arrives, a time elapses, a signal is received)

```ts
export default step.wait({
  description: "Wait for human approval",
  for: { signal: "approved", timeout_ms: 86_400_000 },
});
```

`builtin` steps don't have I/O code — they're declarative. Cori's compiler turns them into the right Temporal primitives.

---

## Shared options across all kinds

Every `step.<kind>({…})` call accepts these in addition to the kind-specific fields:

- **`description`** (required) — one line, ≤80 chars, sentence case. Appears in the run trace.
- **`retries`** (optional) — `{ max: number; backoff: "exponential" | "linear" }`. Default `{ max: 3, backoff: "exponential" }`.
- **`timeout_ms`** (optional) — per-attempt timeout. Default varies by kind: 60s for `cli`/`mcp_tool`, 300s for `llm`, 30s for `code`.

---

## What good step files have in common

When reviewing your generated step files before showing them to the user, check:

- One step per file, file name matches `NN_snake_case.ts`
- Default export is `step.<kind>({…})`
- Input and output are zod schemas
- `description` is present and reads as a verb phrase ("Translate product rows EN → FR", not "Translation")
- For `code`: no `fetch`, no `Deno.readFile`, no `process`, no imports of `node:*` or network modules
- For `cli`: `command` returns an array, not a string
- For `mcp_tool`: `server` and `tool` match a real connected server's tool
- For `llm`: output schema is strict; no untyped string returns
- Imports from `@cori/sdk` are clean — no unused imports

If something looks off, fix it before disk write. A clean workflow on the first try buys enormous trust.

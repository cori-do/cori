# @cori-do/runner

The Deno script that hosts Cori `code` activities at runtime.

Current scope:

- Accepts a step file path on argv
- Reads `{ "input": <value> }` as a JSON object from stdin
- Dynamically imports the step file's default export
- Invokes the step's `run` function with the input
- Writes a single JSON envelope to stdout: `{ "ok": true, "output": <value> }`
  on success or `{ "ok": false, "error": { "message": ..., "stack": ... } }`
  on failure (also exits non-zero on failure)

The runner is invoked by the Cori worker via the `cori-broker` crate; users
never spawn it directly. It is intentionally minimal: every transformation of
the step's I/O happens here in TypeScript so the Rust side just exchanges
JSON.

## Module resolution

The runner ships next to a `deno.json` import map (installed to `~/.cori/runtime/`):

```json
{
  "imports": {
    "@cori-do/sdk": "./sdk/index.ts",
    "zod": "npm:zod@^4.4.3"
  }
}
```

This means user step files can `import { step } from "@cori-do/sdk"` and
`import { z } from "zod"` without any local `node_modules` setup.

## Permissions

The runner currently invokes Deno with `--allow-read` only (it has to read
the step file and the bundled SDK; user `code` steps are pure functions so
they need no other permissions). This can tighten to `--allow-none` once we
can pass the step source over stdin instead of by path.

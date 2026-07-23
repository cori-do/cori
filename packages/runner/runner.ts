// runner.ts — host process for Cori activities.
//
// Invoked as:
//
//   deno run [permission flags] --config <runtime>/deno.json \
//     <runtime>/runner.ts <step-file> <mode>
//
// Supported modes:
//
//   code         — call stepDef.run(input); return its output.
//   cli_command  — call stepDef.command(input); return { command, env }.
//   cli_parse    — call stepDef.parse(stdout, { stderr, exitCode }) or
//                  JSON.parse(stdout) when no parse fn is declared.
//   mcp_args     — call stepDef.args(input); return { server, tool, args }.
//   llm_prompt   — call stepDef.prompt(input); return
//                  { model, prompt, batch, outputSchema, hasOutputSchema }.
//
// Protocol (every mode):
//   stdin  : JSON object — `{ "input": <value>, ... mode-specific extras }`.
//            Empty stdin is treated as `{}`.
//   stdout : exactly one JSON envelope, written as the last line and prefixed
//            with ENVELOPE_PREFIX so the broker can tolerate stray user logs.
//   stderr : free-form Deno diagnostics; never parsed by the broker.
//
// Envelope shape (success):  { "ok": true, "output": <value> }
// Envelope shape (failure):  { "ok": false, "error": { "message", "stack"? } }

import { pathToFileURL } from "node:url";

const ENVELOPE_PREFIX = "\u001ECORI_RUNNER\u001E";

const stepPath = Deno.args[0];
const mode = Deno.args[1] ?? "code";

function emit(envelope: unknown): void {
  const line = ENVELOPE_PREFIX + JSON.stringify(envelope) + "\n";
  Deno.stdout.writeSync(new TextEncoder().encode(line));
}

function fail(message: string, err?: unknown): never {
  const stack = err instanceof Error ? err.stack : undefined;
  emit({ ok: false, error: { message, stack } });
  Deno.exit(1);
}

if (!stepPath) {
  fail("missing step file path (argv[0])");
}

async function readStdin(): Promise<string> {
  const chunks: Uint8Array[] = [];
  for await (const c of Deno.stdin.readable) chunks.push(c);
  let total = 0;
  for (const c of chunks) total += c.length;
  const buf = new Uint8Array(total);
  let o = 0;
  for (const c of chunks) {
    buf.set(c, o);
    o += c.length;
  }
  return new TextDecoder().decode(buf);
}

const stdinText = await readStdin();
let payload: Record<string, unknown>;
try {
  payload = stdinText.trim() ? JSON.parse(stdinText) : {};
} catch (e) {
  fail("could not parse stdin as JSON", e);
}

const url = stepPath.startsWith("file:")
  ? new URL(stepPath)
  // Rust canonicalisation on Windows may produce an extended-length path
  // (`\\?\C:\…`). Concatenating `file://` turns that into `file:///?\C:\…`,
  // which Deno interprets as a directory URL. `pathToFileURL` handles that
  // Windows form (and normal paths) correctly.
  : pathToFileURL(stepPath);

let mod: { default?: unknown };
try {
  mod = await import(url.href);
} catch (e) {
  fail(
    `could not import step file (${stepPath}): ${
      e instanceof Error ? e.message : String(e)
    }`,
    e,
  );
}

// deno-lint-ignore no-explicit-any
const stepDef: any = (mod as { default?: unknown }).default;
if (!stepDef || stepDef.__cori_step !== true) {
  fail(
    `step file's default export is not a Cori step (expected an object with __cori_step === true; got ${typeof stepDef})`,
  );
}

function expectKind(want: string): void {
  if (stepDef.kind !== want) {
    fail(
      `step kind mismatch: runner invoked in mode '${mode}' expects '${want}' but step declares '${stepDef.kind}'`,
    );
  }
}

/**
 * Enforce the step's declared zod `output` schema at runtime. "Typed
 * workflows" must be true when it matters — a step whose output drifts
 * from its declared shape should fail HERE with a nameable reason, not
 * poison the next step with malformed input. (LLM steps already get
 * this via provider-side schema enforcement; this closes the gap for
 * `code` and `cli` outputs.) Best-effort on the schema side: if the
 * declared output isn't a zod schema with safeParse, pass through.
 */
// deno-lint-ignore no-explicit-any
function validateOutput(value: unknown): unknown {
  const schema: any = stepDef.output;
  if (!schema || typeof schema.safeParse !== "function") {
    return value;
  }
  const result = schema.safeParse(value);
  if (!result.success) {
    const issues = (result.error?.issues ?? [])
      .slice(0, 5)
      // deno-lint-ignore no-explicit-any
      .map((i: any) => `${(i.path ?? []).join(".") || "(root)"}: ${i.message}`)
      .join("; ");
    fail(
      `step output does not match its declared output schema — ${issues}. ` +
        `The upstream data shape probably drifted; fix the step (or the schema) ` +
        `so the contract stays true.`,
    );
  }
  return result.data;
}

try {
  switch (mode) {
    case "code": {
      expectKind("code");
      if (typeof stepDef.run !== "function") {
        fail("code step is missing a `run` function");
      }
      const out = await stepDef.run(payload.input);
      emit({ ok: true, output: validateOutput(out ?? null) });
      break;
    }

    case "cli_command": {
      expectKind("cli");
      if (typeof stepDef.command !== "function") {
        fail("cli step is missing a `command` function");
      }
      const argv = await stepDef.command(payload.input);
      if (!Array.isArray(argv) || argv.some((x) => typeof x !== "string")) {
        fail(
          "cli step `command` must return an array of strings (no single string, no nested arrays)",
        );
      }
      if (argv.length === 0) {
        fail("cli step `command` returned an empty array");
      }
      emit({
        ok: true,
        output: {
          command: argv,
          env: stepDef.env ?? null,
        },
      });
      break;
    }

    case "cli_parse": {
      expectKind("cli");
      // deno-lint-ignore no-explicit-any
      const ctx: any = payload.parseCtx ?? {};
      const stdout = String(ctx.stdout ?? "");
      const stderr = String(ctx.stderr ?? "");
      const exitCode = Number(ctx.exitCode ?? 0);
      let parsed: unknown;
      if (typeof stepDef.parse === "function") {
        parsed = await stepDef.parse(stdout, { stderr, exitCode });
      } else {
        const trimmed = stdout.trim();
        parsed = trimmed.length ? JSON.parse(trimmed) : null;
      }
      emit({ ok: true, output: validateOutput(parsed ?? null) });
      break;
    }

    case "mcp_args": {
      expectKind("mcp_tool");
      if (typeof stepDef.args !== "function") {
        fail("mcp_tool step is missing an `args` function");
      }
      const args = await stepDef.args(payload.input);
      emit({
        ok: true,
        output: {
          server: stepDef.server,
          tool: stepDef.tool,
          args: args ?? {},
        },
      });
      break;
    }

    case "llm_prompt": {
      expectKind("llm");
      if (typeof stepDef.prompt !== "function") {
        fail("llm step is missing a `prompt` function");
      }
      const prompt = await stepDef.prompt(payload.input);
      const schema = stepDef.output;
      const hasSchema = !!(schema && typeof schema === "object" && schema._def);
      emit({
        ok: true,
        output: {
          model: stepDef.model,
          prompt: String(prompt ?? ""),
          batch: stepDef.batch ?? null,
          outputSchema: hasSchema ? jsonSchemaFromZod(schema) : null,
          hasOutputSchema: hasSchema,
        },
      });
      break;
    }

    case "llm_stub": {
      expectKind("llm");
      // Retained for backward compatibility; the live provider path uses
      // `llm_prompt`, but the stub is still useful for `--dry-run`.
      const schema = stepDef.output;
      const stub = schema && typeof schema === "object" && schema._def
        ? defaultFromZod(schema)
        : { mocked: true };
      emit({ ok: true, output: stub });
      break;
    }

    default:
      fail(`unknown runner mode: '${mode}'`);
  }
} catch (e) {
  fail(
    `runner failed in mode '${mode}': ${
      e instanceof Error ? e.message : String(e)
    }`,
    e,
  );
}

// ---------------------------------------------------------------------------
// zod default helper
// ---------------------------------------------------------------------------
//
// We do not import zod here (the runner runs against whatever zod the
// step file pulls in). We introspect `_def.typeName`, which is stable
// across zod 3.x. Returns a minimal value: empty arrays/strings/objects,
// `null` for nullable/unknown, the declared `.default()` if present.

// deno-lint-ignore no-explicit-any
function defaultFromZod(schema: any): unknown {
  if (!schema || typeof schema !== "object") {
    return null;
  }
  // Zod 4 reorganised internals: `_def.typeName` is gone, replaced by
  // `_zod.def.type`. Dispatch to the v4 walker when those internals exist.
  if (schema._zod?.def) {
    return defaultFromZodV4(schema);
  }
  if (!schema._def) {
    return null;
  }
  const def = schema._def;
  if (def.typeName === "ZodDefault") {
    try {
      return def.defaultValue();
    } catch {
      return defaultFromZod(def.innerType);
    }
  }
  switch (def.typeName) {
    case "ZodString":
      return "";
    case "ZodNumber":
    case "ZodBigInt":
      return 0;
    case "ZodBoolean":
      return false;
    case "ZodDate":
      return new Date(0).toISOString();
    case "ZodNull":
    case "ZodNullable":
      return null;
    case "ZodUndefined":
    case "ZodVoid":
    case "ZodAny":
    case "ZodUnknown":
      return null;
    case "ZodOptional":
      return defaultFromZod(def.innerType);
    case "ZodArray":
      return [];
    case "ZodTuple":
      return (def.items ?? []).map((s: unknown) => defaultFromZod(s));
    case "ZodEnum":
      return def.values?.[0] ?? null;
    case "ZodNativeEnum": {
      const v = def.values && Object.values(def.values)[0];
      return v ?? null;
    }
    case "ZodLiteral":
      return def.value ?? null;
    case "ZodUnion":
    case "ZodDiscriminatedUnion": {
      const opts = def.options ?? [];
      return opts.length ? defaultFromZod(opts[0]) : null;
    }
    case "ZodIntersection":
      return {
        ...((defaultFromZod(def.left) as object) ?? {}),
        ...((defaultFromZod(def.right) as object) ?? {}),
      };
    case "ZodRecord":
    case "ZodMap":
      return {};
    case "ZodObject": {
      const shape = typeof def.shape === "function" ? def.shape() : def.shape;
      const out: Record<string, unknown> = {};
      for (const k of Object.keys(shape ?? {})) {
        out[k] = defaultFromZod(shape[k]);
      }
      return out;
    }
    case "ZodEffects":
    case "ZodBranded":
    case "ZodCatch":
    case "ZodPipeline":
    case "ZodLazy":
      return defaultFromZod(def.schema ?? def.innerType ?? def.in);
    default:
      return null;
  }
}

// Zod 4 variant of `defaultFromZod`. Walks `_zod.def`, whose `type` field
// replaces v3's `_def.typeName`.
// deno-lint-ignore no-explicit-any
function defaultFromZodV4(schema: any): unknown {
  const def = schema._zod?.def;
  if (!def) return null;
  switch (def.type) {
    case "string":
      return "";
    case "number":
    case "int":
    case "bigint":
      return 0;
    case "boolean":
      return false;
    case "date":
      return new Date(0).toISOString();
    case "null":
    case "nullable":
      return null;
    case "undefined":
    case "void":
    case "any":
    case "unknown":
    case "never":
    case "nan":
      return null;
    case "default":
    case "prefault":
      try {
        return typeof def.defaultValue === "function"
          ? def.defaultValue()
          : def.defaultValue;
      } catch {
        return defaultFromZodV4(def.innerType);
      }
    case "optional":
    case "nonoptional":
    case "readonly":
    case "catch":
      return defaultFromZodV4(def.innerType);
    case "array":
      return [];
    case "tuple":
      return (def.items ?? []).map((s: unknown) => defaultFromZodV4(s));
    case "enum": {
      const vals = Object.values(def.entries ?? {});
      return vals.length ? vals[0] : null;
    }
    case "literal":
      return (def.values ?? [])[0] ?? null;
    case "union": {
      const opts = def.options ?? [];
      return opts.length ? defaultFromZodV4(opts[0]) : null;
    }
    case "intersection":
      return {
        ...((defaultFromZodV4(def.left) as object) ?? {}),
        ...((defaultFromZodV4(def.right) as object) ?? {}),
      };
    case "record":
    case "map":
    case "set":
      return def.type === "set" ? [] : {};
    case "pipe":
      return defaultFromZodV4(def.out ?? def.in);
    case "lazy":
      try {
        return defaultFromZodV4(def.getter ? def.getter() : null);
      } catch {
        return null;
      }
    case "object": {
      const shape = typeof def.shape === "function" ? def.shape() : def.shape;
      const out: Record<string, unknown> = {};
      for (const k of Object.keys(shape ?? {})) {
        out[k] = defaultFromZodV4(shape[k]);
      }
      return out;
    }
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// zod -> JSON Schema converter (minimal)
// ---------------------------------------------------------------------------
//
// Used by `llm_prompt` mode to materialise a JSON Schema describing the
// step's `output` zod schema, which LLM providers can enforce via
// structured-output / response_format APIs. Mirrors `defaultFromZod`'s
// strategy: introspect `_def.typeName`, walk the tree, emit Draft-07-
// compatible JSON Schema with `additionalProperties: false` everywhere
// (which is what OpenAI's structured outputs require).

// deno-lint-ignore no-explicit-any
function jsonSchemaFromZod(schema: any): Record<string, unknown> {
  if (!schema || typeof schema !== "object") {
    return {};
  }
  // Zod 4 internals live under `_zod.def` (with `_def.typeName` removed).
  if (schema._zod?.def) {
    return jsonSchemaFromZodV4(schema);
  }
  if (!schema._def) {
    return {};
  }
  const def = schema._def;
  switch (def.typeName) {
    case "ZodString":
      return { type: "string" };
    case "ZodNumber":
      return { type: "number" };
    case "ZodBigInt":
      return { type: "integer" };
    case "ZodBoolean":
      return { type: "boolean" };
    case "ZodDate":
      return { type: "string", format: "date-time" };
    case "ZodNull":
      return { type: "null" };
    case "ZodAny":
    case "ZodUnknown":
      return {};
    case "ZodUndefined":
    case "ZodVoid":
      return { type: "null" };
    case "ZodLiteral":
      return { const: def.value };
    case "ZodEnum":
      return { type: "string", enum: def.values ?? [] };
    case "ZodNativeEnum":
      return { enum: Object.values(def.values ?? {}) };
    case "ZodOptional":
    case "ZodNullable":
    case "ZodDefault":
    case "ZodCatch":
    case "ZodBranded":
    case "ZodReadonly":
      return jsonSchemaFromZod(def.innerType);
    case "ZodEffects":
      return jsonSchemaFromZod(def.schema);
    case "ZodPipeline":
      return jsonSchemaFromZod(def.out ?? def.in);
    case "ZodLazy":
      try {
        return jsonSchemaFromZod(def.getter ? def.getter() : null);
      } catch {
        return {};
      }
    case "ZodArray":
      return { type: "array", items: jsonSchemaFromZod(def.type) };
    case "ZodTuple":
      return {
        type: "array",
        items: (def.items ?? []).map((s: unknown) => jsonSchemaFromZod(s)),
        minItems: (def.items ?? []).length,
        maxItems: (def.items ?? []).length,
      };
    case "ZodUnion":
    case "ZodDiscriminatedUnion":
      return {
        anyOf: (def.options ?? []).map((s: unknown) => jsonSchemaFromZod(s)),
      };
    case "ZodIntersection":
      return {
        allOf: [
          jsonSchemaFromZod(def.left),
          jsonSchemaFromZod(def.right),
        ],
      };
    case "ZodRecord":
      return {
        type: "object",
        additionalProperties: jsonSchemaFromZod(def.valueType),
      };
    case "ZodMap":
      return { type: "object" };
    case "ZodObject": {
      const shape = typeof def.shape === "function" ? def.shape() : def.shape;
      const properties: Record<string, unknown> = {};
      const required: string[] = [];
      for (const k of Object.keys(shape ?? {})) {
        const child = shape[k];
        properties[k] = jsonSchemaFromZod(child);
        // A field is optional only if its outermost wrapper is
        // ZodOptional / ZodDefault.
        const inner = child?._def?.typeName;
        if (inner !== "ZodOptional" && inner !== "ZodDefault") {
          required.push(k);
        }
      }
      const out: Record<string, unknown> = {
        type: "object",
        properties,
        additionalProperties: false,
      };
      if (required.length) out.required = required;
      return out;
    }
    default:
      return {};
  }
}

// Zod 4 variant of `jsonSchemaFromZod`. Walks `_zod.def` and emits the same
// Draft-07-compatible shape (objects carry `additionalProperties: false`).
// deno-lint-ignore no-explicit-any
function jsonSchemaFromZodV4(schema: any): Record<string, unknown> {
  const def = schema._zod?.def;
  if (!def) return {};
  switch (def.type) {
    case "string":
      return { type: "string" };
    case "number":
      return { type: "number" };
    case "int":
    case "bigint":
      return { type: "integer" };
    case "boolean":
      return { type: "boolean" };
    case "date":
      return { type: "string", format: "date-time" };
    case "null":
      return { type: "null" };
    case "any":
    case "unknown":
      return {};
    case "undefined":
    case "void":
      return { type: "null" };
    case "literal": {
      const vals = def.values ?? [];
      return vals.length === 1 ? { const: vals[0] } : { enum: vals };
    }
    case "enum":
      return { type: "string", enum: Object.values(def.entries ?? {}) };
    case "optional":
    case "nullable":
    case "default":
    case "prefault":
    case "catch":
    case "nonoptional":
    case "readonly":
      return jsonSchemaFromZodV4(def.innerType);
    case "pipe":
      return jsonSchemaFromZodV4(def.out ?? def.in);
    case "lazy":
      try {
        return jsonSchemaFromZodV4(def.getter ? def.getter() : null);
      } catch {
        return {};
      }
    case "array":
      return { type: "array", items: jsonSchemaFromZodV4(def.element) };
    case "tuple": {
      const items = (def.items ?? []).map((s: unknown) =>
        jsonSchemaFromZodV4(s)
      );
      const out: Record<string, unknown> = {
        type: "array",
        items,
        minItems: items.length,
      };
      if (!def.rest) out.maxItems = items.length;
      return out;
    }
    case "union":
      return {
        anyOf: (def.options ?? []).map((s: unknown) => jsonSchemaFromZodV4(s)),
      };
    case "intersection":
      return {
        allOf: [
          jsonSchemaFromZodV4(def.left),
          jsonSchemaFromZodV4(def.right),
        ],
      };
    case "record":
    case "map":
      return {
        type: "object",
        additionalProperties: def.valueType
          ? jsonSchemaFromZodV4(def.valueType)
          : true,
      };
    case "object": {
      const shape = typeof def.shape === "function" ? def.shape() : def.shape;
      const properties: Record<string, unknown> = {};
      const required: string[] = [];
      for (const k of Object.keys(shape ?? {})) {
        const child = shape[k];
        properties[k] = jsonSchemaFromZodV4(child);
        const inner = child?._zod?.def?.type;
        if (inner !== "optional" && inner !== "default") {
          required.push(k);
        }
      }
      const out: Record<string, unknown> = {
        type: "object",
        properties,
        additionalProperties: false,
      };
      if (required.length) out.required = required;
      return out;
    }
    default:
      return {};
  }
}

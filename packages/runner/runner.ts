// SPDX-License-Identifier: Apache-2.0
// Cori runner v2 — long-lived, sandboxed evaluator for step files.
//
// Protocol: JSON lines on stdin/stdout. Request: { id, op, file, name?, ...payload }.
// Response: { id, ok: true, ...result } | { id, ok: false, error, type }.
// The process is started with Deno permissions limited to reading the workflow
// folder and the SDK; no network, no env, no write, no remote imports. Modules are cached per file.
// The first line on stdin is { nonce }: every response line starts with it.
import { parseWithSchema, toJsonSchema } from "./schema.ts";

type Any = any;
const modules = new Map<string, Any>();

async function loadStep(file: string, name?: string): Promise<Any> {
  let mod = modules.get(file);
  if (!mod) { mod = (await import("file://" + file)).default; modules.set(file, mod); }
  if (!mod || mod.__cori_step !== true) throw new RunnerError("StepFailedError", `${file}: default export is not a step`);
  if (!name || mod.name === name) return mod;
  const found = findNested(mod, name);
  if (!found) throw new RunnerError("StepFailedError", `${file}: no nested step named '${name}'`);
  return found;
}

function findNested(step: Any, name: string): Any {
  for (const slot of ["then", "else", "each", "body", "on_error", "default"]) {
    const v = step[slot];
    if (v && v.__cori_step === true) { if (v.name === name) return v; }
  }
  if (step.cases) for (const k of Object.keys(step.cases)) { const v = step.cases[k]; if (v && v.__cori_step === true && v.name === name) return v; }
  return undefined;
}

class RunnerError extends Error { constructor(public type: string, message: string) { super(message); } }

/** The files an llm step gives its model (`files(input)`): references only, each with its id; none without `files`. */
function filesOf(step: Any, input: unknown): Any[] {
  if (typeof step.files !== "function") return [];
  const v = step.files(input);
  if (v === undefined || v === null) return [];
  const list = Array.isArray(v) ? v : [v];
  for (const f of list) {
    if (!f || typeof f !== "object" || typeof f.id !== "string") throw new RunnerError("InvalidInputError", `step '${step.name}': files() must give files (a parameter of type file or files), got ${JSON.stringify(f)}`);
  }
  return list.map((f: Any) => ({ id: f.id, name: f.name, size: f.size, content_type: f.content_type, sha256: f.sha256 }));
}

async function handle(req: Any): Promise<Any> {
  const step = await loadStep(req.file, req.name);
  switch (req.op) {
    case "bind": {
      if (typeof step.input !== "function") return { input: {} };
      return { input: await step.input(req.ctx) };
    }
    case "select": {
      const fn = step[req.selector];
      if (typeof fn !== "function") throw new RunnerError("StepFailedError", `step '${step.name}' has no '${req.selector}' selector`);
      return { value: await fn(req.ctx) };
    }
    case "code": {
      const out = await step.run(req.input);
      return { output: await parseWithSchema(step.output, out, "output") };
    }
    case "prompt": {
      const prompt = step.prompt(req.input);
      const schema = step.output ? toJsonSchema(step.output) : undefined;
      const examples = step.examples ?? [];
      return { prompt, schema, examples, tier: step.tier ?? "medium", fallback: step.fallback ?? "fail", batch: step.batch, files: filesOf(step, req.input) };
    }
    case "request": return { request: step.request(req.input) };
    case "command": return { argv: step.command(req.input) };
    case "query": {
      const q = typeof step.query === "function" ? step.query(req.input) : step.query;
      return { query: q };
    }
    case "parse": {
      let value: unknown;
      if (typeof step.parse === "function") {
        switch (req.kind) {
          case "http": value = await step.parse(req.raw.res, req.input); break;
          case "cli": value = await step.parse(req.raw.stdout, { stderr: req.raw.stderr, exitCode: req.raw.exitCode }, req.input); break;
          case "store_query": value = await step.parse(req.raw.rows, req.input); break;
          default: value = await step.parse(req.raw.result, req.input);
        }
      } else {
        switch (req.kind) {
          case "http": value = req.raw.res.json; break;
          case "cli": { try { value = JSON.parse(req.raw.stdout); } catch { value = { stdout: req.raw.stdout }; } break; }
          case "store_query": value = { rows: req.raw.rows }; break;
          default: value = req.raw.result;
        }
      }
      return { output: await parseWithSchema(step.output, value, "output") };
    }
    case "validate": return { output: await parseWithSchema(step.output, req.value, "output") };
    case "defaults": return { fallback: step.fallback, on_timeout: step.on_timeout, timeout: step.timeout, to: step.to, kind: step.kind, max_concurrency: step.max_concurrency, on_item_error: step.on_item_error, max_iterations: step.max_iterations };
    case "ping": return { pong: true };
    default: throw new RunnerError("StepFailedError", `unknown op '${req.op}'`);
  }
}

// Responses go to stdout as `<nonce> <json>`: the nonce is the first line the engine sends, read before any step
// is loaded, and kept in this module, so a step that prints to stdout cannot pass for the runner. The primitives
// used to answer are captured now, before step code can replace them; console output goes to stderr.
const stdout = Deno.stdout;
const writeOut = stdout.write.bind(stdout);
const encoder = new TextEncoder();
const encode = encoder.encode.bind(encoder);
const stringify = JSON.stringify;
const toStderr = console.error.bind(console);
console.log = toStderr; console.info = toStderr; console.debug = toStderr; console.warn = toStderr;
let nonce = "";
async function write(obj: Any) {
  const bytes = encode(nonce + " " + stringify(obj) + "\n");
  let off = 0;
  while (off < bytes.length) off += await writeOut(bytes.subarray(off));
}

const dec = new TextDecoder();
let buf = "";
for await (const chunk of Deno.stdin.readable) {
  buf += dec.decode(chunk);
  let nl: number;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl); buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    if (!nonce) { try { nonce = String(JSON.parse(line).nonce ?? ""); } catch { nonce = ""; } if (nonce) continue; }
    let req: Any;
    try { req = JSON.parse(line); } catch (e) { await write({ id: null, ok: false, type: "InvalidInputError", error: `bad request: ${e}` }); continue; }
    try {
      const result = await handle(req);
      await write({ id: req.id, ok: true, ...result });
    } catch (e: Any) {
      const type = e?.name === "SchemaValidationError" ? "SchemaValidationError" : (e?.type ?? "StepFailedError");
      await write({ id: req.id, ok: false, type, error: e?.message ?? String(e) });
    }
  }
}

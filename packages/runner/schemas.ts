// SPDX-License-Identifier: Apache-2.0
// Dumps the JSON schema of every step's `output` (and nested steps) for a workflow folder.
// Usage: deno run --allow-read schemas.ts <workflow_dir> <file1.ts> <file2.ts> ...
//        deno run --allow-read schemas.ts --workflows '[["<workflow_dir>", ["<file1.ts>", ...]], ...]'
//        (several workflows of one app: a JSON array with one object per workflow, in order)
import { toJsonSchema } from "./schema.ts";
type Any = any;
async function schemasOf(dir: string, files: string[]): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  function walk(step: Any) {
    if (!step || step.__cori_step !== true) return;
    if (step.output && typeof step.output.safeParse === "function") out[step.name] = toJsonSchema(step.output);
    for (const slot of ["then", "else", "each", "body", "on_error", "default"]) walk(step[slot]);
    if (step.cases) for (const k of Object.keys(step.cases)) walk(step.cases[k]);
  }
  for (const f of files) { const mod = await import("file://" + dir + "/steps/" + f); walk(mod.default); }
  return out;
}
if (Deno.args[0] === "--workflows") {
  const each: [string, string[]][] = JSON.parse(Deno.args[1]);
  const all: Record<string, unknown>[] = [];
  for (const [dir, files] of each) all.push(await schemasOf(dir, files));
  console.log(JSON.stringify(all));
} else {
  const [dir, ...files] = Deno.args;
  console.log(JSON.stringify(await schemasOf(dir, files)));
}

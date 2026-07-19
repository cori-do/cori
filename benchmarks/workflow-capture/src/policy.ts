import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

import type { WorkflowPolicyReport } from "./types.js";

export async function hashDirectory(directory: string): Promise<string> {
  const files = await listFiles(directory);
  const hash = createHash("sha256");
  for (const file of files) {
    hash.update(relative(directory, file));
    hash.update("\0");
    hash.update(await readFile(file));
    hash.update("\0");
  }
  return hash.digest("hex");
}

/**
 * Benchmark-specific static rules. Cori's own compiler remains the source of
 * truth for step parsing; this layer expresses gates that are intentionally
 * stricter for published benchmark submissions.
 */
export async function inspectWorkflowPolicy(
  workflowDir: string,
  forbiddenRuntimeLiterals: readonly string[] = [],
  expectedParameters?: readonly string[],
): Promise<WorkflowPolicyReport> {
  const violations: string[] = [];
  const files = await listFiles(workflowDir);
  const manifestPath = join(workflowDir, "manifest.md");
  if (!files.includes(manifestPath)) {
    violations.push("missing manifest.md");
  } else {
    const manifest = await readFile(manifestPath, "utf8");
    if (!/^tools_required:\s*\[\s*gws\s*\]\s*$/mu.test(manifest)) {
      violations.push("manifest must declare exactly tools_required: [gws]");
    }
    if (/\b(?:secret|api[_-]?key|token)\s*:/iu.test(manifest)) {
      violations.push("manifest appears to contain a credential field");
    }
    for (const literal of forbiddenRuntimeLiterals.filter(Boolean)) {
      if (manifest.includes(literal)) violations.push(`manifest hard-codes captured fixture value ${redactedLiteral(literal)}`);
    }
    if (expectedParameters) {
      const declared = [...manifest.matchAll(/^\s*-\s+name:\s*([a-z][a-z0-9_]*)\s*$/gmu)].map((match) => match[1]!);
      const missing = expectedParameters.filter((name) => !declared.includes(name));
      const extra = declared.filter((name) => !expectedParameters.includes(name));
      if (missing.length > 0 || extra.length > 0) {
        violations.push(`manifest parameters must exactly match the task contract (missing: ${missing.join(", ") || "none"}; extra: ${extra.join(", ") || "none"})`);
      }
    }
  }

  const stepFiles = files.filter((file) => /\/steps\/\d\d_[a-z0-9_]+\.ts$/u.test(file));
  if (stepFiles.length === 0) violations.push("workflow has no numbered TypeScript steps");
  for (const file of stepFiles) {
    const source = await readFile(file, "utf8");
    if (!hasCanonicalSdkImport(source)) {
      violations.push(`${relative(workflowDir, file)} must import step from @cori-do/sdk`);
    }
    if (/step\.(?:map|for_each|branch|parallel|wait)\s*\(/u.test(source)) {
      violations.push(`${relative(workflowDir, file)} uses a deferred v1 builtin`);
    }
    if (/\b(?:bash|sh|zsh|env|xargs)\b/u.test(source)) {
      violations.push(`${relative(workflowDir, file)} uses a shell dispatcher`);
    }
    if (source.includes("step.cli") && !hasLiteralGwsArgv(source)) {
      violations.push(`${relative(workflowDir, file)} has a CLI step without literal gws argv[0]`);
    }
    for (const flag of literalLongFlags(source)) {
      if (!GWS_FLAGS.has(flag)) violations.push(`${relative(workflowDir, file)} uses unsupported gws flag ${flag}`);
    }
    for (const property of unsupportedParseContextProperties(source)) {
      violations.push(`${relative(workflowDir, file)} reads workflow input property ${property} from CLI parse context; parse receives only stderr and exitCode metadata`);
    }
    if (/userEnteredValue\s*:\s*null\b/u.test(source)) {
      violations.push(`${relative(workflowDir, file)} uses invalid Sheets CellData userEnteredValue: null; use values.clear or an ExtendedValue object`);
    }
    if (/\b(?:process\.env|Deno\.env|getenv)\b/u.test(source)) {
      violations.push(`${relative(workflowDir, file)} reads environment state directly`);
    }
    for (const literal of forbiddenRuntimeLiterals.filter(Boolean)) {
      if (source.includes(literal)) violations.push(`${relative(workflowDir, file)} hard-codes captured fixture value ${redactedLiteral(literal)}`);
    }
  }
  return { ok: violations.length === 0, violations, workflowHash: await hashDirectory(workflowDir) };
}

function redactedLiteral(value: string): string {
  return value.length <= 8 ? JSON.stringify(value) : `${JSON.stringify(value.slice(0, 6))}…`;
}

const GWS_FLAGS = new Set([
  "--params", "--json", "--upload", "--upload-content-type", "--output", "--format", "--api-version",
  "--page-all", "--page-limit", "--page-delay", "--sanitize", "--dry-run", "--help", "--resolve-refs",
]);

function literalLongFlags(source: string): readonly string[] {
  return [...source.matchAll(/["'](--[a-z][a-z0-9-]*)["']/giu)].map((match) => match[1]!);
}

function unsupportedParseContextProperties(source: string): readonly string[] {
  const signature = /parse\s*:\s*\(\s*[^,()]+\s*,\s*([A-Za-z_$][\w$]*)\s*\)\s*=>/u.exec(source);
  if (!signature?.[1]) return [];
  const name = signature[1].replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const member = new RegExp(`\\b${name}\\.([A-Za-z_$][\\w$]*)`, "gu");
  return [...source.matchAll(member)].map((match) => match[1]!).filter((property) => property !== "stderr" && property !== "exitCode");
}

function hasCanonicalSdkImport(source: string): boolean {
  return /^\s*import\s*\{[^}]*\bstep\b[^}]*\}\s*from\s*["']@cori-do\/sdk["']\s*;?/mu.test(source);
}

function hasLiteralGwsArgv(source: string): boolean {
  return /command\s*:\s*(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>\s*(?:\{[\s\S]*?return\s*)?\[\s*["']gws["']/u.test(source);
}

async function listFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.sort((a, b) => a.name.localeCompare(b.name)).map(async (entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  }));
  return nested.flat();
}

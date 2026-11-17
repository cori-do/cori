// SPDX-License-Identifier: Apache-2.0
// Zod 4 helpers. The runner imports the same `zod` specifier as the steps (one instance per process),
// so `z.toJSONSchema` accepts the step's schema objects directly.
import { z } from "zod";
type Any = any;

export class SchemaValidationError extends Error {
  constructor(scope: string, issues: Any[]) {
    super(`${scope} schema validation failed:\n` + issues.map((i) => `- ${(i.path ?? []).join(".") || scope}: ${i.message}`).join("\n"));
    this.name = "SchemaValidationError";
  }
}

export async function parseWithSchema(schema: Any, value: unknown, scope: string): Promise<unknown> {
  if (!schema || typeof schema.safeParse !== "function") return value;
  const r = typeof schema.safeParseAsync === "function" ? await schema.safeParseAsync(value) : schema.safeParse(value);
  if (r.success) return r.data;
  throw new SchemaValidationError(scope, r.error?.issues ?? []);
}

export function toJsonSchema(schema: Any): unknown {
  try { return z.toJSONSchema(schema, { unrepresentable: "any" }); } catch (_e) { return undefined; }
}

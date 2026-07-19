import assert from "node:assert/strict";
import { z } from "zod";

import {
  jsonSchemaFromZod,
  parseWithSchema,
  SchemaValidationError,
  stubFromZod,
} from "./schema.ts";

Deno.test("missing input fields produce path-aware diagnostics", async () => {
  const schema = z.object({
    messages: z.array(z.object({ id: z.string() })).min(1),
  });
  await assert.rejects(
    () => parseWithSchema(schema, { messages: [{}] }, "input"),
    (error: unknown) => {
      assert.ok(error instanceof SchemaValidationError);
      assert.match(error.message, /input\.messages\[0\]\.id/u);
      return true;
    },
  );
});

Deno.test("output validation reports the failing output path", async () => {
  const schema = z.object({ rows: z.array(z.object({ count: z.number() })) });
  await assert.rejects(
    () => parseWithSchema(schema, { rows: [{ count: "one" }] }, "output"),
    /output\.rows\[0\]\.count/u,
  );
});

Deno.test("parsing applies defaults, transforms, and unknown-field policy by scope", async () => {
  const schema = z.object({
    name: z.string().transform((value) => value.toUpperCase()),
    enabled: z.boolean().default(true),
    nested: z.object({ value: z.number() }).passthrough(),
  });
  const parsed = await parseWithSchema(
    schema,
    {
      name: "cori",
      ignored_at_root: true,
      nested: { value: 3, retained_by_nested_schema: true },
    },
    "input",
  );
  assert.deepEqual(parsed, {
    name: "CORI",
    enabled: true,
    nested: { value: 3, retained_by_nested_schema: true },
  });
});

Deno.test("omitted schemas retain backward-compatible pass-through values", async () => {
  const value = { legacy: true, nested: { untouched: true } };
  assert.equal(await parseWithSchema(undefined, value, "input"), value);
  assert.equal(await parseWithSchema(undefined, value, "output"), value);
});

Deno.test("LLM stubs honor exact and minimum array cardinality", async () => {
  const schema = z.object({
    exact: z.array(z.literal("classified")).length(3),
    minimum: z.array(z.enum(["P0", "P1", "P2"])).min(2).max(4),
  }).strict();
  const stub = stubFromZod(schema);
  assert.deepEqual(stub, {
    exact: ["classified", "classified", "classified"],
    minimum: ["P0", "P0"],
  });
  assert.equal(
    (await parseWithSchema(schema, stub, "output")) !== undefined,
    true,
  );
});

Deno.test("Zod 4 JSON Schema preserves bounds, literals, enums, and strictness", () => {
  const schema = z.object({
    code: z.string().length(4),
    summary: z.string().min(2).max(12),
    score: z.number().min(0).max(100),
    labels: z.array(z.literal("ready")).length(3),
    priority: z.enum(["P0", "P1", "P2"]),
  }).strict();
  const jsonSchema = jsonSchemaFromZod(schema);
  assert.deepEqual(jsonSchema.properties, {
    code: { type: "string", minLength: 4, maxLength: 4 },
    summary: { type: "string", minLength: 2, maxLength: 12 },
    score: { type: "number", minimum: 0, maximum: 100 },
    labels: {
      minItems: 3,
      maxItems: 3,
      type: "array",
      items: { type: "string", const: "ready" },
    },
    priority: { type: "string", enum: ["P0", "P1", "P2"] },
  });
  assert.equal(jsonSchema.additionalProperties, false);
});

import assert from "node:assert/strict";
import { z } from "zod";

import {
  jsonSchemaFromZod,
  parseWithSchema,
  renderBatchPrompts,
  SchemaValidationError,
  stubFromZod,
  validatedStubFromZod,
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
  assert.deepEqual(stubFromZod(undefined), {});
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

Deno.test("LLM JSON Schema describes the value accepted before transforms", async () => {
  const schema = z.string()
    .transform((value) => ({ summary: value }))
    .pipe(z.object({ summary: z.string() }));

  assert.deepEqual(jsonSchemaFromZod(schema), {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    type: "string",
  });

  const stub = stubFromZod(schema);
  assert.equal(typeof stub, "string");
  assert.deepEqual(await parseWithSchema(schema, stub, "output"), {
    summary: "a",
  });
});

Deno.test("LLM stubs satisfy common formats, patterns, and exclusive bounds", async () => {
  const schema = z.object({
    due_date: z.string().regex(/^\d{4}-\d{2}-\d{2}$/u),
    contact: z.string().email(),
    id: z.string().uuid(),
    score: z.number().positive(),
    ratio: z.number().gt(0).lt(0.5),
    even: z.number().multipleOf(2).gt(3).lt(7),
  });

  const stub = stubFromZod(schema);
  assert.deepEqual(await parseWithSchema(schema, stub, "output"), stub);
});

Deno.test("batch prompts retain transformed non-JSON values in memory", async () => {
  const schema = z.object({
    asOf: z.string().transform((value) => new Date(value)),
    rows: z.array(z.number()),
  });
  const parsed = await parseWithSchema(
    schema,
    { asOf: "2026-07-28T00:00:00.000Z", rows: [1, 2, 3] },
    "input",
  );
  const prompts = await renderBatchPrompts(
    parsed,
    { by: "rows", size: 2 },
    (input) => {
      const value = input as { asOf: Date; rows: number[] };
      assert.ok(value.asOf instanceof Date);
      return `${value.asOf.toISOString()}:${value.rows.join(",")}`;
    },
  );
  assert.deepEqual(prompts, [
    "2026-07-28T00:00:00.000Z:1,2",
    "2026-07-28T00:00:00.000Z:3",
  ]);
});

Deno.test("validated output stubs do not require or evaluate step input", async () => {
  let inputEvaluations = 0;
  const input = z.string().transform((value) => {
    inputEvaluations += 1;
    return value;
  });
  const output = z.object({ rows: z.array(z.string()).min(1) });

  assert.deepEqual(await validatedStubFromZod(output), { rows: ["a"] });
  assert.equal(inputEvaluations, 0);
  assert.equal(input.safeParse("unused").success, true);
});

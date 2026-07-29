// Shared Zod helpers for the Cori activity runner.
//
// This module deliberately does not import Zod. Step files may resolve either
// Zod 3 or Zod 4, so the runtime operates on the schema instance supplied by
// the step definition. Runtime parsing uses Zod's public safeParse API. JSON
// Schema generation prefers Zod 4's native converter and retains an
// introspection-based Zod 3 fallback for older workflows.

// deno-lint-ignore no-explicit-any
type Schema = any;

type SchemaIssue = {
  readonly path?: readonly PropertyKey[];
  readonly message?: string;
};

export class SchemaValidationError extends Error {
  readonly scope: "input" | "output";
  readonly issues: readonly SchemaIssue[];

  constructor(scope: "input" | "output", issues: readonly SchemaIssue[]) {
    super(formatSchemaIssues(scope, issues));
    this.name = "SchemaValidationError";
    this.scope = scope;
    this.issues = issues;
  }
}

export function isSchemaValidationError(
  value: unknown,
): value is SchemaValidationError {
  return value instanceof SchemaValidationError ||
    (value instanceof Error && value.name === "SchemaValidationError");
}

/** Parse a value through an optional Zod schema, applying defaults, strips,
 * coercions, and transforms. When no schema is declared, preserve the legacy
 * pass-through behaviour. */
export async function parseWithSchema(
  schema: Schema | undefined,
  value: unknown,
  scope: "input" | "output",
): Promise<unknown> {
  if (!isZodSchema(schema)) return value;

  const result = typeof schema.safeParseAsync === "function"
    ? await schema.safeParseAsync(value)
    : schema.safeParse(value);
  if (result.success) return result.data;
  throw new SchemaValidationError(scope, result.error?.issues ?? []);
}

export function formatSchemaIssues(
  scope: "input" | "output",
  issues: readonly SchemaIssue[],
): string {
  const rendered = issues.length > 0
    ? issues.map((issue) => {
      const path = formatPath(scope, issue.path ?? []);
      return `${path}: ${issue.message ?? "schema validation failed"}`;
    })
    : [`${scope}: schema validation failed`];
  return `${scope} schema validation failed:\n${
    rendered.map((line) => `- ${line}`).join("\n")
  }`;
}

function formatPath(
  scope: "input" | "output",
  path: readonly PropertyKey[],
): string {
  let out = scope;
  for (const segment of path) {
    if (typeof segment === "number") {
      out += `[${segment}]`;
    } else if (
      typeof segment === "string" && /^[A-Za-z_$][\w$]*$/u.test(segment)
    ) {
      out += `.${segment}`;
    } else {
      out += `[${JSON.stringify(String(segment))}]`;
    }
  }
  return out;
}

/**
 * Convert a Zod output contract to the JSON shape accepted by its parser.
 *
 * Providers return the value that is subsequently passed to `safeParse`, so
 * transforms and pipelines must be described from their input side. Advertising
 * Zod's output side would ask the provider for a post-transform value and then
 * reject that same value when parsing it.
 */
export function jsonSchemaFromZod(schema: Schema): Record<string, unknown> {
  if (!isZodSchema(schema)) return {};

  // Zod 4 exposes its supported JSON Schema conversion on every schema.
  // This carries string/number/array bounds, literals, enums, and the exact
  // object unknown-key policy without relying on private check internals.
  if (schema._zod?.def && typeof schema.toJSONSchema === "function") {
    try {
      return schema.toJSONSchema({
        io: "input",
        // Preserve as much of a mixed schema as Zod can represent. Runtime
        // parsing remains the final authority for unsupported transform nodes.
        unrepresentable: "any",
      });
    } catch {
      // Some unrepresentable transforms cannot be converted. Fall through to
      // the compatibility walker so the provider still receives the closest
      // structural contract available.
    }
  }

  return jsonSchemaFromZodV3(schema);
}

/** Produce a minimal value satisfying the declared output cardinality. */
export function stubFromZod(schema: Schema): unknown {
  // With no declared contract there is no honest data shape to invent.
  // Returning an empty object keeps dry-run trace annotations out of the
  // workflow accumulator while preserving legacy no-schema workflows.
  if (!isZodSchema(schema)) return {};
  return defaultFromJsonSchema(jsonSchemaFromZod(schema));
}

/** Build and validate a dry-run output without evaluating a step's input. */
export async function validatedStubFromZod(schema: Schema): Promise<unknown> {
  const stub = stubFromZod(schema);
  return await parseWithSchema(schema, stub, "output");
}

/**
 * Render every oversized LLM batch inside the same process that parsed the
 * input. Keeping transformed values in memory matters for valid Zod outputs
 * such as Date, Map, and class instances, which cannot cross a JSON envelope
 * without changing type.
 */
export async function renderBatchPrompts(
  input: unknown,
  batch: unknown,
  render: (chunkInput: unknown) => unknown | Promise<unknown>,
): Promise<string[]> {
  if (!batch || typeof batch !== "object" || Array.isArray(batch)) return [];
  const { by, size } = batch as Record<string, unknown>;
  if (typeof by !== "string" || by.length === 0) {
    throw new Error("LLM batch.by must be a non-empty string");
  }
  if (
    typeof size !== "number" || !Number.isSafeInteger(size) || size <= 0
  ) {
    throw new Error("LLM batch.size must be a positive integer");
  }
  if (!input || typeof input !== "object" || Array.isArray(input)) return [];

  const values = (input as Record<string, unknown>)[by];
  if (!Array.isArray(values) || values.length <= size) return [];

  const prompts: string[] = [];
  for (let offset = 0; offset < values.length; offset += size) {
    const chunkInput = {
      ...(input as Record<string, unknown>),
      [by]: values.slice(offset, offset + size),
    };
    prompts.push(String(await render(chunkInput) ?? ""));
  }
  return prompts;
}

function isZodSchema(schema: Schema | undefined): boolean {
  return !!schema && typeof schema === "object" &&
    (typeof schema.safeParseAsync === "function" ||
      typeof schema.safeParse === "function");
}

function defaultFromJsonSchema(schema: unknown): unknown {
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) {
    return null;
  }
  const def = schema as Record<string, unknown>;
  if ("default" in def) return def.default;
  if ("const" in def) return def.const;
  if (Array.isArray(def.enum) && def.enum.length > 0) return def.enum[0];
  for (const alternatives of [def.anyOf, def.oneOf] as unknown[]) {
    if (Array.isArray(alternatives) && alternatives.length > 0) {
      return defaultFromJsonSchema(alternatives[0]);
    }
  }
  if (Array.isArray(def.allOf)) {
    return def.allOf.reduce<unknown>((merged, child) => {
      const value = defaultFromJsonSchema(child);
      return isRecord(merged) && isRecord(value)
        ? { ...merged, ...value }
        : value ?? merged;
    }, {});
  }

  const type = Array.isArray(def.type)
    ? def.type.find((candidate) => candidate !== "null") ?? def.type[0]
    : def.type;
  switch (type) {
    case "object": {
      const properties = isRecord(def.properties) ? def.properties : {};
      return Object.fromEntries(
        Object.entries(properties).map(([key, child]) => [
          key,
          defaultFromJsonSchema(child),
        ]),
      );
    }
    case "array": {
      const min = nonNegativeInteger(def.minItems) ?? 0;
      const max = nonNegativeInteger(def.maxItems);
      const count = max === undefined ? min : Math.min(min, max);
      return Array.from(
        { length: count },
        (_, index) =>
          Array.isArray(def.prefixItems)
            ? defaultFromJsonSchema(def.prefixItems[index] ?? def.items)
            : defaultFromJsonSchema(def.items),
      );
    }
    case "string": {
      return stringDefault(def);
    }
    case "integer":
    case "number":
      return numberDefault(def, type === "integer");
    case "boolean":
      return false;
    case "null":
    default:
      return null;
  }
}

function stringDefault(def: Record<string, unknown>): string {
  const min = nonNegativeInteger(def.minLength) ?? 0;
  const max = nonNegativeInteger(def.maxLength);
  const preferred = stringFormatDefault(def.format);
  const patternText = typeof def.pattern === "string" ? def.pattern : undefined;
  const seeds = [
    preferred,
    "a",
    "0",
    "x".repeat(Math.max(1, min)),
    ...(patternText
      ? [
        "1970-01-01",
        "cori@example.com",
        "00000000-0000-4000-8000-000000000000",
        "https://example.com",
        "127.0.0.1",
      ]
      : []),
  ].filter((value): value is string => typeof value === "string");

  const pattern = patternText ? compilePattern(patternText) : undefined;
  for (const seed of seeds) {
    const candidate = fitStringLength(seed, min, max);
    if (candidate !== undefined && (!pattern || pattern.test(candidate))) {
      return candidate;
    }
  }

  // Unknown regular expressions cannot be generated safely without a regex
  // synthesiser. Return the closest structural value; `llm_stub` immediately
  // validates it and reports the precise unsupported constraint.
  return fitStringLength("x".repeat(Math.max(1, min)), min, max) ?? "";
}

function stringFormatDefault(format: unknown): string | undefined {
  switch (format) {
    case "date":
      return "1970-01-01";
    case "date-time":
      return new Date(0).toISOString();
    case "time":
      return "00:00:00Z";
    case "email":
      return "cori@example.com";
    case "uuid":
      return "00000000-0000-4000-8000-000000000000";
    case "uri":
    case "url":
      return "https://example.com";
    case "hostname":
      return "example.com";
    case "ipv4":
      return "127.0.0.1";
    case "ipv6":
      return "::1";
    default:
      return undefined;
  }
}

function fitStringLength(
  seed: string,
  min: number,
  max: number | undefined,
): string | undefined {
  if (max !== undefined && min > max) return undefined;
  let value = seed;
  if (value.length < min) value += "x".repeat(min - value.length);
  if (max !== undefined && value.length > max) return undefined;
  return value;
}

function compilePattern(pattern: string): RegExp | undefined {
  try {
    return new RegExp(pattern, "u");
  } catch {
    return undefined;
  }
}

function numberDefault(
  def: Record<string, unknown>,
  integer: boolean,
): number {
  const minimum = finiteNumber(def.minimum);
  const exclusiveMinimum = finiteNumber(def.exclusiveMinimum);
  const maximum = finiteNumber(def.maximum);
  const exclusiveMaximum = finiteNumber(def.exclusiveMaximum);
  const multipleOf = finiteNumber(def.multipleOf);
  const lower = exclusiveMinimum ?? minimum;
  const lowerExclusive = exclusiveMinimum !== undefined;
  const upper = exclusiveMaximum ?? maximum;
  const upperExclusive = exclusiveMaximum !== undefined;

  const candidates: number[] = [0, 1, -1];
  if (minimum !== undefined) candidates.push(minimum);
  if (maximum !== undefined) candidates.push(maximum);

  if (multipleOf !== undefined && multipleOf > 0) {
    let first = lower === undefined ? 0 : Math.ceil(lower / multipleOf);
    if (
      lower !== undefined && lowerExclusive &&
      isMultipleOf(lower, multipleOf)
    ) {
      first += 1;
    }
    let last = upper === undefined ? 0 : Math.floor(upper / multipleOf);
    if (
      upper !== undefined && upperExclusive &&
      isMultipleOf(upper, multipleOf)
    ) {
      last -= 1;
    }
    if (lower === undefined && upper !== undefined) first = Math.min(0, last);
    if (upper === undefined && lower !== undefined) last = Math.max(0, first);
    if (first <= last) {
      const index = first <= 0 && last >= 0 ? 0 : first;
      candidates.unshift(index * multipleOf);
    }
  } else if (integer) {
    const first = lower === undefined
      ? Number.MIN_SAFE_INTEGER
      : lowerExclusive
      ? Math.floor(lower) + 1
      : Math.ceil(lower);
    const last = upper === undefined
      ? Number.MAX_SAFE_INTEGER
      : upperExclusive
      ? Math.ceil(upper) - 1
      : Math.floor(upper);
    if (first <= last) {
      candidates.unshift(Math.min(Math.max(0, first), last));
    }
  } else {
    if (lower !== undefined && upper !== undefined && lower < upper) {
      candidates.unshift(lower + (upper - lower) / 2);
    } else if (lower !== undefined) {
      candidates.unshift(
        lowerExclusive ? adjacentInteriorNumber(lower, 1) : lower,
      );
    } else if (upper !== undefined) {
      candidates.unshift(
        upperExclusive ? adjacentInteriorNumber(upper, -1) : upper,
      );
    }
  }

  for (const candidate of candidates) {
    if (numberSatisfies(candidate, def, integer)) return candidate;
  }

  // An impossible numeric schema will still be rejected by the immediate
  // output validation, producing a path-aware diagnostic.
  return 0;
}

function adjacentInteriorNumber(boundary: number, direction: 1 | -1): number {
  const step = Math.max(1, Math.abs(boundary)) * 1e-6;
  return boundary + direction * step;
}

function numberSatisfies(
  candidate: number,
  def: Record<string, unknown>,
  integer: boolean,
): boolean {
  const minimum = finiteNumber(def.minimum);
  const exclusiveMinimum = finiteNumber(def.exclusiveMinimum);
  const maximum = finiteNumber(def.maximum);
  const exclusiveMaximum = finiteNumber(def.exclusiveMaximum);
  const multipleOf = finiteNumber(def.multipleOf);
  return Number.isFinite(candidate) &&
    (!integer || Number.isInteger(candidate)) &&
    (minimum === undefined || candidate >= minimum) &&
    (exclusiveMinimum === undefined || candidate > exclusiveMinimum) &&
    (maximum === undefined || candidate <= maximum) &&
    (exclusiveMaximum === undefined || candidate < exclusiveMaximum) &&
    (multipleOf === undefined ||
      multipleOf <= 0 ||
      isMultipleOf(candidate, multipleOf));
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function isMultipleOf(value: number, divisor: number): boolean {
  const quotient = value / divisor;
  return Math.abs(quotient - Math.round(quotient)) < 1e-9;
}

function nonNegativeInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

// ---------------------------------------------------------------------------
// Zod 3 JSON Schema fallback
// ---------------------------------------------------------------------------

function jsonSchemaFromZodV3(schema: Schema): Record<string, unknown> {
  const def = schema?._def;
  if (!def) return {};
  switch (def.typeName) {
    case "ZodString":
      return withV3StringChecks({ type: "string" }, def.checks ?? []);
    case "ZodNumber":
      return withV3NumberChecks({ type: "number" }, def.checks ?? []);
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
      return literalJsonSchema([def.value]);
    case "ZodEnum":
      return literalJsonSchema(def.values ?? []);
    case "ZodNativeEnum":
      return literalJsonSchema(
        uniqueJsonValues(Object.values(def.values ?? {})),
      );
    case "ZodOptional":
    case "ZodNullable":
    case "ZodDefault":
    case "ZodCatch":
    case "ZodBranded":
    case "ZodReadonly":
      return jsonSchemaFromZodV3(def.innerType);
    case "ZodEffects":
      return jsonSchemaFromZodV3(def.schema);
    case "ZodPipeline":
      return jsonSchemaFromZodV3(def.in ?? def.out);
    case "ZodLazy":
      try {
        return jsonSchemaFromZodV3(def.getter?.());
      } catch {
        return {};
      }
    case "ZodArray": {
      const out: Record<string, unknown> = {
        type: "array",
        items: jsonSchemaFromZodV3(def.type),
      };
      const exact = lengthValue(def.exactLength);
      const min = exact ?? lengthValue(def.minLength);
      const max = exact ?? lengthValue(def.maxLength);
      if (min !== undefined) out.minItems = min;
      if (max !== undefined) out.maxItems = max;
      return out;
    }
    case "ZodTuple": {
      const items = (def.items ?? []).map((child: Schema) =>
        jsonSchemaFromZodV3(child)
      );
      return {
        type: "array",
        prefixItems: items,
        minItems: items.length,
        ...(def.rest ? {} : { maxItems: items.length }),
      };
    }
    case "ZodUnion":
    case "ZodDiscriminatedUnion":
      return {
        anyOf: [...(def.options?.values?.() ?? def.options ?? [])].map(
          (child: Schema) => jsonSchemaFromZodV3(child),
        ),
      };
    case "ZodIntersection":
      return {
        allOf: [
          jsonSchemaFromZodV3(def.left),
          jsonSchemaFromZodV3(def.right),
        ],
      };
    case "ZodRecord":
      return {
        type: "object",
        additionalProperties: jsonSchemaFromZodV3(def.valueType),
      };
    case "ZodMap":
      return { type: "object", additionalProperties: true };
    case "ZodObject": {
      const shape = typeof def.shape === "function" ? def.shape() : def.shape;
      const properties: Record<string, unknown> = {};
      const required: string[] = [];
      for (const [key, child] of Object.entries(shape ?? {})) {
        properties[key] = jsonSchemaFromZodV3(child);
        const childType = (child as Schema)?._def?.typeName;
        if (childType !== "ZodOptional" && childType !== "ZodDefault") {
          required.push(key);
        }
      }
      const catchall = def.catchall?._def?.typeName;
      const additionalProperties = catchall && catchall !== "ZodNever"
        ? jsonSchemaFromZodV3(def.catchall)
        : def.unknownKeys === "passthrough";
      return {
        type: "object",
        properties,
        ...(required.length > 0 ? { required } : {}),
        additionalProperties,
      };
    }
    default:
      return {};
  }
}

function withV3StringChecks(
  base: Record<string, unknown>,
  checks: readonly Record<string, unknown>[],
): Record<string, unknown> {
  const out = { ...base };
  for (const check of checks) {
    if (check.kind === "min") out.minLength = check.value;
    if (check.kind === "max") out.maxLength = check.value;
    if (check.kind === "length") {
      out.minLength = check.value;
      out.maxLength = check.value;
    }
  }
  return out;
}

function withV3NumberChecks(
  base: Record<string, unknown>,
  checks: readonly Record<string, unknown>[],
): Record<string, unknown> {
  const out = { ...base };
  for (const check of checks) {
    if (check.kind === "int") out.type = "integer";
    if (check.kind === "min") {
      out[check.inclusive === false ? "exclusiveMinimum" : "minimum"] =
        check.value;
    }
    if (check.kind === "max") {
      out[check.inclusive === false ? "exclusiveMaximum" : "maximum"] =
        check.value;
    }
  }
  return out;
}

function lengthValue(value: unknown): number | undefined {
  if (typeof value === "number") return value;
  if (isRecord(value) && typeof value.value === "number") return value.value;
  return undefined;
}

function uniqueJsonValues(values: unknown[]): unknown[] {
  const seen = new Set<string>();
  return values.filter((value) => {
    if (
      !["string", "number", "boolean"].includes(typeof value) && value !== null
    ) {
      return false;
    }
    const key = JSON.stringify(value);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function literalJsonSchema(values: unknown[]): Record<string, unknown> {
  const schemas = values.map((value) => {
    const jsonType = value === null ? "null" : typeof value;
    const type = ["null", "string", "number", "boolean"].includes(jsonType)
      ? jsonType
      : undefined;
    return type ? { type, const: value } : { const: value };
  });
  if (schemas.length === 0) return {};
  if (schemas.length === 1) return schemas[0]!;
  const types = new Set(
    schemas.map((candidate) => candidate.type).filter(Boolean),
  );
  return types.size === 1
    ? { type: [...types][0], enum: values }
    : { anyOf: schemas };
}

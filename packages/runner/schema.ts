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

/** Convert a Zod output contract to JSON Schema for structured-output LLMs. */
export function jsonSchemaFromZod(schema: Schema): Record<string, unknown> {
  if (!isZodSchema(schema)) return {};

  // Zod 4 exposes its supported JSON Schema conversion on every schema.
  // This carries string/number/array bounds, literals, enums, and the exact
  // object unknown-key policy without relying on private check internals.
  if (schema._zod?.def && typeof schema.toJSONSchema === "function") {
    try {
      return schema.toJSONSchema();
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
  if (!isZodSchema(schema)) return { mocked: true };
  return defaultFromJsonSchema(jsonSchemaFromZod(schema));
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
      if (def.format === "date-time") return new Date(0).toISOString();
      const min = nonNegativeInteger(def.minLength) ?? 0;
      const max = nonNegativeInteger(def.maxLength);
      return "x".repeat(max === undefined ? min : Math.min(min, max));
    }
    case "integer":
    case "number":
      return typeof def.minimum === "number" ? def.minimum : 0;
    case "boolean":
      return false;
    case "null":
    default:
      return null;
  }
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
      return jsonSchemaFromZodV3(def.out ?? def.in);
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

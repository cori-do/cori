import { z } from "zod";

import {
  type BranchOpts,
  type CliStepOpts,
  type CodeStepOpts,
  step,
} from "../src/index.js";

function expectType<T>(_value: T): void {}

const parsedInput = z.object({
  source: z.string().transform((value) => value.length),
});
const literalOutput = z.object({
  created: z.literal(true),
  status: z.enum(["ready", "blocked"]),
  nested: z.object({
    rows: z.array(z.object({
      kind: z.enum(["primary", "secondary"]),
      values: z.array(z.union([z.string(), z.number()])),
    })),
  }),
});

step.cli({
  description: "natural sync CLI literals",
  input: parsedInput,
  output: literalOutput,
  command: (input) => {
    expectType<number>(input.source);
    return ["tool", String(input.source)];
  },
  parse: () => ({
    created: true,
    status: "ready",
    nested: {
      rows: [{ kind: "primary", values: ["one", 2] }],
    },
  }),
});

step.cli({
  description: "natural async CLI literals",
  output: literalOutput,
  command: () => ["tool"],
  parse: async () => ({
    created: true,
    status: "blocked",
    nested: {
      rows: [{ kind: "secondary", values: [1, "two"] }],
    },
  }),
});

step.code({
  description: "natural sync code literals",
  input: parsedInput,
  output: literalOutput,
  run: (input) => {
    expectType<number>(input.source);
    return {
      created: true,
      status: "ready",
      nested: {
        rows: [{ kind: "primary", values: ["one"] }],
      },
    };
  },
});

step.code({
  description: "natural async code literals",
  output: literalOutput,
  run: async () => ({
    created: true,
    status: "blocked",
    nested: {
      rows: [{ kind: "secondary", values: [2] }],
    },
  }),
});

const schemaInputShapes = z.object({
  transformed: z.string().transform((value) => value.length),
  coerced: z.coerce.number<string>(),
  defaulted: z.string().default("fallback"),
});

step.cli({
  description: "CLI returns schema inputs before Zod parsing",
  output: schemaInputShapes,
  command: () => ["tool"],
  parse: () => ({
    transformed: "four",
    coerced: "42",
  }),
});

step.code({
  description: "code returns schema inputs before Zod parsing",
  output: schemaInputShapes,
  run: () => ({
    transformed: "five",
    coerced: "43",
    defaulted: undefined,
  }),
});

step.cli({
  description: "omitted schemas remain compatible",
  command: () => ["tool"],
  parse: () => ({ anything: true }),
});

step.code({
  description: "omitted schemas remain compatible",
  run: () => ({ anything: true }),
});

const explicitCli: CliStepOpts<typeof parsedInput, typeof literalOutput> = {
  description: "explicit CLI options",
  input: parsedInput,
  output: literalOutput,
  command: () => ["tool"],
  parse: () => ({
    created: true,
    status: "ready",
    nested: { rows: [] },
  }),
};
step.cli(explicitCli);

const explicitCode: CodeStepOpts<typeof parsedInput, typeof literalOutput> = {
  description: "explicit code options",
  input: parsedInput,
  output: literalOutput,
  run: () => ({
    created: true,
    status: "ready",
    nested: { rows: [] },
  }),
};
step.code(explicitCode);

const readyStep = step.wait({
  description: "ready",
  for: { signal: "ready" },
});
const blockedStep = step.wait({
  description: "blocked",
  for: { signal: "blocked" },
});

step.branch({
  description: "cases determine branch keys",
  on: () => "ready",
  cases: {
    ready: readyStep,
    blocked: blockedStep,
  },
});

const explicitBranch: BranchOpts<"ready" | "blocked"> = {
  description: "explicit branch options",
  on: () => "blocked",
  cases: {
    ready: readyStep,
    blocked: blockedStep,
  },
};
step.branch(explicitBranch);

step.cli({
  description: "reject wrong literal",
  output: z.object({ created: z.literal(true) }),
  command: () => ["tool"],
  // @ts-expect-error false is not accepted by z.literal(true)
  parse: () => ({ created: false }),
});

step.code({
  description: "reject wrong enum member",
  output: z.object({ status: z.enum(["ready", "blocked"]) }),
  // @ts-expect-error "other" is not an output-schema input
  run: () => ({ status: "other" }),
});

step.cli({
  description: "reject wrong nested value",
  output: literalOutput,
  command: () => ["tool"],
  // @ts-expect-error nested enum values remain constrained
  parse: () => ({
    created: true,
    status: "ready",
    nested: {
      rows: [{ kind: "tertiary", values: [] }],
    },
  }),
});

const unknownRows: unknown[][] = [["not narrowed"]];
step.cli({
  description: "unknown nested rows remain an authoring error",
  output: z.object({ rows: z.array(z.array(z.string())) }),
  command: () => ["tool"],
  // @ts-expect-error unknown[][] must be narrowed before returning it
  parse: () => ({ rows: unknownRows }),
});

step.branch({
  description: "reject a key absent from cases",
  // @ts-expect-error cases, not on, determine the valid key union
  on: () => "missing",
  cases: {
    ready: readyStep,
    blocked: blockedStep,
  },
});

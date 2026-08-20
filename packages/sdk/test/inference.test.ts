import { z } from "zod";

import {
  type BranchOpts,
  type CliStepOpts,
  type CodeStepOpts,
  goto,
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

const readyStep = step.code({
  description: "ready",
  run: () => ({ handled: "ready" }),
});
const blockedStep = step.code({
  description: "blocked",
  run: () => ({ handled: "blocked" }),
});

step.switch({
  description: "cases determine switch keys",
  on: () => "ready",
  cases: {
    ready: readyStep,
    blocked: blockedStep,
  },
});

step.branch({
  description: "if / else takes nested steps",
  if: (input: { count: number }) => input.count > 0,
  then: readyStep,
  else: blockedStep,
});

const explicitBranch: BranchOpts<{ count: number }> = {
  description: "explicit branch options",
  if: (input) => input.count > 0,
  then: readyStep,
};
step.branch(explicitBranch);

step.loop({
  description: "loop repeats until the goal is met",
  body: readyStep,
  until: (input: { done: boolean }) => input.done,
  max_iterations: 5,
});

step.wait({
  description: "wait pauses for a delay or event",
  for: { signal: "approved", timeout_ms: 60_000 },
});

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

step.switch(
  // @ts-expect-error without a default, cases determine the valid key union
  {
    description: "reject a key absent from cases",
    on: () => "missing",
    cases: {
      ready: readyStep,
      blocked: blockedStep,
    },
  },
);

step.switch({
  description: "a default relaxes on to any string",
  on: (input) => (input as { label: string }).label,
  cases: {
    ready: readyStep,
    blocked: blockedStep,
  },
  default: readyStep,
});

// Routing: branch / switch paths accept goto() refs, freely mixed with
// inline nested steps.
step.branch({
  description: "route the false path",
  if: (input) => (input as { big: boolean }).big,
  then: readyStep,
  else: goto("summarize_small"),
});

step.switch({
  description: "route cases to later steps",
  on: () => "ready",
  cases: {
    ready: goto("fast_path"),
    blocked: blockedStep,
  },
  default: goto("end"),
});

step.for_each({
  description: "loop bodies cannot route",
  over: (input) => (input as { rows: unknown[] }).rows,
  // @ts-expect-error for_each.apply is inline-only — no goto refs
  apply: goto("later_step"),
});

step.loop({
  description: "loop bodies cannot route either",
  // @ts-expect-error loop.body is inline-only — no goto refs
  body: goto("later_step"),
  until: () => true,
});

step.branch({
  description: "reject a builtin nested inside a builtin",
  if: () => true,
  // @ts-expect-error builtins cannot nest builtins
  then: step.wait({ description: "nested wait", for: { timeout_ms: 1 } }),
});

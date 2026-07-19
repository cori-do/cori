/**
 * @cori-do/sdk — TypeScript SDK for authoring Cori workflow steps.
 *
 * The SDK exposes the typed `step.<kind>(...)` constructors used by every
 * step file. The runtime is intentionally inert — each
 * constructor returns a plain `StepDef` object describing the step. The
 * Rust worker statically parses step files and dispatches the actual work;
 * the SDK exists to give the agent (and the user's editor) strong type
 * inference and a single source of truth for the step shape.
 *
 * Zod is the only supported schema library for v1.
 */

import type { infer as ZodInfer, ZodTypeAny } from "zod";

// ---------------------------------------------------------------------------
// Kinds & shared shapes
// ---------------------------------------------------------------------------

export type StepKind = "cli" | "mcp_tool" | "code" | "llm" | "builtin";

export type BackoffKind = "exponential" | "linear";

export interface RetryPolicy {
  readonly max: number;
  readonly backoff: BackoffKind;
}

export interface BaseStepOpts {
  /** One-line summary. Shown in the run trace. */
  readonly description: string;
  /** Optional retry policy override (defaults vary by kind). */
  readonly retries?: RetryPolicy;
  /** Per-attempt timeout in milliseconds. */
  readonly timeout_ms?: number;
  /** Optional override of the manifest's `route_default`. */
  readonly route?: string;
}

export interface StepDef<K extends StepKind = StepKind> {
  readonly kind: K;
  readonly description: string;
  readonly retries?: RetryPolicy;
  readonly timeout_ms?: number;
  readonly route?: string;
  /** Discriminator the compiler / worker rely on at runtime introspection. */
  readonly __cori_step: true;
}

// ---------------------------------------------------------------------------
// cli
// ---------------------------------------------------------------------------

export interface CliStepOpts<I extends ZodTypeAny, O extends ZodTypeAny>
  extends BaseStepOpts {
  readonly input?: I;
  readonly output?: O;
  /** Argv builder — return an array, never a single string. */
  readonly command: (input: ZodInfer<I>) => readonly string[];
  /** Parse the captured stdout into the typed output. */
  readonly parse?: (
    stdout: string,
    ctx: { stderr: string; exitCode: number },
  ) => ZodInfer<O> | Promise<ZodInfer<O>>;
  /** Extra environment variables for the spawned process. */
  readonly env?: Record<string, string>;
}

export interface CliStepDef extends StepDef<"cli"> {
  readonly input?: ZodTypeAny;
  readonly output?: ZodTypeAny;
  readonly command: (input: unknown) => readonly string[];
  readonly parse?: CliStepOpts<ZodTypeAny, ZodTypeAny>["parse"];
  readonly env?: Record<string, string>;
}

// ---------------------------------------------------------------------------
// mcp_tool
// ---------------------------------------------------------------------------

export interface McpStepOpts<I extends ZodTypeAny, O extends ZodTypeAny>
  extends BaseStepOpts {
  readonly server: string;
  readonly tool: string;
  readonly input?: I;
  readonly output?: O;
  readonly args: (input: ZodInfer<I>) => Record<string, unknown>;
}

export interface McpStepDef extends StepDef<"mcp_tool"> {
  readonly server: string;
  readonly tool: string;
  readonly input?: ZodTypeAny;
  readonly output?: ZodTypeAny;
  readonly args: (input: unknown) => Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// code
// ---------------------------------------------------------------------------

export interface CodeStepOpts<I extends ZodTypeAny, O extends ZodTypeAny>
  extends BaseStepOpts {
  readonly input?: I;
  readonly output?: O;
  readonly run: (input: ZodInfer<I>) => ZodInfer<O> | Promise<ZodInfer<O>>;
}

export interface CodeStepDef extends StepDef<"code"> {
  readonly input?: ZodTypeAny;
  readonly output?: ZodTypeAny;
  readonly run: (input: unknown) => unknown;
}

// ---------------------------------------------------------------------------
// llm
// ---------------------------------------------------------------------------

export interface LlmBatchOpts {
  readonly size: number;
  readonly by: string;
}

export interface LlmStepOpts<I extends ZodTypeAny, O extends ZodTypeAny>
  extends BaseStepOpts {
  readonly model: string;
  readonly input?: I;
  readonly output?: O;
  readonly prompt: (input: ZodInfer<I>) => string;
  readonly batch?: LlmBatchOpts;
}

export interface LlmStepDef extends StepDef<"llm"> {
  readonly model: string;
  readonly prompt: (input: unknown) => string;
  readonly batch?: LlmBatchOpts;
  readonly input?: ZodTypeAny;
  readonly output?: ZodTypeAny;
}

// ---------------------------------------------------------------------------
// builtins
// ---------------------------------------------------------------------------

export interface MapOpts<I, O> extends BaseStepOpts {
  readonly over: (input: I) => readonly unknown[];
  readonly apply: StepDef;
  readonly concurrency?: number;
  readonly _phantom?: O;
}

export interface ForEachOpts<I, O> extends BaseStepOpts {
  readonly over: (input: I) => readonly unknown[];
  readonly apply: StepDef;
  readonly _phantom?: O;
}

export interface BranchOpts<T extends string> extends BaseStepOpts {
  readonly on: (input: unknown) => T;
  readonly cases: Record<T, StepDef>;
}

export interface ParallelOpts extends BaseStepOpts {
  readonly steps: readonly StepDef[];
}

export interface WaitOpts extends BaseStepOpts {
  readonly for: {
    readonly signal?: string;
    readonly timeout_ms?: number;
    readonly until?: string;
  };
}

export interface BuiltinStepDef extends StepDef<"builtin"> {
  readonly builtin: "map" | "for_each" | "branch" | "parallel" | "wait";
}

// ---------------------------------------------------------------------------
// Constructor surface
// ---------------------------------------------------------------------------

function base<K extends StepKind>(kind: K, opts: BaseStepOpts): StepDef<K> {
  return {
    kind,
    description: opts.description,
    retries: opts.retries,
    timeout_ms: opts.timeout_ms,
    route: opts.route,
    __cori_step: true,
  };
}

export const step = {
  cli<I extends ZodTypeAny, O extends ZodTypeAny>(
    opts: CliStepOpts<I, O>,
  ): CliStepDef {
    return {
      ...base("cli", opts),
      input: opts.input,
      output: opts.output,
      command: opts.command as (input: unknown) => readonly string[],
      parse: opts.parse,
      env: opts.env,
    };
  },

  mcp_tool<I extends ZodTypeAny, O extends ZodTypeAny>(
    opts: McpStepOpts<I, O>,
  ): McpStepDef {
    return {
      ...base("mcp_tool", opts),
      server: opts.server,
      tool: opts.tool,
      input: opts.input,
      output: opts.output,
      args: opts.args as (input: unknown) => Record<string, unknown>,
    };
  },

  code<I extends ZodTypeAny, O extends ZodTypeAny>(
    opts: CodeStepOpts<I, O>,
  ): CodeStepDef {
    return {
      ...base("code", opts),
      input: opts.input,
      output: opts.output,
      run: opts.run as (input: unknown) => unknown,
    };
  },

  llm<I extends ZodTypeAny, O extends ZodTypeAny>(
    opts: LlmStepOpts<I, O>,
  ): LlmStepDef {
    return {
      ...base("llm", opts),
      model: opts.model,
      prompt: opts.prompt as (input: unknown) => string,
      batch: opts.batch,
      input: opts.input,
      output: opts.output,
    };
  },

  map<I, O>(opts: MapOpts<I, O>): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "map" };
  },

  for_each<I, O>(opts: ForEachOpts<I, O>): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "for_each" };
  },

  branch<T extends string>(opts: BranchOpts<T>): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "branch" };
  },

  parallel(opts: ParallelOpts): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "parallel" };
  },

  wait(opts: WaitOpts): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "wait" };
  },
} as const;

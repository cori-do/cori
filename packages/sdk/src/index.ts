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

import type {
  input as ZodInput,
  output as ZodOutput,
  ZodTypeAny,
} from "zod";

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

/**
 * Callback results are validated by Zod after the step runs. Accept readonly
 * literal inference at authoring time while retaining the schema's input
 * shape as the constraint.
 */
export type RecursiveReadonly<T> = T extends (...args: never[]) => unknown
  ? T
  : T extends readonly unknown[]
  ? { readonly [K in keyof T]: RecursiveReadonly<T[K]> }
  : T extends object
  ? { readonly [K in keyof T]: RecursiveReadonly<T[K]> }
  : T;

// ---------------------------------------------------------------------------
// cli
// ---------------------------------------------------------------------------

export interface CliStepOpts<
  I extends ZodTypeAny,
  O extends ZodTypeAny,
  R extends RecursiveReadonly<ZodInput<O>> =
    RecursiveReadonly<ZodInput<O>>,
>
  extends BaseStepOpts {
  readonly input?: I;
  readonly output?: O;
  /** Argv builder — return an array, never a single string. */
  readonly command: (input: ZodOutput<I>) => readonly string[];
  /** Parse the captured stdout into the typed output. */
  readonly parse?: (
    stdout: string,
    ctx: { stderr: string; exitCode: number },
  ) => R | Promise<R>;
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
  readonly args: (input: ZodOutput<I>) => Record<string, unknown>;
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

export interface CodeStepOpts<
  I extends ZodTypeAny,
  O extends ZodTypeAny,
  R extends RecursiveReadonly<ZodInput<O>> =
    RecursiveReadonly<ZodInput<O>>,
>
  extends BaseStepOpts {
  readonly input?: I;
  readonly output?: O;
  readonly run: (input: ZodOutput<I>) => R | Promise<R>;
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

/**
 * How much model capability an LLM step needs, independent of provider.
 *
 * - `low` — classification, extraction, and short rewrites
 * - `medium` — the default; most workflow model calls
 * - `high` — multi-constraint reasoning and long synthesis
 */
export type LlmLevel = "low" | "medium" | "high";

export interface LlmStepOpts<I extends ZodTypeAny, O extends ZodTypeAny>
  extends BaseStepOpts {
  /**
   * Optional. Omission means `medium`. The active provider on the worker
   * maps this portable level to one of its own models.
   */
  readonly level?: LlmLevel;
  readonly input?: I;
  readonly output?: O;
  readonly prompt: (input: ZodOutput<I>) => string;
  readonly batch?: LlmBatchOpts;
}

export interface LlmStepDef extends StepDef<"llm"> {
  readonly level: LlmLevel;
  /** Runtime-only bridge for Temporal activities started by older builds. */
  readonly __legacyModel?: string;
  readonly prompt: (input: unknown) => string;
  readonly batch?: LlmBatchOpts;
  readonly input?: ZodTypeAny;
  readonly output?: ZodTypeAny;
}

// ---------------------------------------------------------------------------
// builtins — Cori's control-flow primitives.
//
// Four are executable: `branch` (if / else), `switch`, `for_each` / `loop`,
// and `wait` (delay). `map` and `parallel` are still accepted by the
// compiler but deferred at runtime.
//
// A builtin's nested steps (`then`, `cases.<label>`, `apply`, `body`, …)
// are ordinary non-builtin StepDefs declared inline in the same file. The
// worker evaluates the selector function (`if` / `on` / `over` / `until`)
// in the sandboxed runner, then dispatches the selected nested step like
// any other activity. Builtins cannot nest other builtins.
// ---------------------------------------------------------------------------

export type BuiltinKind =
  | "map"
  | "for_each"
  | "branch"
  | "switch"
  | "loop"
  | "parallel"
  | "wait";

/** Steps a builtin may contain. Builtins cannot nest builtins. */
export type NestedStepDef = CliStepDef | McpStepDef | CodeStepDef | LlmStepDef;

/**
 * A routing target produced by [`goto`]. Where a `branch` / `switch`
 * path accepts one, the path jumps to the named sibling step instead of
 * running an inline nested step.
 */
export interface GotoRef {
  readonly __cori_goto: string;
}

/**
 * Route a `branch` / `switch` path to a later sibling step (forward
 * only), or to `"end"` to finish the run after this step.
 *
 * `target` is the step's *name* — the snake_case part of its
 * `NN_name.ts` filename, without the number — so renumbering steps
 * never breaks a route. The compiler resolves it and rejects unknown,
 * ambiguous, backward, or self targets.
 */
export function goto(target: string): GotoRef {
  return { __cori_goto: target };
}

/** What a `branch` / `switch` path may be: run one step, or route. */
export type BranchPath = NestedStepDef | GotoRef;

export interface MapOpts<I, O> extends BaseStepOpts {
  readonly over: (input: I) => readonly unknown[];
  readonly apply: NestedStepDef;
  readonly concurrency?: number;
  readonly _phantom?: O;
}

export interface ForEachOpts<I, O> extends BaseStepOpts {
  /** Extract the list to iterate from the accumulated input. Pure. */
  readonly over: (input: I) => readonly unknown[];
  /**
   * Step applied to each item, sequentially. Receives the accumulated
   * input plus `item` and `item_index` fields.
   */
  readonly apply: NestedStepDef;
  /** Upper bound on iterated items (default 100). */
  readonly max_items?: number;
  readonly _phantom?: O;
}

/** If / Else: splits the path based on whether a rule is met. */
export interface BranchOpts<I> extends BaseStepOpts {
  /** The rule. Evaluated in the sandboxed runner; must be pure. */
  readonly if: (input: I) => boolean;
  /** Path when the rule holds: run a step, or `goto(...)` a later one. */
  readonly then: BranchPath;
  /** Optional path when it does not; omitting it makes `false` a no-op. */
  readonly else?: BranchPath;
}

/**
 * Switch: sends the process down one of many paths based on a value.
 *
 * Without a `default`, `on` must provably return one of the declared
 * case labels (an unmatched label fails the run). Declaring a `default`
 * relaxes `on` to any string — unmatched labels take the default path.
 */
type SwitchOpts<C extends Readonly<Record<string, BranchPath>>> =
  BaseStepOpts & { readonly cases: C } & (
    | {
        /** Fallback for labels not declared in `cases`. */
        readonly default: BranchPath;
        /** Compute the case label from the accumulated input. Pure. */
        readonly on: (input: unknown) => string;
      }
    | {
        readonly default?: undefined;
        /** Compute the case label from the accumulated input. Pure. */
        readonly on: (input: unknown) => NoInfer<Extract<keyof C, string>>;
      }
  );

/** Loop: repeats a step until a goal is met. */
export interface LoopOpts<I> extends BaseStepOpts {
  /**
   * Step to repeat. Its output is merged into the accumulated input
   * before `until` is evaluated and before the next iteration.
   */
  readonly body: NestedStepDef;
  /** The goal. Checked after each iteration; `true` ends the loop. */
  readonly until: (input: I) => boolean;
  /**
   * Iteration cap (default 10, max 100). Reaching it without `until`
   * turning true fails the run.
   */
  readonly max_iterations?: number;
}

export interface ParallelOpts extends BaseStepOpts {
  readonly steps: readonly NestedStepDef[];
}

/** Wait / Delay: pauses the workflow until a time or event occurs. */
export interface WaitOpts extends BaseStepOpts {
  readonly for: {
    /**
     * Name of an external event to wait for (delivered via the
     * workflow's `event_received` signal).
     */
    readonly signal?: string;
    /**
     * Duration to pause, in milliseconds. Combined with `signal` it
     * acts as the wait's timeout instead of a plain delay.
     */
    readonly timeout_ms?: number;
    /** Absolute RFC 3339 timestamp to resume at (e.g. `2026-09-01T09:00:00Z`). */
    readonly until?: string;
  };
}

export interface BuiltinStepDef extends StepDef<"builtin"> {
  readonly builtin: BuiltinKind;
  // Control-flow fields are kept on the runtime object so the runner can
  // evaluate selectors and resolve nested steps at execution time.
  readonly if?: (input: unknown) => boolean;
  readonly on?: (input: unknown) => string;
  readonly over?: (input: unknown) => readonly unknown[];
  readonly until?: (input: unknown) => boolean;
  readonly then?: BranchPath;
  readonly else?: BranchPath;
  readonly cases?: Readonly<Record<string, BranchPath>>;
  readonly default?: BranchPath;
  readonly apply?: NestedStepDef;
  readonly body?: NestedStepDef;
  readonly steps?: readonly NestedStepDef[];
  readonly for?: WaitOpts["for"];
  readonly max_items?: number;
  readonly max_iterations?: number;
  readonly concurrency?: number;
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
  cli<
    I extends ZodTypeAny,
    O extends ZodTypeAny,
    const R extends RecursiveReadonly<ZodInput<O>> =
      RecursiveReadonly<ZodInput<O>>,
  >(
    opts: CliStepOpts<I, O, R>,
  ): CliStepDef {
    return {
      ...base("cli", opts),
      // Keep the declared schemas on the runtime object — the runner
      // enforces `output` after parse. Dropping them here made "typed
      // workflows" a compile-time-only promise (field finding, 2026-07-22).
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

  code<
    I extends ZodTypeAny,
    O extends ZodTypeAny,
    const R extends RecursiveReadonly<ZodInput<O>> =
      RecursiveReadonly<ZodInput<O>>,
  >(
    opts: CodeStepOpts<I, O, R>,
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
    // New source cannot type-check or compile with `model`, but retaining
    // the value at runtime lets an already-started Temporal activity resume
    // safely after an upgrade.
    const legacyModel = (opts as unknown as { model?: string }).model;
    return {
      ...base("llm", opts),
      level: opts.level ?? "medium",
      __legacyModel: legacyModel,
      prompt: opts.prompt as (input: unknown) => string,
      batch: opts.batch,
      input: opts.input,
      output: opts.output,
    };
  },

  /** Deferred in v1: accepted by the compiler, not yet executed. */
  map<I, O>(opts: MapOpts<I, O>): BuiltinStepDef {
    return {
      ...base("builtin", opts),
      builtin: "map",
      over: opts.over as (input: unknown) => readonly unknown[],
      apply: opts.apply,
      concurrency: opts.concurrency,
    };
  },

  /** Repeat a nested step once per item of a runtime-derived list. */
  for_each<I, O>(opts: ForEachOpts<I, O>): BuiltinStepDef {
    return {
      ...base("builtin", opts),
      builtin: "for_each",
      over: opts.over as (input: unknown) => readonly unknown[],
      apply: opts.apply,
      max_items: opts.max_items,
    };
  },

  /** If / Else: split the path based on whether a rule is met. */
  branch<I>(opts: BranchOpts<I>): BuiltinStepDef {
    return {
      ...base("builtin", opts),
      builtin: "branch",
      if: opts.if as (input: unknown) => boolean,
      then: opts.then,
      else: opts.else,
    };
  },

  /** Switch: send the process down one of many paths based on a value. */
  switch<const C extends Readonly<Record<string, BranchPath>>>(
    opts: SwitchOpts<C>,
  ): BuiltinStepDef {
    return {
      ...base("builtin", opts),
      builtin: "switch",
      on: opts.on as (input: unknown) => string,
      cases: opts.cases,
      default: opts.default,
    };
  },

  /** Loop: repeat a nested step until a goal is met. */
  loop<I>(opts: LoopOpts<I>): BuiltinStepDef {
    return {
      ...base("builtin", opts),
      builtin: "loop",
      body: opts.body,
      until: opts.until as (input: unknown) => boolean,
      max_iterations: opts.max_iterations,
    };
  },

  /** Deferred in v1: accepted by the compiler, not yet executed. */
  parallel(opts: ParallelOpts): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "parallel", steps: opts.steps };
  },

  /** Wait / Delay: pause until a time or event occurs. */
  wait(opts: WaitOpts): BuiltinStepDef {
    return { ...base("builtin", opts), builtin: "wait", for: opts.for };
  },
} as const;

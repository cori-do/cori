// SPDX-License-Identifier: Apache-2.0
/**
 * @cori-do/sdk v2 — typed, inert step declarations.
 *
 * Every constructor returns a plain object. Nothing here runs: the compiler
 * (Rust + oxc) reads the file statically, generates `.cori/context.ts`, and
 * `deno check` proves every binding. The runner evaluates `input`, `run`,
 * `prompt`, `files`, `request`, `command` and selectors inside a sandbox.
 *
 * Bindings read a typed context: `input: (ctx: Ctx<"my_step">) => ({...})`.
 * `Ctx` is generated per workflow and only exposes predecessors.
 */
import type { input as ZodInput, output as ZodOutput, ZodTypeAny } from "zod";

export type Tier = "small" | "medium" | "large";
export type BackoffKind = "exponential" | "linear";
/** `max` attempts in all (1 to 10). Retries wait 1 s, then twice as long each time, at most 60 s apart: `backoff` is
 *  accepted for older steps and changes nothing. */
export interface RetryPolicy { readonly max: number; readonly backoff?: BackoffKind }

/** Fields every step shares. `name` is the dataflow identity of the step. */
export interface BaseOpts {
  readonly name: string;
  readonly description: string;
  readonly retries?: RetryPolicy;
  readonly timeout_ms?: number;
  /** Extra ordering edges without data (step names). */
  readonly after?: readonly string[];
}

/** What a step declares to the compiler and the runner. Only `output` is
 * type-relevant downstream, which keeps `typeof step` free of the binding's
 * context type and avoids circular inference through `context.ts`. */
export interface Step<K extends string, O extends ZodTypeAny = ZodTypeAny> {
  readonly kind: K;
  readonly name: string;
  readonly output: O;
  readonly __cori_step: true;
}

type Bound<C, I> = (ctx: C) => I;
/** Deep-readonly view of a schema's input, used as the constraint of return-type parameters so that
 * literal types are preserved during inference (TS keeps literals when the constraint contains them). */
export type Shape<T> = T extends (...args: never[]) => unknown ? T
  : T extends readonly unknown[] ? { readonly [K in keyof T]: Shape<T[K]> }
  : T extends object ? { readonly [K in keyof T]: Shape<T[K]> }
  : T;
type In<O extends ZodTypeAny> = Shape<ZodInput<O>>;

function def<K extends string, O extends ZodTypeAny>(kind: K, opts: { name: string; output: O }): Step<K, O> {
  return { ...opts, kind, __cori_step: true } as unknown as Step<K, O>;
}

// ---------------------------------------------------------------------------
// code — pure function, provably no I/O
// ---------------------------------------------------------------------------
export interface CodeOpts<C, I, O extends ZodTypeAny, R extends In<O>> extends BaseOpts {
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly run: (input: I) => R | Promise<R>;
}
function code<C, I, O extends ZodTypeAny, const R extends In<O> = In<O>>(opts: CodeOpts<C, I, O, R>): Step<"code", O> { return def("code", opts); }

// ---------------------------------------------------------------------------
// llm — typed in, typed out, cost tier
// ---------------------------------------------------------------------------
export interface LlmOpts<C, I, O extends ZodTypeAny> extends BaseOpts {
  readonly tier?: Tier;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly prompt: (input: I) => string;
  /** The files the model reads besides the prompt, from the step's input: PDFs as documents, PNG, JPEG, GIF and WebP as
   *  images, text files (`text/*`, JSON, XML) as text. At most 20 MiB in all. */
  readonly files?: (input: I) => CoriFile | readonly CoriFile[] | undefined;
  readonly examples?: readonly { readonly input: I; readonly output: ZodInput<O> }[];
  readonly fallback?: "fail" | "human" | { readonly default: ZodInput<O> };
  readonly batch?: { readonly size: number; readonly by: keyof I & string };
}
function llm<C, I, O extends ZodTypeAny>(opts: LlmOpts<C, I, O>): Step<"llm", O> { return def("llm", opts); }

// ---------------------------------------------------------------------------
// http — a connector call; the broker injects the credential
// ---------------------------------------------------------------------------
export interface HttpRequest {
  readonly method: "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE";
  readonly path: string;
  readonly query?: Record<string, string | number | boolean | undefined>;
  readonly headers?: Record<string, string>;
  readonly body?: unknown;
}
export interface HttpResponse { readonly status: number; readonly headers: Record<string, string>; readonly json: any; readonly text: string }
export interface HttpOpts<C, I, O extends ZodTypeAny, R extends In<O>> extends BaseOpts {
  readonly connector: string;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly request: (input: I) => HttpRequest;
  readonly parse?: (res: HttpResponse, input: I) => R | Promise<R>;
  readonly idempotent?: boolean;
}
function http<C, I, O extends ZodTypeAny, const R extends In<O> = In<O>>(opts: HttpOpts<C, I, O, R>): Step<"http", O> { return def("http", opts); }

// ---------------------------------------------------------------------------
// mcp — a tool on a connector MCP server (HTTP or stdio)
// ---------------------------------------------------------------------------
/**
 * Tool input types of the org's declared connectors. Empty here; the generated `.cori/context.ts` merges the
 * snapshotted schemas into it (`declare module "@cori-do/sdk" { interface ConnectorTools { helpdesk: { getTicket: {...} } } }`),
 * so a `step.mcp` whose `input` does not match the tool's schema fails `check`. Undeclared tools stay open.
 */
export interface ConnectorTools {}
export type ToolInput<Cn extends string, Tn extends string> =
  Cn extends keyof ConnectorTools ? (Tn extends keyof ConnectorTools[Cn] ? ConnectorTools[Cn][Tn] : Record<string, unknown>) : Record<string, unknown>;

export interface McpOpts<C, I, O extends ZodTypeAny, R extends In<O>, Cn extends string = string, Tn extends string = string> extends BaseOpts {
  readonly connector: Cn;
  readonly tool: Tn;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly parse?: (result: unknown, input: I) => R | Promise<R>;
}
function mcp<C, Cn extends string, Tn extends string, I extends ToolInput<Cn, Tn>, O extends ZodTypeAny, const R extends In<O> = In<O>>(opts: McpOpts<C, I, O, R, Cn, Tn>): Step<"mcp", O> { return def("mcp", opts); }

// ---------------------------------------------------------------------------
// store — a typed tool or a guarded SELECT on a Cori Store
// ---------------------------------------------------------------------------
export interface StoreToolOpts<C, I, O extends ZodTypeAny, R extends In<O>> extends BaseOpts {
  readonly connector?: string;              // omitted ⇒ the app's default store
  readonly tool: string;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly parse?: (result: unknown, input: I) => R | Promise<R>;
}
export interface StoreQueryOpts<C, I, O extends ZodTypeAny, R extends In<O>> extends BaseOpts {
  readonly connector?: string;
  readonly query: { readonly text: string; readonly params: readonly string[] } | ((input: I) => { readonly text: string; readonly params: readonly unknown[] });
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly parse?: (rows: readonly Record<string, unknown>[], input: I) => R | Promise<R>;
}
function store<C, I, O extends ZodTypeAny, const R extends In<O> = In<O>>(opts: StoreToolOpts<C, I, O, R> | StoreQueryOpts<C, I, O, R>): Step<"store", O> { return def("store", opts); }

// ---------------------------------------------------------------------------
// activity — a native Temporal activity on the developers' own worker
// ---------------------------------------------------------------------------
export interface ActivityOpts<C, I, O extends ZodTypeAny> extends BaseOpts {
  readonly connector: string;
  readonly activity: string;
  readonly version: number;
  readonly input: Bound<C, I>;
  readonly output: O;
  /** Informational: the heartbeat timeout comes from the activity's declaration (activities.yaml `timeouts.heartbeat`). */
  readonly heartbeat?: boolean;
}
function activity<C, I, O extends ZodTypeAny>(opts: ActivityOpts<C, I, O>): Step<"activity", O> { return def("activity", opts); }

// ---------------------------------------------------------------------------
// cli — sandbox or your own worker only
// ---------------------------------------------------------------------------
export interface CliOpts<C, I, O extends ZodTypeAny, R extends In<O>> extends BaseOpts {
  readonly connector: string;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly command: (input: I) => readonly string[];
  readonly parse?: (stdout: string, ctx: { readonly stderr: string; readonly exitCode: number }, input: I) => R | Promise<R>;
}
function cli<C, I, O extends ZodTypeAny, const R extends In<O> = In<O>>(opts: CliOpts<C, I, O, R>): Step<"cli", O> { return def("cli", opts); }

// ---------------------------------------------------------------------------
// human — approval or input, routed
// ---------------------------------------------------------------------------
export type HumanRoute = { readonly role: string } | { readonly user: "initiator" };
export interface HumanOpts<C, I, O extends ZodTypeAny> extends BaseOpts {
  readonly kind: "approval" | "input";
  readonly to: HumanRoute;
  readonly input: Bound<C, I>;
  readonly output: O;
  readonly timeout: string;                 // "4h", "3d"
  readonly on_timeout?: "fail" | { readonly default: ZodInput<O> };
}
function human<C, I, O extends ZodTypeAny>(opts: HumanOpts<C, I, O>): Step<"human", O> { return def("human", opts); }

// ---------------------------------------------------------------------------
// workflow — child workflow
// ---------------------------------------------------------------------------
/** A sibling workflow of the same app (`workflows/<id>`), run as a child from the same version. */
export interface WorkflowOpts<C, I, O extends ZodTypeAny> extends BaseOpts {
  readonly id: string;
  readonly input: Bound<C, I>;
  readonly output: O;
}
function workflow<C, I, O extends ZodTypeAny>(opts: WorkflowOpts<C, I, O>): Step<"workflow", O> { return def("workflow", opts); }

// ---------------------------------------------------------------------------
// builtins
// ---------------------------------------------------------------------------
export interface GotoRef { readonly __cori_goto: string }
export function goto(target: string): GotoRef { return { __cori_goto: target }; }
export type Path = Step<string, ZodTypeAny> | GotoRef;

export interface BranchOpts<C, T extends Path, E extends Path | undefined> { readonly name: string; readonly description: string; readonly if: (ctx: C) => boolean; readonly then: T; readonly else?: E }
function branch<C, T extends Path, E extends Path | undefined = undefined>(opts: BranchOpts<C, T, E>): Step<"branch", never> & { readonly then: T; readonly else: E } {
  return { ...opts, ...def("branch", { name: opts.name, output: undefined as never }), then: opts.then, else: opts.else as E };
}

function switch_<C, L extends string, Cs extends Record<L, Path>, D extends Path | undefined = undefined>(opts: { readonly name: string; readonly description: string; readonly on: (ctx: C) => L; readonly cases: Cs; readonly default?: D }): Step<"switch", never> & { readonly cases: Cs; readonly default: D } {
  return { ...opts, ...def("switch", { name: opts.name, output: undefined as never }), cases: opts.cases, default: opts.default as D };
}

export interface MapOpts<C, T, O extends ZodTypeAny> {
  readonly name: string; readonly description: string;
  readonly over: (ctx: C) => readonly T[];
  readonly max_concurrency?: number;
  readonly on_item_error?: "fail_fast" | "collect";
  readonly each: Step<string, O>;
}
/** A map's output is `{ items: O[] }`; the generated context exposes `item` inside `each`. */
function map<C, T, O extends ZodTypeAny>(opts: MapOpts<C, T, O>): Step<"map", O> & { readonly each: Step<string, O>; readonly over: (ctx: C) => readonly T[] } {
  return { ...opts, ...def("map", { name: opts.name, output: opts.each.output }), each: opts.each, over: opts.over };
}

export interface LoopOpts<C, O extends ZodTypeAny> { readonly name: string; readonly description: string; readonly body: Step<string, O>; readonly until: (ctx: C) => boolean; readonly max_iterations?: number }
function loop<C, O extends ZodTypeAny>(opts: LoopOpts<C, O>): Step<"loop", O> & { readonly body: Step<string, O> } { return { ...opts, ...def("loop", { name: opts.name, output: opts.body.output }), body: opts.body }; }

/** Pauses the run for `for.timeout_ms` (at most 30 days). */
export interface WaitOpts { readonly name: string; readonly description: string; readonly for: { readonly timeout_ms?: number } }
function wait(opts: WaitOpts): Step<"wait", never> { return { ...opts, ...def("wait", { name: opts.name, output: undefined as never }) }; }

function try_<O extends ZodTypeAny, E extends Path>(opts: { readonly name: string; readonly description: string; readonly body: Step<string, O>; readonly on_error: E }): Step<"try", O> & { readonly body: Step<string, O>; readonly on_error: E } { return { ...opts, ...def("try", { name: opts.name, output: opts.body.output }), body: opts.body, on_error: opts.on_error }; }

export const step = { code, llm, http, mcp, store, activity, cli, human, workflow, branch, switch: switch_, map, loop, wait, try: try_ } as const;

/** Runtime information every context carries. */
export interface RunInfo { readonly id: string; readonly workflow: string; readonly version: string; readonly attempt: number; readonly started_at: string; readonly last_error?: { readonly message: string; readonly step: string; readonly type?: string } }
/** The caller: subject, roles and the claims allow-listed in cori.toml. */
/** A file a run was given (a `file` or `files` parameter): its reference, never its bytes. `code` steps may read these
 *  fields; an `llm` step's `files` gives the file itself to the model. */
export interface CoriFile { readonly id: string; readonly name: string; readonly size: number; readonly content_type: string; readonly sha256: string }

export interface UserInfo { readonly sub: string; readonly roles: readonly string[]; readonly email?: string; readonly [claim: string]: unknown }

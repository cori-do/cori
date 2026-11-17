import { ago, words } from "./format";
import type { ModelCall, RunSteps, Step, StepAttempt, StepDecision, StepPlan } from "./types";

// A run's steps as a person reads them: the workflow's steps in its order (those inside a condition, a choice, a repeat or a
// fallback under it), where each stands in the run, what it received and gave back, and what it does in words, from what
// its release says of it (its author's description, what it works with, the data it uses), never its code.

/** Where a step stands: `passed` while the run goes on, for a step it went past that says nothing of itself (a condition, a
 * person's step decided, one a jump skipped: the run's end tells); `not_reached`, a step the ended run never got to. */
export type StepState = "done" | "failed" | "running" | "waiting" | "skipped" | "not_reached" | "upcoming" | "passed";

/** One execution of a step run several times (per item of a repeat, per round). */
export type StepExecution = { status: string; input?: unknown; output?: unknown; error?: string };

export type StepRow = {
  name: string;
  plan?: StepPlan;
  kind: string;
  state: StepState;
  /** its entries in the run's trace: one, or one per item for a step run per item */
  trace: Step[];
  attempts: StepAttempt[];
  decision?: StepDecision;
  calls: ModelCall[];
  child?: { run_id: string; workflow: string };
  /** what it received and gave back, when the run kept them (`undefined`: not kept); `…TooLarge`: left out, in bytes */
  input?: unknown; output?: unknown; inputTooLarge?: number; outputTooLarge?: number;
  /** a step run several times: each time, when its attempts say */
  executions: StepExecution[];
  /** the way a condition, a choice or a fallback went: `then`, `else`, `cases.<label>`, `default`, `on_error` */
  path?: string;
  children: StepRow[];
};

const ok = (s: string) => s === "ok" || s === "succeeded";

/** The run's steps, in the workflow's order: from its release's steps when the server has them (`plan`), else from what
 *  the run recorded. `trace`: the run's steps as it recorded them (all of them once it ended, those done so far while it
 *  runs); `outputs`: each step's output as the run stored it, once ended. */
export function stepRows(o: { plan?: RunSteps | null; trace: Step[]; outputs?: unknown; running: boolean }): StepRow[] {
  const plan = o.plan?.steps ?? [];
  const outputs = o.outputs && typeof o.outputs === "object" ? (o.outputs as Record<string, unknown>) : null;
  const make = (name: string, p?: StepPlan): StepRow => {
    const trace = o.trace.filter((t) => t.name === name);
    const attempts = o.plan?.attempts?.[name] ?? [];
    const decision = (o.plan?.decisions ?? []).filter((d) => d.step === name).at(-1);
    const child = (o.plan?.children ?? []).filter((c) => c.step === name).at(-1);
    const done = attempts.filter((a) => ok(a.status));
    const last = attempts.at(-1);
    const lastWith = (k: "input" | "output") => [...attempts].reverse().find((a) => a[k] !== undefined);
    const row: StepRow = {
      name, plan: p, kind: p?.kind ?? trace[0]?.kind ?? "step", state: "upcoming", trace, attempts, decision,
      calls: (o.plan?.model_calls ?? []).filter((c) => c.step === name), child: child && { run_id: child.run_id, workflow: child.workflow },
      input: lastWith("input")?.input ?? (decision?.shown ?? child?.input ?? undefined),
      output: outputs && name in outputs ? outputs[name] : done.at(-1)?.output,
      inputTooLarge: last?.too_large?.input, outputTooLarge: outputs && name in outputs ? undefined : last?.too_large?.output,
      executions: done.length > 1 ? done.map((a) => ({ status: a.status, input: a.input, output: a.output, error: a.error })) : [],
      path: trace.at(-1)?.path, children: [],
    };
    return row;
  };

  // the release's steps, nested under the step that holds them; then any the run recorded that its release does not name
  const rows: StepRow[] = [];
  const all = new Map<string, StepRow>();
  for (const p of plan) {
    const row = make(p.name, p);
    all.set(p.name, row);
    const parent = p.nested_in ? all.get(p.nested_in) : undefined;
    (parent ? parent.children : rows).push(row);
  }
  for (const t of o.trace) if (!all.has(t.name)) { const row = make(t.name); all.set(t.name, row); rows.push(row); }

  const own = (r: StepRow): StepState | null => {
    if (r.decision?.status === "pending") return "waiting";
    const t = r.trace;
    if (t.length) {
      const lastT = t.at(-1)!;
      if (lastT.status === "failed") return o.running ? "running" : "failed"; // under way: another attempt may follow
      if (t.every((x) => x.status === "not_taken")) return "skipped";
      if (lastT.status === "running") return "running";
      return "done";
    }
    if (r.decision) return "done";
    return null;
  };
  const settle = (r: StepRow) => { for (const c of r.children) settle(c); const s = own(r); if (s) r.state = s; else if (r.children.some((c) => own(c))) r.state = "done"; return r; };
  rows.forEach(settle);
  for (const r of all.values()) if (!r.path && r.children.length) r.path = wentBy(r);

  if (!o.running) {
    // ended: a step without a trace entry was never reached (the run stopped first)
    const unreached = (r: StepRow, parentState?: StepState) => {
      if (!own(r) && !r.children.some((c) => own(c))) r.state = parentState === "skipped" ? "skipped" : "not_reached";
      r.children.forEach((c) => unreached(c, r.state));
    };
    rows.forEach((r) => unreached(r));
    return rows;
  }

  // under way: the steps before the furthest one with news have been passed, those after are to come, the next one runs
  const news = (r: StepRow): boolean => !!own(r) || r.children.some(news);
  const furthest = rows.reduce((k, r, i) => (news(r) ? i : k), -1);
  const busy = [...all.values()].some((r) => r.state === "waiting" || r.state === "running");
  // the steps a step holds: to come with it, else on the way it went (passed) or not (skipped)
  const mark = (x: StepRow, s: StepState) => {
    if (!news(x)) x.state = s;
    for (const c of x.children) mark(c, x.state === "running" || x.state === "upcoming" ? "upcoming" : x.path && c.plan?.slot !== x.path ? "skipped" : "passed");
  };
  rows.forEach((r, i) => mark(r, i < furthest ? "passed" : i === furthest ? r.state : i === furthest + 1 && !busy ? "running" : "upcoming"));
  return rows;
}

/** The way a step holding others went, from those that ran (a run recorded before steps said so themselves). */
function wentBy(r: StepRow): string | undefined {
  const ran = r.children.find((c) => c.trace.some((t) => t.status !== "not_taken") || c.decision);
  if (ran?.plan?.slot) return ran.plan.slot;
  if (r.kind === "branch") {
    const skipped = r.children.find((c) => c.trace.length && c.trace.every((t) => t.status === "not_taken"));
    if (skipped?.plan?.slot === "then") return "else";
    if (skipped?.plan?.slot === "else") return "then";
  }
  return undefined;
}

// ---- in words

/** A piece of a sentence: text, or a name to set apart (`b`). */
export type Part = string | { b: string };

/** What a kind of step is, in a word or two, and the icon that says it. */
export function kindOf(kind: string, p?: StepPlan): { icon: Icon; label: string } {
  switch (kind) {
    case "llm": return { icon: "ai", label: "AI model" };
    case "store": return { icon: "data", label: "Data" };
    case "mcp": case "http": return { icon: "connector", label: p?.connector ? words(p.connector) : "Connector" };
    case "activity": return { icon: "system", label: p?.connector ? words(p.connector) : "Activity" };
    case "cli": return { icon: "program", label: "Program" };
    case "human": return { icon: p?.ask === "input" ? "form" : "person", label: "Person" };
    case "workflow": return { icon: "workflow", label: "Workflow" };
    case "branch": return { icon: "condition", label: "Condition" };
    case "switch": return { icon: "choice", label: "Choice" };
    case "map": case "loop": return { icon: "repeat", label: kind === "map" ? "For each" : "Repeat" };
    case "try": return { icon: "fallback", label: "Fallback" };
    case "wait": return { icon: "pause", label: "Pause" };
    case "code": return { icon: "logic", label: "Logic" };
    default: return { icon: "step", label: words(kind) };
  }
}
export type Icon = "ai" | "data" | "connector" | "system" | "program" | "person" | "form" | "workflow" | "condition" | "choice" | "repeat" | "fallback" | "pause" | "logic" | "step";

const article = (w: string) => (/^[aeiou]/i.test(w) ? "an" : "a");
/** `createTimeEntry` → { op: "create", thing: "time entry" }. */
export function storeTool(tool?: string): { op?: "get" | "list" | "create" | "update" | "delete"; thing: string } {
  const m = /^(get|list|create|update|delete)([A-Z].*)$/.exec(tool || "");
  if (!m) return { thing: tool ? words(tool).toLowerCase() : "" };
  return { op: m[1] as "get", thing: m[2].replace(/([a-z0-9])([A-Z])/g, "$1 $2").toLowerCase() };
}
const ms = (n: number) => {
  const s = n / 1000;
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  if (s < 86400) return `${Math.round(s / 3600)} h`;
  return `${Math.round(s / 86400)} days`;
};
/** "7d" → "7 days", "4h" → "4 hours". */
export function span(t?: string): string {
  const m = /^(\d+)\s*([smhdw])$/.exec((t || "").trim());
  if (!m) return t || "";
  const n = Number(m[1]);
  const unit = { s: "second", m: "minute", h: "hour", d: "day", w: "week" }[m[2] as "s"];
  return `${n} ${unit}${n === 1 ? "" : "s"}`;
}
/** Who a person's step asks, as a phrase: "a manager", "the person who started the run". */
export function whom(to?: { role?: string; user?: string } | string | null): string {
  const role = typeof to === "string" ? (to.startsWith("user:") ? null : to) : to?.role;
  if (role) { const w = words(role).toLowerCase(); return `${article(w)} ${w}`; }
  return "the person who started the run";
}

/** What a step does, as a sentence: its mechanism in plain words (`title` names another step of the workflow); `short`, its
 *  first clause only (a list's line). */
export function action(p: StepPlan | undefined, kind: string, title: (name: string) => string, children: StepRow[] = [], short = false): Part[] {
  const full = sentence(p, kind, title, children);
  if (!short) return full;
  // up to the first `;` or `, and`: the rest says what happens when things go another way
  const out: Part[] = [];
  for (const x of full) {
    if (typeof x !== "string") { out.push(x); continue; }
    const cut = x.search(/;|, and /);
    if (cut >= 0) { if (cut > 0) out.push(x.slice(0, cut)); break; }
    out.push(x);
  }
  return out;
}

function sentence(p: StepPlan | undefined, kind: string, title: (name: string) => string, children: StepRow[]): Part[] {
  const store = p?.connector ? { b: words(p.connector) } : "the app's data";
  const slotTitle = (slot: string): Part[] | null => {
    const jump = p?.goto?.find((g) => g.slot === slot);
    if (jump) return jump.to === "end" ? ["ends the run"] : ["goes on to ", { b: title(jump.to) }];
    const c = children.find((x) => x.plan?.slot === slot);
    return c ? [{ b: title(c.name) }] : null;
  };
  switch (kind) {
    case "llm": {
      const size = p?.tier === "small" ? "a small AI model" : p?.tier === "large" ? "a large AI model" : "an AI model";
      const fb = p?.fallback === "default" ? ", and takes a default answer when its answer does not fit" : p?.fallback === "human" ? ", and asks a person when its answer does not fit" : "";
      return [`Asks ${size}${fb}`];
    }
    case "store": {
      if (p?.query) return ["Looks data up in ", store];
      const { op, thing } = storeTool(p?.tool);
      const one = `${article(thing)} ${thing}`;
      switch (op) {
        case "create": return [`Adds ${one} to `, store];
        case "update": return [`Updates ${one} in `, store];
        case "delete": return [`Deletes ${one} from `, store];
        case "get": return [`Looks up ${one} in `, store];
        case "list": return [`Lists ${thing} in `, store];
        default: return thing ? [`Uses “${thing}” in `, store] : ["Uses ", store];
      }
    }
    case "mcp": {
      const what = p?.tool ? `: ${words(p.tool).toLowerCase()}` : "";
      const where = { b: p?.connector ? words(p.connector) : "a connector" };
      return p?.access === "read" ? ["Reads from ", where, what] : ["Acts in ", where, what];
    }
    case "http": { const where = { b: p?.connector ? words(p.connector) : "a service" }; return p?.access === "read" ? ["Reads from ", where] : ["Sends to ", where]; }
    case "activity": return ["Runs ", { b: words(p?.activity || "an activity").toLowerCase() }, " on ", { b: p?.connector ? words(p.connector) : "its own system" }];
    case "cli": return ["Runs the program ", { b: p?.program || "?" }];
    case "human": {
      const who = whom(p?.to);
      const asks = p?.ask === "input" ? `Asks ${who} for information` : `Asks ${who} to approve`;
      const until = p?.timeout ? `; after ${span(p.timeout)} without an answer, ${p.on_timeout === "default" ? "it takes a default answer" : "the run stops"}` : "";
      return [asks + until];
    }
    case "workflow": return ["Runs the workflow ", { b: words(p?.workflow || "?") }];
    case "branch": {
      const then = slotTitle("then"), otherwise = slotTitle("else");
      return ["Checks a condition", ...(then ? ["; if it holds: ", ...then] : []), ...(otherwise ? ["; otherwise: ", ...otherwise] : ["; otherwise it goes on"])];
    }
    case "switch": {
      const cases = [...new Set([...children.map((c) => c.plan?.slot), ...(p?.goto || []).map((g) => g.slot)])].filter((s): s is string => !!s?.startsWith("cases.")).map((s) => s.slice(6));
      return cases.length ? [`Picks one way among: ${cases.join(", ")}`] : ["Picks one way among several"];
    }
    case "map": { const each = slotTitle("each"); return each ? ["Does ", ...each, " for each item"] : ["Does a step for each item"]; }
    case "loop": { const body = slotTitle("body"); return [...(body ? ["Repeats ", ...body] : ["Repeats a step"]), ` until done${p?.max_iterations ? ` (at most ${p.max_iterations} times)` : ""}`]; }
    case "try": { const body = slotTitle("body"), handler = slotTitle("on_error"); return [...(body ? ["Tries ", ...body] : ["Tries a step"]), ...(handler ? ["; if it fails: ", ...handler] : [])]; }
    case "wait": return p?.wait_ms ? [`Pauses for ${ms(p.wait_ms)}`] : ["Pauses"];
    case "code": return ["Works it out with the app's own logic, nothing from outside"];
    default: return [words(kind)];
  }
}

/** What a step did in the run, in a few words (none when its state says it all). `who` reads a person's id. */
export function outcome(r: StepRow, title: (name: string) => string, who: (id: string) => string): Part[] {
  const d = r.decision;
  if (r.state === "skipped") return ["Skipped: the run went another way"];
  if (r.state === "not_reached") return ["Did not run: the run stopped before it"];
  if (r.kind === "human") {
    if (d?.status === "pending") return [`Waiting for an answer, asked ${ago(d.asked_at)}`];
    const timedOut = r.trace.some((t) => (t.notes || []).some((n) => n.startsWith("timed out")));
    if (timedOut) return ["No answer in time: it took the default answer"];
    const verdict = r.output && typeof r.output === "object" && "approved" in r.output ? ((r.output as { approved?: unknown }).approved ? "Approved" : "Declined") : "Answered";
    if (d?.decided_by) return [`${verdict} by `, { b: who(d.decided_by) }];
    return r.state === "done" && verdict !== "Answered" ? [verdict] : [];
  }
  const way = r.path;
  if (r.kind === "branch" && way) {
    const jump = r.plan?.goto?.find((g) => g.slot === way);
    const held = way === "then" ? "The condition held" : "It did not hold";
    return jump ? [held, ": went on to ", { b: jump.to === "end" ? "the end" : title(jump.to) }] : [held];
  }
  if (r.kind === "switch" && way) return way === "default" ? ["No case matched: it took the default"] : ["It took ", { b: way.replace(/^cases\./, "") }];
  if (r.kind === "try" && way === "on_error") return ["It failed: the fallback ran"];
  const each = r.children[0];
  if (r.kind === "map" && r.state === "done") {
    const n = Array.isArray((r.output as { items?: unknown[] } | undefined)?.items) ? (r.output as { items: unknown[] }).items.length : each?.trace.length;
    return n !== undefined ? [`Ran for ${n} item${n === 1 ? "" : "s"}`] : [];
  }
  if (r.kind === "loop" && r.state === "done" && each?.trace.length) return [`Ran ${each.trace.length} time${each.trace.length === 1 ? "" : "s"}`];
  return [];
}

/** The data a step uses, by where it comes from: the run's parameters, an earlier step, the person who started the run,
 *  the item of a repeat. */
export function uses(p: StepPlan | undefined, title: (name: string) => string): { from: string; fields: string[] }[] {
  const groups = new Map<string, Set<string>>();
  const add = (from: string, field?: string) => { const g = groups.get(from) ?? new Set<string>(); if (field) g.add(field); groups.set(from, g); };
  for (const u of p?.uses ?? []) {
    const [root, a, b] = u.split(/[.[]/);
    if (root === "params") add("the parameters", a && words(a));
    else if (root === "steps" && a) add(title(a), b && words(b));
    else if (root === "user") add("the person who started the run", a && (a === "sub" ? "ID" : words(a)));
    else if (root === "item") add("the item", a && words(a));
    else if (root === "run") add("the run", a && words(a));
  }
  return [...groups].map(([from, f]) => ({ from, fields: [...f] }));
}

/** A failure's message without its type (`StepFailedError: …`): the type is for builders. */
export function failure(e?: string): { type?: string; message: string } {
  const m = /^([A-Z][A-Za-z0-9]*(?:Error|Exceeded|Limited|Transient|Timeout|Cancelled|Terminated)?): ([\s\S]*)$/.exec(e || "");
  return m ? { type: m[1], message: m[2] } : { message: e || "" };
}

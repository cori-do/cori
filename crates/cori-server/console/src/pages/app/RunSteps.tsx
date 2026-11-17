import { useEffect, useId, useState, type ReactNode } from "react";
import {
  Box, Check, ChevronRight, ClipboardPen, Cog, Database, GitBranch, Hourglass, LifeBuoy, Loader2, Plug, Repeat, RotateCcw, Server,
  Sparkles, Split, Terminal, Timer, UserCheck, Workflow, X,
} from "lucide-react";
import { day, duration, eur, plural, when, words } from "../../lib/format";
import { personName, usePeople } from "../../lib/people";
import { href } from "../../lib/router";
import { action, failure, kindOf, outcome, uses, whom, type Icon, type Part, type StepRow, type StepState } from "../../lib/steps";
import type { ModelCall, StepAttempt } from "../../lib/types";
import { cx, Dash, Time } from "../../ui/core";
import { Json, Payload } from "../../ui/value";

// A run's steps as a timeline a person reads: each step says what it does in words (its author's description, then its
// mechanism: "Adds a request to requests", "Asks a manager to approve") and what it did; opened, what it received and gave
// back, the person who decided, the way a condition went. Its builders also get where it is written, its attempts, the
// model calls and a query's text. Never the code.

const ICONS: Record<Icon, typeof Box> = {
  ai: Sparkles, data: Database, connector: Plug, system: Server, program: Terminal, person: UserCheck, form: ClipboardPen, workflow: Workflow,
  condition: GitBranch, choice: Split, repeat: Repeat, fallback: LifeBuoy, pause: Timer, logic: Cog, step: Box,
};

/** "Amount (€)" → "amount (€)" in a sentence; "ID" stays. */
const lower = (s: string) => (/^[A-Z][a-z]/.test(s) ? s.charAt(0).toLowerCase() + s.slice(1) : s);
/** "a, b and c". */
const listed = (xs: string[]) => (xs.length < 2 ? xs.join("") : `${xs.slice(0, -1).join(", ")} and ${xs.at(-1)}`);

/** A sentence's parts, names set apart. */
export function Said({ parts }: { parts: Part[] }) {
  return <>{parts.map((p, i) => (typeof p === "string" ? <span key={i}>{p}</span> : <span key={i} className="font-medium text-fg-2">{p.b}</span>))}</>;
}

/** The steps of a run: `open`, the step to show opened first (a link's `?step=`, else the one that failed). */
export function StepList({ rows, app, builds, running, open: first }: { rows: StepRow[]; app: string; builds: boolean; running: boolean; open?: string }) {
  usePeople();
  const titles = new Map<string, string>();
  const index = (rs: StepRow[]) => rs.forEach((r) => { titles.set(r.name, r.plan?.description || words(r.name)); index(r.children); });
  index(rows);
  const title = (name: string) => titles.get(name) || words(name);
  const [open, setOpen] = useState<Set<string>>(() => new Set(first ? [first] : []));
  useEffect(() => { if (first) setOpen((o) => (o.has(first) ? o : new Set(o).add(first))); }, [first]);
  const toggle = (name: string) => {
    const n = new Set(open);
    if (n.has(name)) n.delete(name); else n.add(name);
    setOpen(n);
    // the address names the step last opened, so that a link shows it
    const base = location.hash.split("?")[0];
    history.replaceState(null, "", `${location.pathname}${location.search}${base}${n.has(name) ? `?step=${encodeURIComponent(name)}` : ""}`);
  };
  const ctx: Ctx = { app, builds, running, title, open, toggle };
  return (
    <ol aria-label="Steps">
      {rows.map((r, i) => <StepItem key={r.name} r={r} ctx={ctx} last={i === rows.length - 1} />)}
    </ol>
  );
}

type Ctx = { app: string; builds: boolean; running: boolean; title: (name: string) => string; open: Set<string>; toggle: (name: string) => void };

const QUIET: StepState[] = ["skipped", "not_reached", "upcoming", "passed"];

function StepItem({ r, ctx, last, nested }: { r: StepRow; ctx: Ctx; last: boolean; nested?: boolean }) {
  const id = useId();
  const open = ctx.open.has(r.name);
  const kind = kindOf(r.kind, r.plan);
  const KindIcon = ICONS[kind.icon];
  const title = ctx.title(r.name);
  const did = outcome(r, ctx.title, (s) => personName(s));
  const what = action(r.plan, r.kind, ctx.title, r.children, true);
  const ms = r.trace.reduce((t, x) => t + (x.ms || 0), 0);
  const tries = Math.max(0, ...r.trace.map((t) => t.attempts ?? t.attempt ?? 0), ...r.attempts.map((a) => a.attempt ?? 0));
  const err = r.state === "failed" ? failure(r.trace.at(-1)?.error || r.attempts.at(-1)?.error) : null;
  const retrying = r.state === "running" && r.attempts.at(-1)?.status === "failed";
  const quiet = QUIET.includes(r.state);
  return (
    <li className="relative">
      {!last && <span className={cx("absolute top-8 bottom-0 w-px bg-line", nested ? "left-[11px]" : "left-[23px]")} aria-hidden />}
      <div className={cx("flex gap-3 rounded-lg py-2.5", nested ? "pr-1" : "px-3", !open && "hover:bg-surface-2/50")}>
        <StateMark state={r.state} />
        <div className="min-w-0 flex-1">
          <button type="button" aria-expanded={open} aria-controls={id} onClick={() => ctx.toggle(r.name)} className="group flex w-full cursor-pointer items-start gap-2 text-left">
            <span className="min-w-0 flex-1">
              <span className={cx("block font-medium", quiet ? "text-fg-3" : "text-fg")}>{title}</span>
              <span className="mt-0.5 flex min-w-0 items-start gap-1.5 text-[13px] text-fg-3">
                <KindIcon className="mt-[3px] size-3.5 shrink-0" aria-label={kind.label} />
                <span className="min-w-0 break-words">
                  <Said parts={what} />
                  {did.length > 0 && <> · <Said parts={did} /></>}
                  {retrying && <> · attempt {r.attempts.at(-1)?.attempt ?? 1} failed, trying again</>}
                </span>
              </span>
            </span>
            {tries > 1 && !retrying && <span className="mt-0.5 inline-flex shrink-0 items-center gap-1 text-xs text-amber-700 dark:text-amber-300" title="Attempts"><RotateCcw className="size-3" aria-hidden />{tries}</span>}
            <span className="mt-0.5 shrink-0 text-[13px] text-fg-3 tabular-nums">{ms ? duration(ms) : ""}</span>
            <ChevronRight className={cx("mt-0.5 size-4 shrink-0 text-fg-4 transition-transform group-hover:text-fg-2", open && "rotate-90")} aria-hidden />
          </button>
          {err && <div className="mt-1.5 rounded-md bg-rose-50 px-2.5 py-1.5 text-[13px] break-words whitespace-pre-wrap text-rose-800 dark:bg-rose-500/10 dark:text-rose-200">{err.message}</div>}
          {open && <div id={id}><StepPanel r={r} ctx={ctx} /></div>}
          {r.children.length > 0 && (
            <ol className="mt-1">{r.children.map((c, i) => <StepItem key={c.name} r={c} ctx={ctx} nested last={i === r.children.length - 1} />)}</ol>
          )}
        </div>
      </div>
    </li>
  );
}

function StateMark({ state }: { state: StepState }) {
  const label = { done: "Done", failed: "Failed", running: "Running", waiting: "Waiting for a person", skipped: "Skipped", not_reached: "Did not run", upcoming: "To come", passed: "Passed" }[state];
  return (
    <span role="img" aria-label={label} title={label} className={cx("relative mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full",
      state === "done" ? "bg-emerald-500 text-white" : state === "failed" ? "bg-rose-500 text-white" : state === "running" ? "bg-brand-500 text-white"
        : state === "waiting" ? "bg-amber-500 text-white" : state === "passed" ? "bg-surface-3 text-fg-3" : "bg-surface text-fg-4 ring-1 ring-line ring-inset")}>
      {state === "done" || state === "passed" ? <Check className="size-3" /> : state === "failed" ? <X className="size-3" /> : state === "running" ? <Loader2 className="size-3 animate-spin" />
        : state === "waiting" ? <Hourglass className="size-3" /> : state === "skipped" ? <span className="h-px w-2 bg-current" /> : null}
    </span>
  );
}

/** One step opened: what it does, what it uses, what happened, what it received and gave back; the technical part for builders. */
function StepPanel({ r, ctx }: { r: StepRow; ctx: Ctx }) {
  const full = action(r.plan, r.kind, ctx.title, r.children);
  const from = uses(r.plan, ctx.title);
  const ran = !["upcoming", "skipped", "not_reached", "passed"].includes(r.state) || r.input !== undefined || r.output !== undefined;
  const person = r.kind === "human";
  // a condition, a choice and a pause take and give nothing; a repeat or a fallback gives its step's output
  const flow = ["branch", "switch", "wait"].includes(r.kind), gives = ["map", "loop", "try"].includes(r.kind);
  return (
    <div className="mt-2.5 space-y-4 rounded-lg bg-surface-2/50 p-4 text-[14px] ring-1 ring-line">
      <div className="space-y-1.5">
        <p className="text-fg-2"><Said parts={full} />.</p>
        {from.length > 0 && (
          <p className="text-[13px] text-fg-3">
            Uses {from.map((g, i) => <span key={g.from}>{i > 0 && "; "}{g.fields.length ? listed(g.fields.map(lower)) : "everything"} from <span className="font-medium text-fg-2">{g.from}</span></span>)}.
          </p>
        )}
      </div>

      {person && <PersonPart r={r} />}
      {r.child && <p className="text-[13px] text-fg-3">Started <a className="link" href={href("app", ctx.app, "runs", r.child.run_id)}>a run of {words(r.child.workflow)}</a>.</p>}
      {!ran && <p className="text-[13px] text-fg-3">{r.state === "skipped" ? "Skipped: the run went another way." : r.state === "not_reached" ? "It did not run: the run stopped before it." : r.state === "passed" ? "The run went past it; what it did shows once the run ends." : "It has not run yet."}</p>}

      {ran && !flow && !gives && (
        <>
          <Data label={person ? "What they were shown" : "Received"} value={r.input} tooLarge={r.inputTooLarge} app={ctx.app} />
          <Data label={person ? "Their answer" : "Gave back"} value={r.output} tooLarge={r.outputTooLarge} pending={person && r.state === "waiting"} app={ctx.app} />
        </>
      )}
      {ran && gives && r.output !== undefined && <Data label="Gave back" value={r.output} app={ctx.app} />}
      {r.executions.length > 1 && <Executions r={r} app={ctx.app} />}
      {ctx.builds && <Technical r={r} />}
    </div>
  );
}

function Data({ label, value, tooLarge, pending, app }: { label: string; value: unknown; tooLarge?: number; pending?: boolean; app?: string }) {
  return (
    <div className="min-w-0">
      <div className="mb-1.5 text-xs font-medium tracking-wide text-fg-3 uppercase">{label}</div>
      {tooLarge ? <p className="text-[13px] text-fg-3">Too large to show here ({Math.round(tooLarge / 1024).toLocaleString()} KB).</p>
        : pending ? <p className="text-[13px] text-fg-3">No answer yet.</p>
          : value === undefined ? <p className="text-[13px] text-fg-3">Not kept for this run.</p>
            : <Payload value={value} empty="Nothing." app={app} />}
    </div>
  );
}

/** Who a person's step asked and when; who answered and when. */
function PersonPart({ r }: { r: StepRow }) {
  const d = r.decision;
  if (!d) return null;
  const later = d.decided_at && d.asked_at ? duration(Date.parse(d.decided_at) - Date.parse(d.asked_at)) : "";
  return (
    <ul className="space-y-1 text-[13px] text-fg-2">
      <li><span className="text-fg-3">Asked</span> {whom(d.to)} · <Time at={d.asked_at}>{when(d.asked_at)}</Time></li>
      {d.status === "pending" ? <li className="text-amber-700 dark:text-amber-300">Waiting for an answer</li>
        : d.decided_by ? <li><span className="text-fg-3">Answered by</span> <span className="font-medium">{personName(d.decided_by)}</span> · <Time at={d.decided_at}>{when(d.decided_at)}</Time>{later && <span className="text-fg-3"> · {later} later</span>}</li> : null}
    </ul>
  );
}

/** A step run several times: each time, its input and output. */
function Executions({ r, app }: { r: StepRow; app: string }) {
  return (
    <details className="group">
      <summary className="cursor-pointer text-[13px] font-medium text-fg-3 hover:text-fg">Each of its {plural(r.executions.length, "run")}</summary>
      <ol className="mt-2 space-y-2">
        {r.executions.map((x, i) => (
          <li key={i} className="space-y-3 rounded-lg bg-surface p-3 ring-1 ring-line">
            <Data label={`#${i + 1} received`} value={x.input} app={app} />
            <Data label="Gave back" value={x.output} app={app} />
          </li>
        ))}
      </ol>
    </details>
  );
}

/** For the app's builders: where the step is written, its attempts, notes, model calls and query. */
function Technical({ r }: { r: StepRow }) {
  const p = r.plan;
  const notes = [...new Set(r.trace.flatMap((t) => t.notes || []))];
  const cost = r.trace.reduce((t, x) => t + (x.cost_eur || 0), 0);
  const err = failure(r.trace.at(-1)?.error || r.attempts.at(-1)?.error);
  const rows: [string, ReactNode][] = [
    ["Step", <code>{r.name}</code>],
    ["Kind", <code>{r.kind}{p?.tool ? ` · ${p.tool}` : ""}{p?.method ? ` · ${p.method}` : ""}{p?.tier ? ` · ${p.tier}` : ""}</code>],
    ...(p?.file ? [["File", <code>{p.file}{p.line ? `:${p.line}` : ""}</code>] as [string, ReactNode]] : []),
    ...(err.type ? [["Error", <code>{err.type}</code>] as [string, ReactNode]] : []),
    ...(notes.length ? [["Notes", <span className="break-words">{notes.join(" · ")}</span>] as [string, ReactNode]] : []),
    ...(cost ? [["Cost", eur(cost)] as [string, ReactNode]] : []),
  ];
  return (
    <details className="group border-t border-line pt-3">
      <summary className="cursor-pointer text-[13px] font-medium text-fg-3 hover:text-fg">Technical details</summary>
      <div className="mt-3 space-y-4">
        <dl className="grid grid-cols-[minmax(5rem,auto)_1fr] gap-x-5 gap-y-1.5 text-[13px]">
          {rows.map(([k, v]) => <div key={k} className="contents"><dt className="text-fg-3">{k}</dt><dd className="min-w-0 break-words text-fg-2">{v}</dd></div>)}
        </dl>
        {r.attempts.length > 1 && <Attempts attempts={r.attempts} />}
        {r.calls.map((c, i) => <ModelCallView key={i} c={c} />)}
        {p?.query_text && <div><div className="mb-1.5 text-xs font-medium text-fg-3">Query</div><Json value={p.query_text} className="max-h-64 whitespace-pre-wrap" /></div>}
      </div>
    </details>
  );
}

function Attempts({ attempts }: { attempts: StepAttempt[] }) {
  return (
    <div>
      <div className="mb-1.5 text-xs font-medium text-fg-3">Attempts</div>
      <ol className="space-y-1 text-[13px]">
        {attempts.map((a, i) => (
          <li key={i} className="flex flex-wrap gap-x-2">
            <span className={a.status === "failed" ? "text-rose-700 dark:text-rose-300" : "text-fg-2"}>#{a.attempt ?? i + 1} {a.status === "failed" ? "failed" : "succeeded"}</span>
            <span className="text-fg-3 tabular-nums">{duration(a.ms)} · <Time at={a.at}>{day(a.at)} {new Date(a.at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</Time></span>
            {a.error && <span className="w-full font-mono text-[12px] break-words text-fg-3">{a.error}</span>}
          </li>
        ))}
      </ol>
    </div>
  );
}

function ModelCallView({ c }: { c: ModelCall }) {
  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium text-fg-3">Model call</div>
      <p className="text-[13px] text-fg-2">
        <code>{c.provider ? `${c.provider}/` : ""}{c.model}</code> <span className="text-fg-3">({c.tier}{c.mode !== "primary" ? `, ${c.mode}` : ""})</span>
        {" · "}{c.tokens_in.toLocaleString()} → {c.tokens_out.toLocaleString()} tokens · {eur(c.cost_eur) || <Dash />} · {duration(c.ms)}
        {" · "}{c.valid ? "its answer fit the expected form" : <span className="text-amber-700 dark:text-amber-300">its answer did not fit the expected form</span>}
      </p>
      {c.prompt && (
        <details>
          <summary className="cursor-pointer text-[13px] text-fg-3 hover:text-fg">What it asked the model</summary>
          <Json value={c.prompt} className="mt-2 max-h-80 whitespace-pre-wrap" />
        </details>
      )}
    </div>
  );
}

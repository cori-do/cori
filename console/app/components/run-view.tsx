// Unified run view — same component renders both the live run (events
// streamed via `subscribeRun`) and the historical trace (loaded from
// `~/.cori/runs/<key>/<utc>.json`). Picks its data source from props.
//
//   • Live mode  → `initialTrace` is undefined; we subscribe via a
//                  Tauri Channel and accumulate plan/steps until the
//                  Completed event lands, at which point we have the
//                  full trace and the view switches to the rich one.
//   • Historical → `initialTrace` is provided; state is seeded once
//                  and no subscription happens. Otherwise identical.

import { useEffect, useMemo, useReducer, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  connectCapability,
  isIpcError,
  listCapabilities,
  subscribeRun,
  type CapabilityInfo,
  type PlanStep,
  type RunEvent,
  type RunTrace,
} from "../lib/api";
import { ResultCard } from "./result-card";
import {
  formatAbsolute,
  formatCost,
  formatDuration,
  formatRelative,
} from "../lib/format";
import { openSettings } from "../lib/windows";

export interface RunViewProps {
  /** Always known: from URL in live mode, from trace.run_id in historical. */
  runId: string;
  /** Present iff historical mode. Seeds state and skips the subscription. */
  initialTrace?: RunTrace;
}

interface RunState {
  plan: PlanStep[] | null;
  /** activity_id → incremental live state; empty in historical mode. */
  steps: Record<string, LiveStep>;
  /** Populated post-Completed (live) or up-front (historical). */
  trace: RunTrace | null;
  error: string | null;
  /** True after Completed / Failed. Distinguishes "still running" from
   *  "done — no more events coming." Always true in historical mode. */
  closed: boolean;
}

interface LiveStep {
  step_name: string;
  kind?: string;
  task_queue?: string | null;
  status: "running" | "succeeded" | "failed" | "skipped" | "queued";
  duration_ms?: number;
  error?: string | null;
}

type Action =
  | { kind: "plan"; assignments: PlanStep[] }
  | {
      kind: "step_start";
      activity_id: string;
      step_name: string;
      step_kind: string;
      task_queue: string | null;
    }
  | {
      kind: "step_finish";
      activity_id: string;
      step_name: string;
      status: string;
      duration_ms: number;
      error: string | null;
    }
  | { kind: "completed"; trace: RunTrace }
  | { kind: "failed"; error: string };

function reducer(state: RunState, a: Action): RunState {
  switch (a.kind) {
    case "plan": {
      const steps: Record<string, LiveStep> = {};
      for (const s of a.assignments) {
        steps[s.activity_id] = {
          step_name: s.step_name,
          task_queue: s.task_queue,
          status: "queued",
        };
      }
      return { ...state, plan: a.assignments, steps };
    }
    case "step_start":
      return {
        ...state,
        steps: {
          ...state.steps,
          [a.activity_id]: {
            ...state.steps[a.activity_id],
            step_name: a.step_name,
            kind: a.step_kind,
            task_queue: a.task_queue,
            status: "running",
          },
        },
      };
    case "step_finish":
      return {
        ...state,
        steps: {
          ...state.steps,
          [a.activity_id]: {
            ...state.steps[a.activity_id],
            step_name: a.step_name,
            status: (a.status as LiveStep["status"]) ?? "succeeded",
            duration_ms: a.duration_ms,
            error: a.error,
          },
        },
      };
    case "completed":
      return { ...state, trace: a.trace, closed: true };
    case "failed":
      return { ...state, error: a.error, closed: true };
  }
}

function makeInitial(trace?: RunTrace): RunState {
  return {
    plan: null,
    steps: {},
    trace: trace ?? null,
    error: trace?.error ?? null,
    closed: trace != null,
  };
}

export function RunView({ runId, initialTrace }: RunViewProps) {
  const [state, dispatch] = useReducer(
    reducer,
    initialTrace,
    makeInitial,
  );

  // Live mode only: subscribe to the per-run RunChannel. Replay buffer
  // on the Rust side covers any events that fired before we attached.
  useEffect(() => {
    if (initialTrace) return; // historical mode — no subscription
    if (!runId) return;
    let cancelled = false;
    const channel = new Channel<RunEvent>();
    channel.onmessage = (ev) => {
      if (cancelled) return;
      switch (ev.type) {
        case "plan":
          dispatch({ kind: "plan", assignments: ev.assignments });
          break;
        case "step_start":
          dispatch({
            kind: "step_start",
            activity_id: ev.activity_id,
            step_name: ev.step_name,
            step_kind: ev.kind,
            task_queue: ev.task_queue,
          });
          break;
        case "step_finish":
          dispatch({
            kind: "step_finish",
            activity_id: ev.activity_id,
            step_name: ev.step_name,
            status: ev.status,
            duration_ms: ev.duration_ms,
            error: ev.error,
          });
          break;
        case "completed":
          dispatch({ kind: "completed", trace: ev.trace });
          break;
        case "failed":
          dispatch({ kind: "failed", error: ev.error });
          break;
      }
    };
    subscribeRun({ run_id: runId, on_event: channel }).catch((e) => {
      if (cancelled) return;
      dispatch({
        kind: "failed",
        error: e instanceof Error ? e.message : String(e),
      });
    });
    return () => {
      cancelled = true;
    };
  }, [runId, initialTrace]);

  const title = state.trace?.workflow_id ?? "Live run";
  const status = runStatus(state);

  return (
    <div className="run-window">
      <header className="run-window-head" data-tauri-drag-region>
        <h1>
          <span className="run-window-title">{title}</span>
          <span className={`pill ${pillFor(status)}`}>{status}</span>
        </h1>
        <div style={{ flex: 1 }} />
        <button
          type="button"
          className="btn"
          onClick={() => void openSettings("runs")}
        >
          All runs
        </button>
      </header>
      <div className="run-window-body">
        {state.trace ? (
          <TraceBody trace={state.trace} />
        ) : (
          <LiveBody
            runId={runId}
            plan={state.plan}
            steps={state.steps}
            error={state.error}
          />
        )}
      </div>
    </div>
  );
}

// ── Trace body (post-completion or historical) ───────────────────────

function TraceBody({ trace }: { trace: RunTrace }) {
  // Activity traces deliberately persist outputs and the original run
  // parameters. Each activity receives the accumulated parameters plus the
  // successful object outputs before it, so reconstructing here restores the
  // actual input without expanding the trace schema or writing another copy
  // of user data to disk.
  const activityInputs = reconstructActivityInputs(trace);
  return (
    <>
      {trace.result && (
        <ResultCard result={trace.result} partial={trace.status === "failed"} />
      )}
      <div className="card">
        <dl className="kv">
          <dt>Run id</dt>
          <dd>{trace.run_id}</dd>
          <dt>Trigger</dt>
          <dd>{trace.trigger}</dd>
          {trace.workflow_content_hash && (
            <>
              <dt>Content</dt>
              <dd>{trace.workflow_content_hash.slice(0, 12)}</dd>
            </>
          )}
          <dt>Started</dt>
          <dd>
            {formatAbsolute(trace.started_at)} ({formatRelative(trace.started_at)})
          </dd>
          <dt>Duration</dt>
          <dd>{formatDuration(trace.duration_ms)}</dd>
          {trace.cost && trace.cost.total_eur > 0 && (
            <>
              <dt>Cost</dt>
              <dd>
                {formatCost(trace.cost.total_eur)} ({trace.cost.input_tokens} in /{" "}
                {trace.cost.output_tokens} out)
              </dd>
            </>
          )}
          {trace.error && (
            <>
              <dt>Error</dt>
              <dd style={{ color: "var(--red)" }}>{trace.error}</dd>
            </>
          )}
        </dl>
        {trace.error && <ConnectOffer error={trace.error} />}
      </div>

      <h2>Steps</h2>
      {trace.activities.length === 0 ? (
        <div className="empty">No activities recorded.</div>
      ) : (
        <div className="timeline">
          {trace.activities.map((a, i) => (
            <div
              key={a.activity_id}
              className={`step ${a.status === "failed" ? "failed" : ""}`}
            >
              <div className="num">{stepNumber(i)}</div>
              <div className="step-body">
                <div className="name">{a.step_name}</div>
                <div className="meta">
                  {a.attempts > 1 ? `${a.attempts} attempts` : "1 attempt"}
                  {a.task_queue ? ` · ${a.task_queue}` : ""}
                </div>
                {a.error && (
                  <div className="meta" style={{ color: "var(--red)" }}>
                    {a.error}
                  </div>
                )}
                <ActivityInput value={activityInputs[i]} />
                <details>
                  <summary>output</summary>
                  <pre>{JSON.stringify(a.output, null, 2)}</pre>
                </details>
              </div>
              <div className={kindClass(a.kind)}>{a.kind}</div>
              <div className="right">
                <span className={`pill ${pillFor(a.status)}`}>{a.status}</span>
                <span>{formatDuration(a.duration_ms)}</span>
                {a.cost_eur != null && a.cost_eur > 0 && (
                  <span className="cost">{formatCost(a.cost_eur)}</span>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

const INPUT_PREVIEW_FIELDS = 3;
const INPUT_MAX_COLLECTION_ITEMS = 50;
const INPUT_MAX_OBJECT_FIELDS = 80;
const INPUT_MAX_STRING_CHARS = 2_000;
const SENSITIVE_INPUT_KEY = /(?:api[_-]?key|authorization|credential|password|secret|token)/i;

/** Restore the exact accumulated object supplied to each activity. This
 * mirrors `CoriWorkflow::run`: only successful object outputs move forward
 * (plus dry-run stubs). */
function reconstructActivityInputs(trace: RunTrace): unknown[] {
  const accumulated = isRecord(trace.params) ? { ...trace.params } : {};
  return trace.activities.map((activity) => {
    const input = { ...accumulated };
    const contributes =
      activity.status === "ok" || (trace.dry_run && activity.status === "skipped");
    if (contributes && isRecord(activity.output)) {
      Object.assign(accumulated, activity.output);
    }
    return input;
  });
}

function ActivityInput({ value }: { value: unknown }) {
  const display = redactInputForDisplay(value);
  const preview = inputPreview(display);
  return (
    <>
      <div className="step-input-preview" title={preview}>
        <span>input</span>
        <span>{preview}</span>
      </div>
      <details>
        <summary>view input</summary>
        <pre>{formatInputJson(display)}</pre>
      </details>
    </>
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Keep an accidental credential from becoming more visible in the app, and
 * keep a very large workflow input inspectable without making the details
 * pane unwieldy. The stored trace itself is left untouched. */
function redactInputForDisplay(value: unknown, depth = 0): unknown {
  if (depth > 8) return "… nested value omitted";
  if (typeof value === "string") {
    return value.length > INPUT_MAX_STRING_CHARS
      ? `${value.slice(0, INPUT_MAX_STRING_CHARS)}… (${value.length} characters total)`
      : value;
  }
  if (Array.isArray(value)) {
    const items = value
      .slice(0, INPUT_MAX_COLLECTION_ITEMS)
      .map((item) => redactInputForDisplay(item, depth + 1));
    if (value.length > INPUT_MAX_COLLECTION_ITEMS) {
      items.push(`… ${value.length - INPUT_MAX_COLLECTION_ITEMS} more items omitted`);
    }
    return items;
  }
  if (isRecord(value)) {
    const entries = Object.entries(value).slice(0, INPUT_MAX_OBJECT_FIELDS);
    const result: Record<string, unknown> = {};
    for (const [key, nested] of entries) {
      result[key] = SENSITIVE_INPUT_KEY.test(key)
        ? "••••••"
        : redactInputForDisplay(nested, depth + 1);
    }
    const remaining = Object.keys(value).length - entries.length;
    if (remaining > 0) result["…"] = `${remaining} more fields omitted`;
    return result;
  }
  return value;
}

function inputPreview(value: unknown): string {
  if (!isRecord(value)) return describeInputValue(value);
  const entries = Object.entries(value);
  if (entries.length === 0) return "No input values";
  const shown = entries
    .slice(0, INPUT_PREVIEW_FIELDS)
    .map(([key, nested]) => `${key}: ${describeInputValue(nested)}`);
  if (entries.length > INPUT_PREVIEW_FIELDS) {
    shown.push(`+${entries.length - INPUT_PREVIEW_FIELDS} more`);
  }
  return shown.join(" · ");
}

function describeInputValue(value: unknown): string {
  if (typeof value === "string") {
    const compact = value.replace(/\s+/g, " ");
    return `“${compact.length > 72 ? `${compact.slice(0, 72)}…` : compact}”`;
  }
  if (Array.isArray(value)) return `[${value.length} items]`;
  if (isRecord(value)) return `{${Object.keys(value).length} fields}`;
  if (value === null) return "null";
  return String(value);
}

function formatInputJson(value: unknown): string {
  const text = JSON.stringify(value, null, 2);
  return text ?? "null";
}

// ── Live body (pre-completion only) ──────────────────────────────────

interface LiveBodyProps {
  runId: string;
  plan: PlanStep[] | null;
  steps: Record<string, LiveStep>;
  error: string | null;
}

function LiveBody({ runId, plan, steps, error }: LiveBodyProps) {
  const ordered = (plan ?? []).map((p) => p.activity_id);
  const extras = Object.keys(steps).filter((id) => !ordered.includes(id));
  const hasAny = ordered.length > 0 || extras.length > 0;
  return (
    <>
      <p className="hint">
        Run id: <code>{runId}</code>
      </p>

      {error && (
        <div className="card error">
          <strong>Run failed</strong>
          <pre style={{ whiteSpace: "pre-wrap" }}>{error}</pre>
          <ConnectOffer error={error} />
        </div>
      )}

      <h2>Steps</h2>
      {!hasAny ? (
        <div className="empty">Waiting for plan…</div>
      ) : (
        <div className="timeline">
          {ordered.map((id, i) => (
            <LiveStepRow key={id} idx={i + 1} step={steps[id]} />
          ))}
          {extras.map((id, i) => (
            <LiveStepRow
              key={id}
              idx={ordered.length + i + 1}
              step={steps[id]}
            />
          ))}
        </div>
      )}
    </>
  );
}

function LiveStepRow({ idx, step }: { idx: number; step: LiveStep | undefined }) {
  if (!step) return null;
  return (
    <div className={`step ${step.status === "failed" ? "failed" : ""}`}>
      <div className="num">{stepNumber(idx - 1)}</div>
      <div className="step-body">
        <div className="name">{step.step_name}</div>
        {step.task_queue && <div className="meta">{step.task_queue}</div>}
        {step.error && (
          <div className="meta" style={{ color: "var(--red)" }}>{step.error}</div>
        )}
      </div>
      <div className={kindClass(step.kind)}>{step.kind ?? "—"}</div>
      <div className="right">
        <span className={`pill ${pillFor(step.status)}`}>{step.status}</span>
        {step.duration_ms != null && (
          <span>{formatDuration(step.duration_ms)}</span>
        )}
      </div>
    </div>
  );
}

// ── Reconnect offer (sign-in failures) ────────────────────────────────
//
// Both failure shapes name the capability:
//   preflight — "capabilities need sign-in — run `cori login <id>` and
//               try again: gws, notion"
//   mid-run   — "gws needs sign-in for user `jean` — run: cori login gws"
// Extract the ids, keep only those the Console can actually connect
// (Cori-provisioned OAuth client available), and offer the same
// one-click Connect as the Capabilities tab.

function extractCapabilityIds(error: string): string[] {
  const ids = new Set<string>();
  for (const m of error.matchAll(/cori login ([a-z0-9_-]+)/g)) {
    ids.add(m[1]);
  }
  const tail = error.match(/need sign-in[^:]*try again: (.+)$/m);
  if (tail) {
    for (const part of tail[1].split(",")) {
      const id = part.trim();
      if (/^[a-z0-9_-]+$/.test(id)) ids.add(id);
    }
  }
  return [...ids];
}

export function ConnectOffer({ error }: { error: string }) {
  const ids = useMemo(() => extractCapabilityIds(error), [error]);
  const [caps, setCaps] = useState<CapabilityInfo[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [connected, setConnected] = useState<Record<string, boolean>>({});
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    if (ids.length === 0) return;
    listCapabilities()
      .then((all) =>
        setCaps(all.filter((c) => ids.includes(c.id) && c.connectable)),
      )
      .catch(() => {});
  }, [ids]);

  if (caps.length === 0) return null;

  const connect = async (id: string) => {
    setBusy(id);
    setFailure(null);
    try {
      const updated = await connectCapability({ id });
      setConnected((d) => ({ ...d, [id]: updated.authed === true }));
    } catch (e) {
      setFailure(isIpcError(e) ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  const allConnected = caps.every((c) => connected[c.id]);

  return (
    <div style={{ marginTop: 10 }}>
      {caps.map((c) =>
        connected[c.id] ? (
          <span key={c.id} className="pill ok" style={{ marginRight: 8 }}>
            {c.display_name} connected
          </span>
        ) : (
          <button
            key={c.id}
            type="button"
            className="btn primary"
            style={{ marginRight: 8 }}
            disabled={busy !== null}
            onClick={() => void connect(c.id)}
          >
            {busy === c.id ? "Connecting…" : `Connect ${c.display_name}`}
          </button>
        ),
      )}
      {busy && (
        <p className="hint" style={{ marginBottom: 0 }}>
          Waiting for the browser sign-in to finish…
        </p>
      )}
      {allConnected && (
        <p className="hint" style={{ marginBottom: 0 }}>
          Signed in. Launch the workflow again to retry.
        </p>
      )}
      {failure && (
        <p className="hint" style={{ marginBottom: 0, color: "var(--red)" }}>
          {failure}
        </p>
      )}
    </div>
  );
}

// ── Presentation helpers ──────────────────────────────────────────────

/** `01`, `02`, … — zero-padded so the column stays one width and the
 *  step names all start on the same pixel. */
function stepNumber(index: number): string {
  return String(index + 1).padStart(2, "0");
}

/**
 * `llm` is the only kind that bills, which makes it the one thing on the
 * row worth colouring: it names itself in the number colour, the same as
 * the cost beside it.
 */
function kindClass(kind: string | undefined): string {
  return kind === "llm" ? "kind is-billed" : "kind";
}

function runStatus(s: RunState): string {
  if (s.trace) return s.trace.status;
  if (s.error) return "failed";
  if (s.closed) return "closed";
  return "running";
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  if (status === "running") return "warn";
  if (status === "skipped") return "muted";
  return "muted";
}

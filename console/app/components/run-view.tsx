// Unified run view — same component renders both the live run (events
// streamed via `subscribeRun`) and the historical trace (loaded from
// `~/.cori/runs/<key>/<utc>.json`). Picks its data source from props.
//
//   • Live mode  → `initialTrace` is undefined; we subscribe via a
//                  Tauri Channel and accumulate plan/steps/log until the
//                  Completed event lands, at which point we have the
//                  full trace and the view switches to the rich one.
//   • Historical → `initialTrace` is provided; state is seeded once
//                  and no subscription happens. Otherwise identical.
//
// The trace body is a scaled timeline plus ONE fixed inspector rail —
// selecting a step fills the rail; nothing expands, nothing shifts.

import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  connectCapability,
  isIpcError,
  listCapabilities,
  listRuns,
  stepMedians,
  subscribeRun,
  type ActivityTrace,
  type CapabilityInfo,
  type PlanStep,
  type RunEvent,
  type RunListEntry,
  type RunTrace,
  type StepMedianEntry,
} from "../lib/api";
import { ResultCard } from "./result-card";
import { CopyButton, JsonBlock, RichText, looksLikeHtml } from "./rich-content";
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
  /** Run-history directory key — enables median ticks + deltas. */
  historyKey?: string;
  /** On-disk trace path, shown (and yankable) in the header. */
  tracePath?: string;
}

interface RunState {
  plan: PlanStep[] | null;
  /** activity_id → incremental live state; empty in historical mode. */
  steps: Record<string, LiveStep>;
  /** Timestamped log lines accumulated from run events (live mode). */
  log: LogLine[];
  /** Populated post-Completed (live) or up-front (historical). */
  trace: RunTrace | null;
  error: string | null;
  /** True after Completed / Failed. Always true in historical mode. */
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

interface LogLine {
  ts: string;
  text: string;
  tone: "info" | "note" | "error";
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
      notes: string[];
    }
  | { kind: "completed"; trace: RunTrace }
  | { kind: "failed"; error: string };

function now(): string {
  return new Date().toISOString().slice(11, 19);
}

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
      return {
        ...state,
        plan: a.assignments,
        steps,
        log: [
          ...state.log,
          {
            ts: now(),
            text: `plan · ${a.assignments.length} steps`,
            tone: "info",
          },
        ],
      };
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
        log: [
          ...state.log,
          {
            ts: now(),
            text: `▶ ${a.step_name} (${a.step_kind}${a.task_queue ? ` · ${a.task_queue}` : ""})`,
            tone: "info",
          },
        ],
      };
    case "step_finish": {
      const mark = a.status === "failed" ? "✗" : "✓";
      const lines: LogLine[] = [
        {
          ts: now(),
          text: `${mark} ${a.step_name} · ${a.status} · ${formatDuration(a.duration_ms)}`,
          tone: a.status === "failed" ? "error" : "info",
        },
        ...a.notes.map(
          (n): LogLine => ({ ts: now(), text: `  ${n}`, tone: "note" }),
        ),
      ];
      if (a.error) {
        lines.push({ ts: now(), text: `  ${a.error}`, tone: "error" });
      }
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
        log: [...state.log, ...lines],
      };
    }
    case "completed":
      return {
        ...state,
        trace: a.trace,
        closed: true,
        log: [...state.log, { ts: now(), text: "run completed", tone: "info" }],
      };
    case "failed":
      return {
        ...state,
        error: a.error,
        closed: true,
        log: [...state.log, { ts: now(), text: a.error, tone: "error" }],
      };
  }
}

function makeInitial(trace?: RunTrace): RunState {
  return {
    plan: null,
    steps: {},
    log: [],
    trace: trace ?? null,
    error: trace?.error ?? null,
    closed: trace != null,
  };
}

export function RunView({
  runId,
  initialTrace,
  historyKey,
  tracePath,
}: RunViewProps) {
  const [state, dispatch] = useReducer(reducer, initialTrace, makeInitial);

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
            notes: ev.notes ?? [],
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
          {(state.trace?.dry_run ?? false) && (
            <span className="pill muted" title="Dry run — external steps stubbed">
              dry run
            </span>
          )}
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
          <TraceBody
            trace={state.trace}
            historyKey={historyKey}
            tracePath={tracePath}
          />
        ) : (
          <LiveBody
            runId={runId}
            plan={state.plan}
            steps={state.steps}
            log={state.log}
            error={state.error}
          />
        )}
      </div>
    </div>
  );
}

// ── Trace body (post-completion or historical) ───────────────────────

function TraceBody({
  trace,
  historyKey,
  tracePath,
}: {
  trace: RunTrace;
  historyKey?: string;
  tracePath?: string;
}) {
  // Per-step medians + run-level history, for ticks and deltas. Both
  // are best-effort: without a history key the timeline still scales
  // to this run's own durations.
  const [medians, setMedians] = useState<Record<string, StepMedianEntry>>({});
  const [history, setHistory] = useState<RunListEntry[]>([]);
  useEffect(() => {
    if (!historyKey) return;
    let cancelled = false;
    stepMedians({ history_key: historyKey })
      .then((rows) => {
        if (cancelled) return;
        const map: Record<string, StepMedianEntry> = {};
        for (const r of rows) map[r.activity_id] = r;
        setMedians(map);
      })
      .catch(() => {});
    listRuns({ history_key: historyKey, limit: 20 })
      .then((rows) => !cancelled && setHistory(rows))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [historyKey]);

  // One inspector, not ten toggles: the selected step fills the rail.
  // Default to the failed step, else the most expensive one.
  const defaultSelection = useMemo(() => {
    const failed = trace.activities.find((a) => a.status === "failed");
    if (failed) return failed.activity_id;
    let best: ActivityTrace | null = null;
    for (const a of trace.activities) {
      if (!best || a.duration_ms > best.duration_ms) best = a;
    }
    return best?.activity_id ?? null;
  }, [trace]);
  const [selected, setSelected] = useState<string | null>(null);
  const selectedId = selected ?? defaultSelection;
  const selectedIndex = trace.activities.findIndex(
    (a) => a.activity_id === selectedId,
  );
  const activityInputs = useMemo(
    () => reconstructActivityInputs(trace),
    [trace],
  );

  const baseline = runMedian(history, trace.run_id);
  const durationDelta =
    !trace.dry_run && trace.status === "succeeded"
      ? deltaVsMedian(trace.duration_ms, baseline.durationMs, 500)
      : null;
  const costDelta =
    !trace.dry_run && trace.status === "succeeded" && trace.cost
      ? deltaVsMedian(trace.cost.total_eur, baseline.costEur, 0.005)
      : null;

  // Bars scale against the slowest of (this run's steps, their medians),
  // so a step far over its median visibly overshoots the tick.
  const scaleMax = Math.max(
    1,
    ...trace.activities.map((a) => a.duration_ms),
    ...trace.activities.map((a) => medians[a.activity_id]?.median_ms ?? 0),
  );

  return (
    <>
      {/* Identity strip: ids last, one line, no card. */}
      <div className="trace-identity">
        <span title="Run id">{trace.run_id}</span>
        <span className="sep">·</span>
        <span>{trace.trigger}</span>
        {trace.workflow_content_hash && (
          <>
            <span className="sep">·</span>
            <span title={`Workflow content hash ${trace.workflow_content_hash}`}>
              content {trace.workflow_content_hash.slice(0, 8)}
            </span>
          </>
        )}
        <span className="sep">·</span>
        <span title={formatAbsolute(trace.started_at)}>
          {formatRelative(trace.started_at)}
        </span>
        {tracePath && (
          <>
            <span className="sep">·</span>
            <button
              type="button"
              className="trace-path"
              title={`${tracePath} — click to copy`}
              onClick={() =>
                void navigator.clipboard?.writeText(tracePath).catch(() => {})
              }
            >
              {tracePath}
            </button>
          </>
        )}
      </div>

      {/* The headline numbers, each against this workflow's median. */}
      <div className="trace-deltas">
        <span className="trace-delta-item">
          {formatDuration(trace.duration_ms)}
          {durationDelta != null && (
            <DeltaChip delta={durationDelta} format={formatDuration} />
          )}
        </span>
        {trace.cost && trace.cost.total_eur > 0 && (
          <span className="trace-delta-item">
            {formatCost(trace.cost.total_eur)}
            {costDelta != null && (
              <DeltaChip delta={costDelta} format={formatCost} />
            )}
          </span>
        )}
        <span className="trace-delta-item">
          {trace.activities.length} steps
        </span>
        {baseline.samples >= 3 && (
          <span className="trace-delta-note">
            vs median of {baseline.samples} real runs
          </span>
        )}
      </div>

      {trace.error && (
        <div className="card error">
          <strong>Run failed</strong>
          <pre style={{ whiteSpace: "pre-wrap" }}>{trace.error}</pre>
          <ConnectOffer error={trace.error} />
        </div>
      )}

      {trace.result && (
        <ResultCard result={trace.result} partial={trace.status === "failed"} />
      )}

      <h2>Steps</h2>
      {trace.activities.length === 0 ? (
        <div className="empty">No activities recorded.</div>
      ) : (
        <div className="trace-layout">
          <div className="trace-timeline" role="listbox" aria-label="Steps">
            {trace.activities.map((a, i) => (
              <TimelineRow
                key={a.activity_id}
                index={i}
                activity={a}
                median={medians[a.activity_id]}
                scaleMax={scaleMax}
                selected={a.activity_id === selectedId}
                onSelect={() => setSelected(a.activity_id)}
              />
            ))}
          </div>
          <StepInspector
            activity={
              selectedIndex >= 0 ? trace.activities[selectedIndex] : null
            }
            median={selectedId ? medians[selectedId] : undefined}
            input={selectedIndex >= 0 ? activityInputs[selectedIndex] : null}
          />
        </div>
      )}
    </>
  );
}

function TimelineRow({
  index,
  activity: a,
  median,
  scaleMax,
  selected,
  onSelect,
}: {
  index: number;
  activity: ActivityTrace;
  median: StepMedianEntry | undefined;
  scaleMax: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const width = Math.max(1.5, (a.duration_ms / scaleMax) * 100);
  const tick =
    median && median.samples >= 3
      ? Math.min(100, (median.median_ms / scaleMax) * 100)
      : null;
  return (
    <button
      type="button"
      className={`trace-row${selected ? " is-selected" : ""}${a.status === "failed" ? " is-failed" : ""}`}
      role="option"
      aria-selected={selected}
      onClick={onSelect}
    >
      <span className="trace-row-num">{stepNumber(index)}</span>
      <span className="trace-row-name">{a.step_name}</span>
      <span className="trace-row-track" aria-hidden>
        <span className="trace-row-bar" style={{ width: `${width}%` }} />
        {tick != null && median && (
          <span
            className="trace-row-tick"
            style={{ left: `${tick}%` }}
            title={`median ${formatDuration(median.median_ms)} over ${median.samples} runs`}
          />
        )}
      </span>
      <span className="trace-row-duration">
        {formatDuration(a.duration_ms)}
      </span>
      <span className={kindClass(a.kind)}>{a.kind}</span>
      <span
        className={`trace-row-mark ${a.status === "failed" ? "is-bad" : a.status === "ok" ? "is-ok" : "is-muted"}`}
      >
        {a.status === "failed" ? "✗" : a.status === "ok" ? "✓" : "–"}
      </span>
    </button>
  );
}

/**
 * The fixed right rail. Selecting a step fills it — no accordions, no
 * layout shift. Shows the numbers, the manifest/broker note, and the
 * exact input/output the activity saw.
 */
function StepInspector({
  activity: a,
  median,
  input,
}: {
  activity: ActivityTrace | null;
  median: StepMedianEntry | undefined;
  input: unknown;
}) {
  if (!a) {
    return (
      <aside className="trace-inspector">
        <div className="empty">Select a step.</div>
      </aside>
    );
  }
  const display = redactInputForDisplay(input);
  return (
    <aside className="trace-inspector">
      <div className="trace-inspector-title">
        <span className="trace-inspector-name">{a.step_name}</span>
        <span className={`pill ${pillFor(a.status)}`}>{a.status}</span>
      </div>
      <dl className="kv">
        <dt>Kind</dt>
        <dd>{a.kind}</dd>
        <dt>Duration</dt>
        <dd>
          {formatDuration(a.duration_ms)}
          {median && median.samples >= 3 && (
            <span className="trace-inspector-median">
              {" "}
              · p50 {formatDuration(median.median_ms)}
            </span>
          )}
        </dd>
        <dt>Attempts</dt>
        <dd>{a.attempts}</dd>
        {a.task_queue && (
          <>
            <dt>Queue</dt>
            <dd>{a.task_queue}</dd>
          </>
        )}
        {a.cost_eur != null && a.cost_eur > 0 && (
          <>
            <dt>Cost</dt>
            <dd>
              {formatCost(a.cost_eur)}
              {a.tokens
                ? ` (${a.tokens.input_tokens} in / ${a.tokens.output_tokens} out)`
                : ""}
            </dd>
          </>
        )}
      </dl>
      {a.error && <div className="trace-inspector-error">{a.error}</div>}
      {a.notes && (
        <div className="trace-inspector-note" title="Manifest / broker note">
          {a.notes}
        </div>
      )}
      <div className="trace-inspector-block">
        <span className="label">Input</span>
        <div className="step-input-preview" title={inputPreview(display)}>
          <span>{inputPreview(display)}</span>
        </div>
        <details>
          <summary>full input</summary>
          <JsonBlock value={display} />
        </details>
      </div>
      <StepOutput output={a.output} />
    </aside>
  );
}

/**
 * Step output, rendered by shape. A bare string (or a long string field
 * inside an object — the usual LLM-step shape) gets the rich renderer:
 * markdown as prose, HTML in the sandboxed frame. The raw JSON stays a
 * click away, now with a copy button.
 */
function StepOutput({ output }: { output: unknown }) {
  if (typeof output === "string") {
    return (
      <div className="trace-inspector-block">
        <span className="label">
          Output <CopyButton text={output} />
        </span>
        <div className="trace-inspector-rich">
          <RichText value={output} />
        </div>
      </div>
    );
  }
  const rich = isRecord(output)
    ? Object.entries(output).filter(
        ([, v]) =>
          typeof v === "string" && (v.length >= 160 || looksLikeHtml(v)),
      )
    : [];
  return (
    <div className="trace-inspector-block">
      <span className="label">Output</span>
      {rich.map(([key, value], index) => (
        <details key={key} className="trace-inspector-rendered" open={index === 0}>
          <summary>{key} · rendered</summary>
          <div className="trace-inspector-rich">
            <RichText value={value as string} />
          </div>
        </details>
      ))}
      <JsonBlock value={output} className="trace-inspector-output" />
    </div>
  );
}

// ── Median helpers ────────────────────────────────────────────────────

function runMedian(
  history: RunListEntry[],
  excludeRunId: string,
): { durationMs: number | null; costEur: number | null; samples: number } {
  const real = history.filter(
    (h) => !h.dry_run && h.status === "succeeded" && h.run_id !== excludeRunId,
  );
  if (real.length < 3) {
    return { durationMs: null, costEur: null, samples: real.length };
  }
  const med = (ns: number[]): number => {
    const s = [...ns].sort((a, b) => a - b);
    const mid = Math.floor(s.length / 2);
    return s.length % 2 === 1 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
  };
  return {
    durationMs: med(real.map((h) => h.duration_ms)),
    costEur: med(real.map((h) => h.cost.total_eur)),
    samples: real.length,
  };
}

function deltaVsMedian(
  actual: number,
  median: number | null,
  minAbs: number,
): number | null {
  if (median == null) return null;
  const delta = actual - median;
  if (Math.abs(delta) < Math.max(median * 0.05, minAbs)) return null;
  return delta;
}

function DeltaChip({
  delta,
  format,
}: {
  delta: number;
  format: (n: number) => string;
}) {
  const over = delta > 0;
  return (
    <span
      className={`pane-delta ${over ? "is-over" : "is-under"}`}
      title="vs this workflow's median (real runs)"
    >
      {over ? "+" : "−"}
      {format(Math.abs(delta))}
    </span>
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

// ── Live body (pre-completion only) ──────────────────────────────────
//
// A fixed step table on top — rows never move — and the raw event log
// underneath, dark, following by default. Filtering is a plain text
// box; grep-and-follow must survive the GUI.

interface LiveBodyProps {
  runId: string;
  plan: PlanStep[] | null;
  steps: Record<string, LiveStep>;
  log: LogLine[];
  error: string | null;
}

function LiveBody({ runId, plan, steps, log, error }: LiveBodyProps) {
  const ordered = (plan ?? []).map((p) => p.activity_id);
  const extras = Object.keys(steps).filter((id) => !ordered.includes(id));
  const hasAny = ordered.length > 0 || extras.length > 0;
  const [filter, setFilter] = useState("");
  const [follow, setFollow] = useState(true);
  const logRef = useRef<HTMLDivElement>(null);

  const visible = filter.trim()
    ? log.filter((l) => l.text.toLowerCase().includes(filter.toLowerCase()))
    : log;

  useEffect(() => {
    if (!follow) return;
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [visible.length, follow]);

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

      <div className="run-log">
        <div className="run-log-bar">
          <span className="label">Log</span>
          <input
            type="text"
            className="run-log-filter"
            placeholder="filter…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            spellCheck={false}
          />
          <label className="run-log-follow">
            <input
              type="checkbox"
              checked={follow}
              onChange={(e) => setFollow(e.target.checked)}
            />
            follow
          </label>
        </div>
        <div
          className="run-log-pane"
          ref={logRef}
          onScroll={(e) => {
            const el = e.currentTarget;
            const atBottom =
              el.scrollHeight - el.scrollTop - el.clientHeight < 12;
            // Scrolling up pauses follow; returning to the bottom resumes.
            setFollow(atBottom);
          }}
        >
          {visible.length === 0 ? (
            <div className="run-log-empty">
              {log.length === 0 ? "No events yet." : "No line matches."}
            </div>
          ) : (
            visible.map((l, i) => (
              <div key={i} className={`run-log-line is-${l.tone}`}>
                <span className="run-log-ts">{l.ts}</span>
                <span className="run-log-text">{l.text}</span>
              </div>
            ))
          )}
        </div>
      </div>
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
  if (status === "succeeded" || status === "ok") return "ok";
  if (status === "failed") return "bad";
  if (status === "running") return "warn";
  if (status === "skipped") return "muted";
  return "muted";
}

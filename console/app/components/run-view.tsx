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
import {
  formatAbsolute,
  formatCost,
  formatDuration,
  formatRelative,
} from "../lib/format";
import { openManage } from "../lib/windows";

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
        <h1 style={{ margin: 0 }}>
          {title}{" "}
          <span className={`pill ${pillFor(status)}`}>{status}</span>
        </h1>
        <div style={{ flex: 1 }} />
        <button
          type="button"
          className="btn"
          onClick={() => void openManage("runs")}
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
  return (
    <>
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
              <div className="num">{i + 1}.</div>
              <div>
                <div className="name">{a.step_name}</div>
                <div className="meta">
                  {a.kind}
                  {" · "}
                  {a.attempts > 1 ? `${a.attempts} attempts` : "1 attempt"}
                  {a.task_queue ? ` · ${a.task_queue}` : ""}
                  {" · "}
                  {formatDuration(a.duration_ms)}
                </div>
                {a.error && (
                  <div className="meta" style={{ color: "var(--red)" }}>
                    {a.error}
                  </div>
                )}
                <details>
                  <summary>input</summary>
                  <pre>{JSON.stringify(a.input_summary, null, 2)}</pre>
                </details>
                <details>
                  <summary>output</summary>
                  <pre>{JSON.stringify(a.output, null, 2)}</pre>
                </details>
              </div>
              <div className="right">
                <div>
                  <span className={`pill ${pillFor(a.status)}`}>{a.status}</span>
                </div>
                {a.cost_eur != null && a.cost_eur > 0 && (
                  <div>{formatCost(a.cost_eur)}</div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
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
      <div className="num">{idx}.</div>
      <div>
        <div className="name">{step.step_name}</div>
        <div className="meta">
          {step.kind ?? "—"}
          {step.task_queue ? ` · ${step.task_queue}` : ""}
        </div>
        {step.error && (
          <div className="meta" style={{ color: "var(--red)" }}>{step.error}</div>
        )}
      </div>
      <div className="right">
        <div>
          <span className={`pill ${pillFor(step.status)}`}>{step.status}</span>
        </div>
        {step.duration_ms != null && <div>{formatDuration(step.duration_ms)}</div>}
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

function ConnectOffer({ error }: { error: string }) {
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

// ── Status helpers ────────────────────────────────────────────────────

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

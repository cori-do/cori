import { useEffect, useReducer, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { apiGet, type PlanStep, type RunEvent, type RunListEntry, type RunTrace } from "../lib/api";
import { formatCost, formatDuration } from "../lib/format";

interface LiveState {
  plan: PlanStep[] | null;
  /** activity_id → live status */
  steps: Record<string, LiveStep>;
  finalTrace: RunTrace | null;
  error: string | null;
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
  | { kind: "failed"; error: string }
  | { kind: "closed" };

function reducer(state: LiveState, a: Action): LiveState {
  switch (a.kind) {
    case "plan": {
      const steps: Record<string, LiveStep> = {};
      for (const s of a.assignments) {
        steps[s.activity_id] = { step_name: s.step_name, task_queue: s.task_queue, status: "queued" };
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
      return { ...state, finalTrace: a.trace, closed: true };
    case "failed":
      return { ...state, error: a.error, closed: true };
    case "closed":
      return { ...state, closed: true };
  }
}

const INITIAL: LiveState = {
  plan: null,
  steps: {},
  finalTrace: null,
  error: null,
  closed: false,
};

export function meta() {
  return [{ title: "Live run — Cori Console" }];
}

export default function RunLive() {
  const { runId } = useParams();
  const [state, dispatch] = useReducer(reducer, INITIAL);
  const esRef = useRef<EventSource | null>(null);
  const [detailLink, setDetailLink] = useState<string | null>(null);

  // After the run lands a trace on disk, find its run-history key +
  // filename so we can deep-link to the historical detail view.
  useEffect(() => {
    if (!runId || !state.finalTrace) return;
    let cancelled = false;
    const wf = state.finalTrace.workflow_id;
    const qs = new URLSearchParams({ workflow_id: wf, limit: "50" });
    apiGet<RunListEntry[]>(`/api/runs?${qs.toString()}`)
      .then((rows) => {
        if (cancelled) return;
        const hit = rows.find((r) => r.run_id === runId);
        if (hit) {
          setDetailLink(
            `/runs/${encodeURIComponent(hit.key)}/${encodeURIComponent(hit.utc)}`,
          );
        }
      })
      .catch(() => {
        /* non-fatal — the user can navigate via /runs */
      });
    return () => {
      cancelled = true;
    };
  }, [runId, state.finalTrace]);

  useEffect(() => {
    if (!runId) return;
    const es = new EventSource(`/api/runs/${encodeURIComponent(runId)}/stream`, {
      withCredentials: true,
    });
    esRef.current = es;

    function on(name: string, handler: (ev: RunEvent) => void) {
      es.addEventListener(name, (e) => {
        try {
          const data = JSON.parse((e as MessageEvent).data) as RunEvent;
          handler(data);
        } catch (err) {
          console.error(`bad SSE payload on ${name}:`, err);
        }
      });
    }

    on("plan", (ev) => {
      if (ev.type === "plan") dispatch({ kind: "plan", assignments: ev.assignments });
    });
    on("step_start", (ev) => {
      if (ev.type === "step_start")
        dispatch({
          kind: "step_start",
          activity_id: ev.activity_id,
          step_name: ev.step_name,
          step_kind: ev.kind,
          task_queue: ev.task_queue,
        });
    });
    on("step_finish", (ev) => {
      if (ev.type === "step_finish")
        dispatch({
          kind: "step_finish",
          activity_id: ev.activity_id,
          step_name: ev.step_name,
          status: ev.status,
          duration_ms: ev.duration_ms,
          error: ev.error,
        });
    });
    on("completed", (ev) => {
      if (ev.type === "completed") dispatch({ kind: "completed", trace: ev.trace });
      es.close();
    });
    on("failed", (ev) => {
      if (ev.type === "failed") dispatch({ kind: "failed", error: ev.error });
      es.close();
    });

    es.onerror = () => {
      // Browser auto-reconnects unless we close. The stream-closed
      // case is already handled in `completed`/`failed`.
      if (state.closed) es.close();
    };

    return () => es.close();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId]);

  const plan = state.plan ?? [];
  const orderedActivityIds = plan.map((p) => p.activity_id);
  const anyExtras = Object.keys(state.steps).filter(
    (id) => !orderedActivityIds.includes(id),
  );

  return (
    <>
      <p className="hint">
        <Link to="/run">← run another</Link> · <Link to="/runs">all runs</Link>
      </p>
      <h1>
        Live run{" "}
        <span className={`pill ${runPill(state)}`}>{runLabel(state)}</span>
      </h1>
      <p className="hint">
        Run id: <code>{runId}</code>
      </p>

      {state.error && (
        <div className="card error">
          <strong>Run failed</strong>
          <pre style={{ whiteSpace: "pre-wrap" }}>{state.error}</pre>
        </div>
      )}

      <h2>Steps</h2>
      {plan.length === 0 && Object.keys(state.steps).length === 0 ? (
        <div className="empty">Waiting for plan…</div>
      ) : (
        <div className="timeline">
          {orderedActivityIds.map((id, i) => (
            <LiveStepRow key={id} idx={i + 1} step={state.steps[id]} />
          ))}
          {anyExtras.map((id, i) => (
            <LiveStepRow
              key={id}
              idx={orderedActivityIds.length + i + 1}
              step={state.steps[id]}
            />
          ))}
        </div>
      )}

      {state.finalTrace && (
        <>
          {detailLink ? (
            <p className="hint" style={{ marginTop: 24 }}>
              <Link to={detailLink}>Open full trace →</Link>
            </p>
          ) : (
            <p className="hint" style={{ marginTop: 24 }}>
              Full trace persisted to <code>~/.cori/runs/</code>; visible in{" "}
              <Link to="/runs">Runs</Link>.
            </p>
          )}

          <h2>Activity details</h2>
          <div className="timeline">
            {state.finalTrace.activities.map((a, i) => (
              <div
                key={a.activity_id}
                className={`step ${a.status === "failed" ? "failed" : ""}`}
              >
                <div className="num">{i + 1}.</div>
                <div>
                  <div className="name">{a.step_name}</div>
                  <div className="meta">
                    {a.kind} · {a.duration_ms}ms · {a.attempts} attempt
                    {a.attempts === 1 ? "" : "s"}
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
                    <span className={`pill ${stepPill(a.status)}`}>{a.status}</span>
                  </div>
                  {a.cost_eur != null && a.cost_eur > 0 && (
                    <div>{formatCost(a.cost_eur)}</div>
                  )}
                </div>
              </div>
            ))}
          </div>
        </>
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
          <span className={`pill ${stepPill(step.status)}`}>{step.status}</span>
        </div>
        {step.duration_ms != null && <div>{formatDuration(step.duration_ms)}</div>}
      </div>
    </div>
  );
}

function runLabel(s: LiveState): string {
  if (s.error) return "failed";
  if (s.finalTrace) return s.finalTrace.status;
  if (s.closed) return "closed";
  return "running";
}

function runPill(s: LiveState): string {
  const l = runLabel(s);
  if (l === "succeeded") return "ok";
  if (l === "failed") return "bad";
  if (l === "running") return "warn";
  return "muted";
}

function stepPill(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  if (status === "running") return "warn";
  return "muted";
}


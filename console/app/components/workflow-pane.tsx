// The launcher's right pane: the workflow you picked, and running it.
//
// One window is the whole product — picking a workflow on the left does
// not open a second window, it fills this pane. The pane resolves the
// source, shows what the workflow was compiled into, takes its
// parameters, and then runs it in place, with the compiled steps landing
// one by one under a clock that is the real one.
//
// Runs started elsewhere (an agent over `cori mcp`, a schedule, the CLI)
// still open their own run window — those are not the workflow *you*
// picked, so they do not belong in this pane.

import {
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type Ref,
} from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  isIpcError,
  recordTrust,
  resolveWorkflow,
  startRun,
  type ConsentRequired,
  type ParameterDef,
  type PlanStep,
  type RunEvent,
  type RunTrace,
  type WorkflowPreflight,
} from "../lib/api";
import { formatCost, formatDuration } from "../lib/format";
import { ConnectOffer } from "./run-view";

/** What the launcher can ask of the pane from its own key handling. */
export interface WorkflowPaneHandle {
  /** True when a second Enter should start the run rather than re-resolve. */
  canRun(): boolean;
  run(): void;
}

interface LiveStep {
  step_name: string;
  kind?: string;
  status: "queued" | "running" | "succeeded" | "failed" | "skipped";
  duration_ms?: number;
  error?: string | null;
}

interface RunState {
  runId: string | null;
  /** activity_ids in plan order — the order the steps are shown in. */
  order: string[];
  steps: Record<string, LiveStep>;
  trace: RunTrace | null;
  error: string | null;
  closed: boolean;
}

type Phase = "idle" | "loading" | "error" | "ready";

export function WorkflowPane({
  source,
  handleRef,
}: {
  /** `cori run`-compatible source string, or null when nothing is picked. */
  source: string | null;
  handleRef?: Ref<WorkflowPaneHandle>;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [preflight, setPreflight] = useState<WorkflowPreflight | null>(null);
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [error, setError] = useState<string | null>(null);
  const [consent, setConsent] = useState<ConsentRequired | null>(null);
  const [trusting, setTrusting] = useState(false);
  const [run, setRun] = useState<RunState | null>(null);

  // Only the newest resolve is allowed to land: arrowing down a long list
  // and pressing Enter twice must not let a slow first request overwrite
  // the pane with a workflow the user has already moved off.
  const requestId = useRef(0);

  const resolve = useCallback(async (src: string, update = false) => {
    const id = ++requestId.current;
    setPhase("loading");
    setError(null);
    setConsent(null);
    setPreflight(null);
    setRun(null);
    try {
      const pf = await resolveWorkflow({ source: src, update });
      if (id !== requestId.current) return;
      const defaults: Record<string, unknown> = {};
      for (const p of pf.manifest.parameters) {
        if (p.default !== undefined && p.default !== null) {
          defaults[p.name] = p.default;
        }
      }
      setPreflight(pf);
      setParams(defaults);
      setPhase("ready");
    } catch (e: unknown) {
      if (id !== requestId.current) return;
      if (isIpcError(e) && e.code === "consent_required") {
        setConsent(e.details as ConsentRequired);
        setPhase("error");
      } else {
        setError(formatErr(e));
        setPhase("error");
      }
    }
  }, []);

  useEffect(() => {
    if (!source) {
      requestId.current += 1; // drop anything in flight
      setPhase("idle");
      setPreflight(null);
      setRun(null);
      setError(null);
      setConsent(null);
      return;
    }
    void resolve(source);
  }, [source, resolve]);

  const running = run != null && !run.closed;

  const startWorkflow = useCallback(() => {
    if (!source || !preflight || running) return;
    setRun({
      runId: null,
      order: [],
      steps: {},
      trace: null,
      error: null,
      closed: false,
    });

    const channel = new Channel<RunEvent>();
    channel.onmessage = (ev) => {
      setRun((prev) => (prev ? reduceRun(prev, ev) : prev));
    };

    startRun({ source, params, dry_run: false, on_event: channel })
      .then(({ run_id }) =>
        setRun((prev) => (prev ? { ...prev, runId: run_id } : prev)),
      )
      .catch((e: unknown) => {
        if (isIpcError(e) && e.code === "consent_required") {
          setConsent(e.details as ConsentRequired);
          setRun(null);
          return;
        }
        setRun((prev) =>
          prev ? { ...prev, error: formatErr(e), closed: true } : prev,
        );
      });
  }, [source, preflight, params, running]);

  useImperativeHandle(
    handleRef,
    () => ({
      canRun: () =>
        phase === "ready" &&
        preflight != null &&
        preflight.ready &&
        !preflight.has_builtin_step &&
        !running,
      run: startWorkflow,
    }),
    [phase, preflight, running, startWorkflow],
  );

  async function trustAndRetry() {
    if (!consent || !source) return;
    setTrusting(true);
    try {
      await recordTrust({
        host: consent.host,
        repo: consent.repo,
        subpath: consent.subpath,
        ref_str: consent.ref_str,
        sha: consent.sha,
      });
      setConsent(null);
      await resolve(source);
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setTrusting(false);
    }
  }

  if (!source) return <PaneEmpty />;

  return (
    <div className="pane">
      {consent && (
        <ConsentModal
          consent={consent}
          submitting={trusting}
          onTrust={() => void trustAndRetry()}
          onCancel={() => setConsent(null)}
        />
      )}

      {phase === "loading" && (
        <div className="pane-status">Resolving {source}…</div>
      )}

      {phase === "error" && !consent && (
        <div className="pane-status is-error">{error ?? "Could not resolve."}</div>
      )}

      {preflight && (
        <>
          <PaneHeader
            preflight={preflight}
            run={run}
            onRun={startWorkflow}
            running={running}
          />

          {preflight.manifest.description && (
            <p className="pane-desc">{preflight.manifest.description}</p>
          )}

          {preflight.has_builtin_step && (
            <p className="pane-note is-warn">
              Uses a <code>builtin</code> step — the runtime short-circuits
              those in v1, so this one cannot run yet.
            </p>
          )}

          {preflight.missing_capabilities.length > 0 && (
            <p className="pane-note is-bad">
              Needs {preflight.missing_capabilities.join(", ")}
            </p>
          )}

          {/* Parameters come before the steps, because they are the last
              thing you touch before pressing run. Hidden once the run is
              under way — the numbers are the story then. */}
          {!run && preflight.manifest.parameters.length > 0 && (
            <div className="pane-params">
              <span className="label">Parameters</span>
              {preflight.manifest.parameters.map((p) => (
                <ParamField
                  key={p.name}
                  param={p}
                  value={params[p.name]}
                  onChange={(v) =>
                    setParams((prev) => ({ ...prev, [p.name]: v }))
                  }
                />
              ))}
            </div>
          )}

          <Steps preflight={preflight} run={run} />

          <RunSummary run={run} />
        </>
      )}
    </div>
  );
}

function PaneEmpty() {
  return (
    <div className="pane pane-empty">
      <p>Pick a workflow on the left.</p>
      <p className="pane-empty-hint">
        Enter opens it here. Enter again runs it.
      </p>
    </div>
  );
}

// ─── Header: name, clock, run ────────────────────────────────────────────

function PaneHeader({
  preflight,
  run,
  running,
  onRun,
}: {
  preflight: WorkflowPreflight;
  run: RunState | null;
  running: boolean;
  onRun: () => void;
}) {
  const elapsed = useElapsed(run);
  const blocked = !preflight.ready || preflight.has_builtin_step;
  return (
    <div className="pane-head">
      {/* Its own line when the pane is narrow — this name is the title of
          the window, and a truncated title is not one. */}
      <span className="pane-title">{preflight.manifest.name}</span>
      {elapsed != null && (
        <span className="pane-clock" title="elapsed">
          {(elapsed / 1000).toFixed(1)}s
        </span>
      )}
      <button
        type="button"
        className="btn primary pane-run"
        onClick={onRun}
        disabled={running || blocked}
        title={
          blocked
            ? "This workflow cannot run yet — see the note below"
            : "Run this workflow (Enter)"
        }
      >
        {running ? (
          "running…"
        ) : (
          <>
            <PlayIcon />
            run
          </>
        )}
      </button>
    </div>
  );
}

/**
 * Milliseconds since the run started, ticking while it runs and frozen on
 * the trace's own duration once it lands. Null before the first run, so
 * the pane shows no clock rather than a lying `0.0s`.
 */
function useElapsed(run: RunState | null): number | null {
  const [now, setNow] = useState(0);
  const startedAt = useRef(0);
  const active = run != null && !run.closed;

  useEffect(() => {
    if (!active) return;
    startedAt.current = performance.now();
    setNow(0);
    let raf = 0;
    const tick = () => {
      setNow(performance.now() - startedAt.current);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [active]);

  if (!run) return null;
  if (run.trace) return run.trace.duration_ms;
  if (run.closed) return now;
  return now;
}

// ─── Steps ───────────────────────────────────────────────────────────────

/**
 * Before a run, the compiled steps as the preflight reported them; during
 * and after one, the same rows carrying live status. Same list, same
 * order, same widths — so pressing run changes what the rows say, never
 * where they are.
 */
function Steps({
  preflight,
  run,
}: {
  preflight: WorkflowPreflight;
  run: RunState | null;
}) {
  if (preflight.steps.length === 0) return null;

  // The plan names the same activities the preflight did, so live state
  // joins onto the compiled list by activity_id rather than replacing it.
  const live = run?.steps ?? {};

  return (
    <ol className="timeline is-flat">
      {preflight.steps.map((s, i) => {
        const l = live[s.activity_id];
        return (
          <li key={s.activity_id} className="step">
            <div className="num">{String(i + 1).padStart(2, "0")}</div>
            <div className="step-body">
              <div className="name">{s.name}</div>
            </div>
            <div className={s.kind === "llm" ? "kind is-billed" : "kind"}>
              {s.kind}
            </div>
            <div className="right">
              <StepState step={l} />
            </div>
          </li>
        );
      })}
    </ol>
  );
}

function StepState({ step }: { step: LiveStep | undefined }) {
  if (!step || step.status === "queued") {
    return (
      <span className="step-pending" aria-label="not started">
        ·
      </span>
    );
  }
  if (step.status === "running") {
    return <span className="step-running" aria-label="running" />;
  }
  if (step.status === "failed") {
    return (
      <>
        <span className="step-mark is-bad">✗</span>
        <span>{step.duration_ms != null ? formatDuration(step.duration_ms) : ""}</span>
      </>
    );
  }
  return (
    <>
      <span className="step-mark is-ok">✓</span>
      <span>{step.duration_ms != null ? formatDuration(step.duration_ms) : ""}</span>
    </>
  );
}

// ─── Summary ─────────────────────────────────────────────────────────────

/**
 * The two lines the run adds up to: how it went, and what it cost. Held
 * in place rather than mounted, so a rerun never changes the height of
 * the pane under the pointer that pressed run.
 */
function RunSummary({ run }: { run: RunState | null }) {
  if (!run) return null;
  const trace = run.trace;
  const failed = run.error != null || trace?.status === "failed";
  const cost = trace?.cost?.total_eur;

  return (
    <div className="pane-summary">
      <div className={`pane-summary-line${run.closed ? "" : " is-pending"}`}>
        {failed ? (
          <>
            <span className="step-mark is-bad">✗</span>
            <span className="pane-summary-text">
              {run.error ?? trace?.error ?? "failed"}
            </span>
          </>
        ) : (
          <>
            <span className="step-mark is-ok">✓</span>
            <span className="pane-summary-text">
              {trace ? `${trace.activities.length} steps` : "done"}
            </span>
            {trace && (
              <span className="pane-summary-aside">
                <span className="sep">· </span>
                {formatDuration(trace.duration_ms)}
              </span>
            )}
          </>
        )}
      </div>

      {cost != null && cost > 0 && (
        <div className="pane-summary-line">
          <span className="pane-cost">{formatCost(cost)}</span>
          <span className="pane-summary-aside">
            billed · declared llm steps only
          </span>
        </div>
      )}

      {(run.error ?? trace?.error) && (
        <ConnectOffer error={run.error ?? trace?.error ?? ""} />
      )}
    </div>
  );
}

// ─── Parameters ──────────────────────────────────────────────────────────

function ParamField({
  param,
  value,
  onChange,
}: {
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  const id = `p-${param.name}`;
  return (
    <div className="param-row">
      <label htmlFor={id} className="param-name">
        {param.name}
        {param.required && <span className="param-required"> *</span>}
        <span className="param-type">{param.type}</span>
      </label>
      {param.description && (
        <div className="param-desc">{param.description}</div>
      )}
      <ParamInput id={id} param={param} value={value} onChange={onChange} />
    </div>
  );
}

function ParamInput({
  id,
  param,
  value,
  onChange,
}: {
  id: string;
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  if (param.type === "boolean") {
    return (
      <input
        id={id}
        type="checkbox"
        checked={value === true}
        onChange={(e) => onChange(e.target.checked)}
      />
    );
  }

  if (param.type === "enum" && Array.isArray(param.values)) {
    return (
      <select
        id={id}
        className="param-input"
        value={value == null ? "" : String(value)}
        onChange={(e) => onChange(e.target.value)}
      >
        <option value="">— select —</option>
        {param.values.map((v, i) => (
          <option key={i} value={String(v)}>
            {String(v)}
          </option>
        ))}
      </select>
    );
  }

  if (param.type === "number") {
    return (
      <input
        id={id}
        className="param-input"
        type="number"
        value={value == null ? "" : String(value)}
        min={param.min ?? undefined}
        max={param.max ?? undefined}
        onChange={(e) =>
          onChange(e.target.value === "" ? null : Number(e.target.value))
        }
      />
    );
  }

  return (
    <input
      id={id}
      className="param-input"
      type="text"
      value={value == null ? "" : String(value)}
      onChange={(e) => onChange(e.target.value)}
      placeholder={param.type === "path" ? "/abs/path or ./relative" : ""}
    />
  );
}

// ─── Consent ─────────────────────────────────────────────────────────────

function ConsentModal({
  consent,
  onTrust,
  onCancel,
  submitting,
}: {
  consent: ConsentRequired;
  onTrust: () => void;
  onCancel: () => void;
  submitting: boolean;
}) {
  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h2>Trust this remote workflow?</h2>
        <dl className="kv" style={{ margin: "16px 0" }}>
          <dt>Host</dt>
          <dd>{consent.host}</dd>
          <dt>Repo</dt>
          <dd>{consent.repo}</dd>
          {consent.subpath && (
            <>
              <dt>Subpath</dt>
              <dd>{consent.subpath}</dd>
            </>
          )}
          {consent.ref_str && (
            <>
              <dt>Ref</dt>
              <dd>{consent.ref_str}</dd>
            </>
          )}
          <dt>SHA</dt>
          <dd>{consent.sha.slice(0, 12)}</dd>
        </dl>
        <p className="hint">
          Trusting records consent for (
          <code>
            {consent.host}/{consent.repo}
          </code>
          , <code>{consent.sha.slice(0, 12)}</code>) in{" "}
          <code>~/.cori/cache/remote/trust.json</code>.
        </p>
        <div className="modal-actions">
          <button className="btn" onClick={onCancel} disabled={submitting}>
            Cancel
          </button>
          <button className="btn primary" onClick={onTrust} disabled={submitting}>
            {submitting ? "Recording…" : "Trust & continue"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── Reducer + helpers ───────────────────────────────────────────────────

function reduceRun(state: RunState, ev: RunEvent): RunState {
  switch (ev.type) {
    case "plan": {
      const steps: Record<string, LiveStep> = {};
      for (const p of ev.assignments as PlanStep[]) {
        steps[p.activity_id] = { step_name: p.step_name, status: "queued" };
      }
      return {
        ...state,
        order: ev.assignments.map((p) => p.activity_id),
        steps,
      };
    }
    case "step_start":
      return {
        ...state,
        steps: {
          ...state.steps,
          [ev.activity_id]: {
            ...state.steps[ev.activity_id],
            step_name: ev.step_name,
            kind: ev.kind,
            status: "running",
          },
        },
      };
    case "step_finish":
      return {
        ...state,
        steps: {
          ...state.steps,
          [ev.activity_id]: {
            ...state.steps[ev.activity_id],
            step_name: ev.step_name,
            status: (ev.status as LiveStep["status"]) ?? "succeeded",
            duration_ms: ev.duration_ms,
            error: ev.error,
          },
        },
      };
    case "completed":
      return { ...state, trace: ev.trace, closed: true };
    case "failed":
      return { ...state, error: ev.error, closed: true };
  }
}

function formatErr(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Solid, unlike the outlined icon set — it is a button, not a glyph. */
function PlayIcon() {
  return (
    <svg viewBox="0 0 16 16" width="9" height="9" aria-hidden fill="currentColor">
      <path d="M5.5 3.6a.7.7 0 011.06-.6l6 4.4a.7.7 0 010 1.2l-6 4.4a.7.7 0 01-1.06-.6z" />
    </svg>
  );
}

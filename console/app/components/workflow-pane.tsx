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
  listLlmProviders,
  listRuns,
  recordTrust,
  resolveWorkflow,
  startRun,
  type ConsentRequired,
  type LlmProviderInfo,
  type ParameterDef,
  type PlanStep,
  type RunEvent,
  type RunListEntry,
  type RunTrace,
  type WorkflowPreflight,
} from "../lib/api";
import { ProviderKeyForm } from "./provider-key-form";
import {
  formatAbsolute,
  formatCost,
  formatDuration,
  formatRelative,
} from "../lib/format";
import { openRun } from "../lib/windows";
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

type ResolveFailure = {
  kind: "missing" | "invalid" | "generic";
  message: string;
};

export function WorkflowPane({
  source,
  handleRef,
  onLocateMissing,
}: {
  /** `cori run`-compatible source string, or null when nothing is picked. */
  source: string | null;
  handleRef?: Ref<WorkflowPaneHandle>;
  /** Open the launcher's local browser near a workflow's old path. */
  onLocateMissing?: (source: string) => Promise<void>;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [phaseSource, setPhaseSource] = useState<string | null>(null);
  const [loaderVisible, setLoaderVisible] = useState(false);
  const [preflight, setPreflight] = useState<WorkflowPreflight | null>(null);
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [failure, setFailure] = useState<ResolveFailure | null>(null);
  const [consent, setConsent] = useState<ConsentRequired | null>(null);
  const [trusting, setTrusting] = useState(false);
  const [run, setRun] = useState<RunState | null>(null);
  const [history, setHistory] = useState<RunListEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [llmProviders, setLlmProviders] = useState<LlmProviderInfo[]>([]);

  // Only the newest resolve is allowed to land: arrowing down a long list
  // and pressing Enter twice must not let a slow first request overwrite
  // the pane with a workflow the user has already moved off.
  const requestId = useRef(0);

  const resolve = useCallback(async (src: string, update = false) => {
    const id = ++requestId.current;
    setPhaseSource(src);
    setPhase("loading");
    setFailure(null);
    setConsent(null);
    setPreflight(null);
    setRun(null);
    try {
      const pf = await resolveWorkflow({ source: src, update });
      if (id !== requestId.current) return;
      if (pf.required_llm_providers.length > 0) {
        listLlmProviders().then(setLlmProviders).catch(() => {});
      }
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
        setFailure(classifyResolveFailure(e));
        setPhase("error");
      }
    }
  }, []);

  useEffect(() => {
    if (!source) {
      requestId.current += 1; // drop anything in flight
      setPhase("idle");
      setPhaseSource(null);
      setPreflight(null);
      setRun(null);
      setFailure(null);
      setConsent(null);
      return;
    }
    void resolve(source);
  }, [source, resolve]);

  // A quick cache hit should feel instant, not flash a one-frame status.
  // Slower resolves earn a quiet loader after a short threshold; keeping the
  // node mounted lets it fade away while the result fades in underneath.
  const resolving = phase === "loading" || phaseSource !== source;
  useEffect(() => {
    if (!resolving) {
      setLoaderVisible(false);
      return;
    }
    const timer = window.setTimeout(() => setLoaderVisible(true), 160);
    return () => window.clearTimeout(timer);
  }, [resolving, source]);

  const running = run != null && !run.closed;
  const completedRunId = run?.closed
    ? (run.trace?.run_id ?? run.runId ?? "closed")
    : null;

  // Run history belongs to this exact source, not merely to a manifest id
  // that another folder could share. `history_key` is computed by the Rust
  // loader with the same path/ref identity used when traces are persisted.
  useEffect(() => {
    const historyKey = preflight?.history_key;
    if (!historyKey) {
      setHistory([]);
      setHistoryLoading(false);
      setHistoryError(null);
      return;
    }

    let cancelled = false;
    setHistoryLoading(true);
    setHistoryError(null);
    listRuns({ history_key: historyKey, limit: 12 })
      .then((runs) => {
        if (!cancelled) setHistory(runs);
      })
      .catch((e: unknown) => {
        if (!cancelled) setHistoryError(formatErr(e));
      })
      .finally(() => {
        if (!cancelled) setHistoryLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [preflight?.history_key, completedRunId]);

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
      setFailure(classifyResolveFailure(e));
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

      <div
        className={`pane-loader${loaderVisible ? " is-visible" : ""}`}
        role="status"
        aria-live="polite"
        aria-hidden={!loaderVisible}
        title={source}
      >
        <span>Resolving workflow</span>
        <span className="pane-loader-dots" aria-hidden>
          <i />
          <i />
          <i />
        </span>
      </div>

      {!resolving && phase === "error" && !consent && (
        <div className="pane-enter" key={`error:${source}`}>
          <WorkflowFailureView
            failure={failure}
            source={source}
            onLocateMissing={onLocateMissing}
          />
        </div>
      )}

      {!resolving && preflight && (
        <div className="pane-enter" key={`workflow:${source}`}>
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

          {!run && (
            <MissingLlmProviders
              preflight={preflight}
              providers={llmProviders}
              onSaved={(updated) => {
                setLlmProviders((ps) =>
                  ps.map((p) => (p.id === updated.id ? updated : p)),
                );
                // Re-resolve so missing capabilities and readiness pick
                // up the newly stored key.
                if (source) void resolve(source);
              }}
            />
          )}

          {nonLlmMissing(preflight, llmProviders).length > 0 && (
            <p className="pane-note is-bad">
              Needs {nonLlmMissing(preflight, llmProviders).join(", ")}
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

          <WorkflowHistory
            runs={history}
            loading={historyLoading}
            error={historyError}
          />
        </div>
      )}
    </div>
  );
}

/** Required LLM providers with no stored key and no env override. */
function missingLlmIds(
  preflight: WorkflowPreflight,
  providers: LlmProviderInfo[],
): string[] {
  if (providers.length === 0) return []; // list not loaded — raw note covers it
  return preflight.required_llm_providers.filter((id) => {
    const p = providers.find((x) => x.id === id);
    return p ? !p.configured && !p.env_override : false;
  });
}

/** Missing-capability lines minus the LLM ones we render inline forms
 *  for (format from cori-broker: "missing LLM provider: `id` — hint"). */
function nonLlmMissing(
  preflight: WorkflowPreflight,
  providers: LlmProviderInfo[],
): string[] {
  if (missingLlmIds(preflight, providers).length === 0) {
    return preflight.missing_capabilities;
  }
  return preflight.missing_capabilities.filter(
    (m) => !m.startsWith("missing LLM provider:"),
  );
}

function MissingLlmProviders({
  preflight,
  providers,
  onSaved,
}: {
  preflight: WorkflowPreflight;
  providers: LlmProviderInfo[];
  onSaved: (updated: LlmProviderInfo) => void;
}) {
  const missing = missingLlmIds(preflight, providers);
  if (missing.length === 0) return null;
  return (
    <div className="card">
      <p className="pane-note is-warn" style={{ margin: "0 0 8px" }}>
        This workflow has LLM steps — paste an API key to continue. It's
        verified, stored once, and reused by every future run.
      </p>
      {missing.map((id) => {
        const p = providers.find((x) => x.id === id);
        if (!p) return null;
        return (
          <div key={id} style={{ marginBottom: 10 }}>
            <span className="label">{p.display_name}</span>
            <ProviderKeyForm provider={p} onChanged={onSaved} />
          </div>
        );
      })}
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

function WorkflowFailureView({
  failure,
  source,
  onLocateMissing,
}: {
  failure: ResolveFailure | null;
  source: string;
  onLocateMissing?: (source: string) => Promise<void>;
}) {
  if (failure?.kind === "missing") {
    return (
      <MissingWorkflow
        source={source}
        onLocate={onLocateMissing}
      />
    );
  }
  if (failure?.kind === "invalid") {
    return <InvalidWorkflow />;
  }
  return (
    <div className="pane-status is-error">
      {failure?.message ?? "Could not resolve."}
    </div>
  );
}

function MissingWorkflow({
  source,
  onLocate,
}: {
  source: string;
  onLocate?: (source: string) => Promise<void>;
}) {
  const [locating, setLocating] = useState(false);
  const [locateError, setLocateError] = useState<string | null>(null);
  const folderName = workflowFolderName(source);

  async function locate() {
    if (!onLocate || locating) return;
    setLocating(true);
    setLocateError(null);
    try {
      await onLocate(source);
    } catch (e: unknown) {
      setLocateError(formatErr(e));
    } finally {
      setLocating(false);
    }
  }

  return (
    <div className="workflow-issue" role="status">
      <div className="workflow-issue-icon is-missing" aria-hidden>
        <FolderSearchIcon />
      </div>
      <div className="workflow-issue-kicker">Workflow moved</div>
      <h2>Let’s find it again.</h2>
      <p>
        Cori can’t find <strong>{folderName}</strong> at its last saved
        location. It may have been moved, renamed, or deleted.
      </p>
      <div className="workflow-issue-path" title={source}>
        {source}
      </div>
      <div className="workflow-issue-helper">
        {onLocate && (
          <button
            type="button"
            className="btn primary"
            onClick={() => void locate()}
            disabled={locating}
          >
            <FolderIcon />
            {locating ? "opening…" : "browse nearby"}
          </button>
        )}
        <span>
          Pick the workflow folder on the left, or drop it anywhere in this
          window.
        </span>
      </div>
      {locateError && (
        <div className="workflow-issue-error">{locateError}</div>
      )}
    </div>
  );
}

function InvalidWorkflow() {
  return (
    <div className="workflow-issue is-invalid" role="status">
      <div className="workflow-issue-icon is-invalid" aria-hidden>
        <RemakeIcon />
      </div>
      <div className="workflow-issue-kicker">Workflow out of date</div>
      <h2>Please remake this workflow.</h2>
      <p>
        This workflow is no longer valid in Cori. Remake it from the original
        task, then select the new workflow to continue.
      </p>
    </div>
  );
}

function workflowFolderName(source: string): string {
  const clean = source.replace(/[\\/]+$/, "");
  const leaf = clean.split(/[\\/]/).pop();
  return leaf || "this workflow";
}

function classifyResolveFailure(e: unknown): ResolveFailure {
  const message = formatErr(e);
  if (isIpcError(e)) {
    if (e.code === "workflow_missing") return { kind: "missing", message };
    if (e.code === "workflow_invalid") return { kind: "invalid", message };
  }

  // Backward-compatible with an older Console backend during hot reload.
  if (
    message.includes("resolving workflow path") &&
    (message.includes("No such file or directory") ||
      message.includes("os error 2"))
  ) {
    return { kind: "missing", message };
  }
  if (
    message.includes("validating workflow TypeScript modules") ||
    message.includes("workflow TypeScript module graph validation failed")
  ) {
    return { kind: "invalid", message };
  }
  return { kind: "generic", message };
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
        className="btn pane-run"
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

// ─── Workflow run history ───────────────────────────────────────────────

function WorkflowHistory({
  runs,
  loading,
  error,
}: {
  runs: RunListEntry[];
  loading: boolean;
  error: string | null;
}) {
  return (
    <section className="pane-history" aria-labelledby="workflow-history-title">
      <div className="pane-history-head">
        <span id="workflow-history-title" className="label">
          Run history
        </span>
        {runs.length > 0 && (
          <span className="pane-history-count">
            {runs.length} recent
          </span>
        )}
      </div>

      {loading && runs.length === 0 && (
        <div className="pane-history-empty">Loading runs…</div>
      )}
      {error && <div className="pane-history-empty is-error">{error}</div>}
      {!loading && !error && runs.length === 0 && (
        <div className="pane-history-empty">
          No launches recorded for this workflow yet.
        </div>
      )}

      {runs.length > 0 && (
        <div className="pane-history-list">
          {runs.map((entry) => (
            <HistoryRow key={`${entry.key}:${entry.utc}`} entry={entry} />
          ))}
        </div>
      )}
    </section>
  );
}

function HistoryRow({ entry }: { entry: RunListEntry }) {
  const statusClass = historyStatusClass(entry.status);
  const status = entry.status.replaceAll("_", " ");
  return (
    <button
      type="button"
      className="pane-history-row"
      onClick={() =>
        void openRun(entry.run_id, { key: entry.key, utc: entry.utc })
      }
      title={`Open run ${entry.run_id}\n${formatAbsolute(entry.started_at)}`}
    >
      <span className={`pane-history-mark ${statusClass}`} aria-hidden>
        {historyStatusMark(entry.status)}
      </span>
      <span className="pane-history-main">
        <span className={`pane-history-status ${statusClass}`}>{status}</span>
        <span className="pane-history-meta">
          <span title={formatAbsolute(entry.started_at)}>
            {formatRelative(entry.started_at)}
          </span>
          <span aria-hidden>·</span>
          <span>{historyTriggerLabel(entry.trigger)}</span>
        </span>
      </span>
      <span className="pane-history-duration">
        {formatDuration(entry.duration_ms)}
      </span>
      <span className="pane-history-open" aria-hidden>
        →
      </span>
    </button>
  );
}

function historyStatusClass(status: string): string {
  if (status === "succeeded") return "is-ok";
  if (status === "failed") return "is-bad";
  if (status === "running") return "is-live";
  return "is-muted";
}

function historyStatusMark(status: string): string {
  if (status === "succeeded") return "✓";
  if (status === "failed") return "×";
  if (status === "running") return "•";
  return "–";
}

function historyTriggerLabel(trigger: string): string {
  if (trigger === "mcp") return "agent";
  return trigger.replaceAll("_", " ");
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

function FolderIcon() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden fill="none">
      <path
        d="M1.75 4.25h4l1.3 1.5h7.2v6.5a1 1 0 01-1 1H2.75a1 1 0 01-1-1v-8z"
        stroke="currentColor"
        strokeWidth="1.25"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function FolderSearchIcon() {
  return (
    <svg viewBox="0 0 24 24" width="25" height="25" aria-hidden fill="none">
      <path
        d="M3 6.5h6l2 2h10v8.25A2.25 2.25 0 0118.75 19H5.25A2.25 2.25 0 013 16.75V6.5z"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <circle cx="16.25" cy="14.25" r="2.25" stroke="currentColor" strokeWidth="1.5" />
      <path d="M17.9 15.9l2.1 2.1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

function RemakeIcon() {
  return (
    <svg viewBox="0 0 24 24" width="25" height="25" aria-hidden fill="none">
      <path
        d="M6 3.5h8l4 4v13H6v-17z"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <path d="M14 3.5v4h4M9.5 12.25l5 5m0-5l-5 5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

// The launcher's right pane: the workflow you picked, and running it.
//
// One window is the whole product — picking a workflow on the left does
// not open a second window, it fills this pane. The pane resolves the
// source and hands the compiled plan to the graph canvas, which carries
// every phase of the workflow's life: an agent writing it, the frozen
// proposal, the compiled plan, the live run, the settled trace.
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
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  isIpcError,
  listRuns,
  recordTrust,
  resolveWorkflow,
  startRun,
  type AuthoringSession,
  type ConsentRequired,
  type ParameterDef,
  type PlanStep,
  type RunEvent,
  type RunListEntry,
  type StepSummary,
  type WorkflowPreflight,
} from "../lib/api";
import {
  ParamField,
  isBlank,
  missingRequired,
  paramDefaults,
} from "./param-fields";
import {
  formatAbsolute,
  formatCost,
  formatDuration,
  formatRelative,
} from "../lib/format";
import { openRun } from "../lib/windows";
import { ConnectOffer } from "./run-view";
import { ResultCard } from "./result-card";
import { GraphCanvas, type LiveStep, type RunState } from "./graph-canvas";

/** What the launcher can ask of the pane from its own key handling. */
export interface WorkflowPaneHandle {
  /** True when a second Enter should start the run rather than re-resolve. */
  canRun(): boolean;
  /** Enter is a dry run; only an explicit ⌘⏎ passes `false`. */
  run(dryRun: boolean): void;
  /** Copy the printed `cori run` command; false when nothing to copy. */
  yank(): boolean;
  /** Reveal the resolved workflow folder in the OS file manager. */
  openFolder(): void;
}

type Phase = "idle" | "loading" | "error" | "ready";

type ResolveFailure = {
  kind: "missing" | "invalid" | "generic";
  message: string;
};

export function WorkflowPane({
  source,
  session = null,
  handleRef,
  onLocateMissing,
}: {
  /** `cori run`-compatible source string, or null when nothing is picked. */
  source: string | null;
  /** Live authoring session journalling into this workflow's folder. */
  session?: AuthoringSession | null;
  handleRef?: Ref<WorkflowPaneHandle>;
  /** Open the launcher's local browser near a workflow's old path. */
  onLocateMissing?: (source: string) => Promise<void>;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [phaseSource, setPhaseSource] = useState<string | null>(null);
  const [loaderVisible, setLoaderVisible] = useState(false);
  const [preflight, setPreflight] = useState<WorkflowPreflight | null>(null);
  const [params, setParams] = useState<Record<string, unknown>>({});
  // A re-runner rarely edits parameters: collapsed to a defaults summary
  // unless a required parameter has no value yet.
  const [paramsOpen, setParamsOpen] = useState(false);
  const [failure, setFailure] = useState<ResolveFailure | null>(null);
  const [consent, setConsent] = useState<ConsentRequired | null>(null);
  const [trusting, setTrusting] = useState(false);
  const [run, setRun] = useState<RunState | null>(null);
  const [history, setHistory] = useState<RunListEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);

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
      const defaults = paramDefaults(pf.manifest.parameters);
      setPreflight(pf);
      setParams(defaults);
      setParamsOpen(missingRequired(pf.manifest.parameters, defaults).length > 0);
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

  // While an agent session journals into this folder, re-resolve quietly
  // on every journal bump so nodes land on the canvas as they are
  // written (the backend serves a draft parse while the folder cannot
  // fully compile). Quiet: the current preflight stays on screen; a
  // resolve that fails outright keeps the last good plan (or the
  // canvas's "thinking" state when there was none). One more refresh
  // fires when the session ends — a publish or accept just changed the
  // folder, and the draft view must give way to the compiled one.
  const sessionSeq = session?.current_seq;
  const sessionState = session?.state ?? null;
  const hadSessionRef = useRef(false);
  useEffect(() => {
    const hadSession = hadSessionRef.current;
    hadSessionRef.current = sessionSeq != null;
    if (!source) return;
    if (sessionSeq == null && !hadSession) return;
    // Observe (never bump) the request counter: a quiet refresh must not
    // cancel a real resolve, and must discard itself if one starts.
    const id = requestId.current;
    const timer = window.setTimeout(() => {
      resolveWorkflow({ source })
        .then((pf) => {
          if (id !== requestId.current) return;
          setPreflight(pf);
          setFailure(null);
          setPhase("ready");
          // New parameters get their defaults; anything the human already
          // set survives the refresh.
          setParams((prev) => ({
            ...paramDefaults(pf.manifest.parameters),
            ...prev,
          }));
        })
        .catch(() => {
          // Mid-edit folders legitimately fail to compile — keep showing
          // what we had.
        });
    }, 250);
    return () => window.clearTimeout(timer);
  }, [source, sessionSeq, sessionState]);

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

  // §0 Q4 — judged against its own history: medians over real (non-dry)
  // succeeded runs, excluding the run currently on screen.
  const baseline = medianBaseline(history, run?.trace?.run_id ?? null);

  const startWorkflow = useCallback(
    (dryRun: boolean) => {
      if (!source || !preflight || running) return;
      if (missingRequired(preflight.manifest.parameters, params).length > 0) {
        setParamsOpen(true);
        return;
      }
      setRun({
        runId: null,
        order: [],
        steps: {},
        trace: null,
        error: null,
        closed: false,
        dry: dryRun,
      });

      const channel = new Channel<RunEvent>();
      channel.onmessage = (ev) => {
        setRun((prev) => (prev ? reduceRun(prev, ev) : prev));
      };

      startRun({ source, params, dry_run: dryRun, on_event: channel })
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
    },
    [source, preflight, params, running],
  );

  const runCommand =
    source && preflight
      ? buildRunCommand(source, preflight.manifest.parameters, params)
      : null;

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
      yank: () => {
        if (!runCommand) return false;
        void navigator.clipboard?.writeText(runCommand).catch(() => {});
        return true;
      },
      openFolder: () => {
        if (preflight?.absolute_path) {
          void revealItemInDir(preflight.absolute_path).catch(() => {});
        }
      },
    }),
    [phase, preflight, running, startWorkflow, runCommand],
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

  const elapsed = useElapsed(run);

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
          {session != null ? (
            // The folder does not compile because an agent is mid-edit:
            // that is the canvas's "thinking" state, not a failure.
            <GraphCanvas
              preflight={null}
              run={null}
              session={session}
              elapsedMs={null}
              historyKey={null}
              onRun={() => {}}
              onSessionChanged={() => void resolve(source)}
            />
          ) : (
            <WorkflowFailureView
              failure={failure}
              source={source}
              onLocateMissing={onLocateMissing}
            />
          )}
        </div>
      )}

      {!resolving && preflight && (
        <div className="pane-enter" key={`workflow:${source}`}>
          <PaneHeader
            preflight={preflight}
            elapsed={elapsed}
            onRun={startWorkflow}
            running={running}
          />

          <IdentityStrip
            source={source}
            preflight={preflight}
            history={history}
          />

          {preflight.manifest.description && (
            <p className="pane-desc">{preflight.manifest.description}</p>
          )}

          <ManifestBrief preflight={preflight} />

          {preflight.has_builtin_step && (
            <p className="pane-note is-warn">
              Uses a <code>map</code> or <code>parallel</code> step — the
              runtime still defers those, so this one cannot run yet. Control
              flow (<code>branch</code>, <code>switch</code>,{" "}
              <code>for_each</code>, <code>loop</code>, <code>wait</code>)
              runs.
            </p>
          )}

          {preflight.missing_capabilities.length > 0 && (
            <p className="pane-note is-bad">
              Needs {preflight.missing_capabilities.join(", ")}.
              {preflight.missing_capabilities.some((item) =>
                item.startsWith("missing AI provider:"),
              )
                ? " Open Settings → AI Providers to select or repair the active provider."
                : null}
            </p>
          )}

          {/* Parameters come before the steps, because they are the last
              thing you touch before pressing run. Hidden only while a run
              is under way — the numbers are the story then — and back once
              it settles, so the next run can be launched with new values. */}
          {(!run || run.closed) && preflight.manifest.parameters.length > 0 && (
            <div className="pane-params">
              <div className="pane-params-head">
                <span className="label">Parameters</span>
                <button
                  type="button"
                  className="pane-params-toggle"
                  onClick={() => setParamsOpen((o) => !o)}
                >
                  {paramsOpen ? "done" : "edit"}
                </button>
              </div>
              {paramsOpen ? (
                preflight.manifest.parameters.map((p) => (
                  <ParamField
                    key={p.name}
                    param={p}
                    value={params[p.name]}
                    onChange={(v) =>
                      setParams((prev) => ({ ...prev, [p.name]: v }))
                    }
                  />
                ))
              ) : (
                <ParamsSummary
                  parameters={preflight.manifest.parameters}
                  values={params}
                  onEdit={() => setParamsOpen(true)}
                />
              )}
            </div>
          )}

          {(!run || run.closed) && runCommand && (
            <CommandLine command={runCommand} />
          )}

          <GraphCanvas
            preflight={preflight}
            run={run}
            session={session}
            elapsedMs={elapsed}
            historyKey={preflight.history_key}
            onRun={startWorkflow}
            onSessionChanged={() => void resolve(source)}
          />

          <RunSummary run={run} baseline={baseline} />

          <WorkflowHistory
            runs={history}
            loading={historyLoading}
            error={historyError}
            baseline={baseline}
          />
        </div>
      )}
    </div>
  );
}

function PaneEmpty() {
  return (
    <div className="pane pane-empty">
      <p>Pick a workflow on the left.</p>
      <p className="pane-empty-hint">
        Enter opens it here, then dry-runs it. ⌘Enter runs it for real.
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

// ─── Manifest brief: the pane is the manifest ────────────────────────────

/** Extract one `## Heading` section from the manifest body, verbatim. */
function manifestSection(body: string, heading: string): string | null {
  const lines = body.split("\n");
  const start = lines.findIndex(
    (l) => l.trim().toLowerCase() === `## ${heading}`.toLowerCase(),
  );
  if (start === -1) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((l) => /^##\s/.test(l.trim()));
  const text = (end === -1 ? rest : rest.slice(0, end)).join("\n").trim();
  return text.length > 0 ? text : null;
}

/**
 * One mono line answering "which workflow, from where, still the same
 * code?" — source kind, the source itself, the content hash, and
 * whether the folder changed since the last recorded run.
 */
function IdentityStrip({
  source,
  preflight,
  history,
}: {
  source: string;
  preflight: WorkflowPreflight;
  history: RunListEntry[];
}) {
  const local =
    source.startsWith("/") || source.startsWith("~") || source.startsWith(".");
  const lastHash = history.find(
    (h) => h.workflow_content_hash,
  )?.workflow_content_hash;
  const changed = lastHash != null && lastHash !== preflight.content_hash;
  return (
    <div className="pane-identity">
      <span className={`pane-identity-kind${local ? "" : " is-remote"}`}>
        {local ? "local" : "remote"}
      </span>
      <span className="sep" aria-hidden>
        ·
      </span>
      <span className="pane-identity-source" title={source}>
        {source}
      </span>
      <span className="sep" aria-hidden>
        ·
      </span>
      <span title={`Workflow content hash ${preflight.content_hash}`}>
        content {preflight.content_hash.slice(0, 8)}
      </span>
      {lastHash != null && (
        <>
          <span className="sep" aria-hidden>
            ·
          </span>
          <span className={changed ? "pane-identity-changed" : "pane-identity-same"}>
            {changed ? "changed since last run" : "unchanged since last run"}
          </span>
        </>
      )}
    </div>
  );
}

/**
 * The manifest's own Goal, verbatim, plus what the run executes with:
 * the CLI binaries steps shell out to, and the model each `llm` step
 * resolves to under current settings.
 */
function ManifestBrief({ preflight }: { preflight: WorkflowPreflight }) {
  const body = preflight.manifest.body ?? "";
  const goal = manifestSection(body, "Goal");
  const binaries = preflight.required_cli_binaries;
  const models = llmUsage(preflight.steps);
  if (!goal && binaries.length === 0 && models.length === 0) return null;
  return (
    <div className="pane-brief">
      {goal && (
        <section className="pane-section">
          <span className="label">Goal</span>
          <div className="pane-section-text">{goal}</div>
        </section>
      )}
      {(binaries.length > 0 || models.length > 0) && (
        <div className="pane-runtime">
          {binaries.map((b) => (
            <span
              key={`bin:${b}`}
              className="pane-runtime-item"
              title={`A cli step shells out to ${b}`}
            >
              <span className="pane-runtime-kind">binary</span>
              <code>{b}</code>
            </span>
          ))}
          {models.map((m) => (
            <span key={`llm:${m.key}`} className="pane-runtime-item" title={m.title}>
              <span className="pane-runtime-kind">model</span>
              <code>{m.label}</code>
              {m.count > 1 && (
                <span className="pane-runtime-count">×{m.count}</span>
              )}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The workflow's `llm` steps folded into "model × step count" — the
 * resolution comes from the preflight, the same source Settings shows,
 * so the chip and Settings can never disagree.
 */
function llmUsage(
  steps: StepSummary[],
): { key: string; label: string; title: string; count: number }[] {
  const rows = new Map<
    string,
    { key: string; label: string; title: string; count: number }
  >();
  for (const s of steps) {
    if (s.kind !== "llm") continue;
    const r = s.llm_resolution;
    const key = r ? `${r.backend_id}:${r.model}` : `level:${s.level ?? "?"}`;
    const row = rows.get(key);
    if (row) {
      row.count += 1;
      continue;
    }
    rows.set(key, {
      key,
      label: r ? r.model : `${s.level ?? "llm"} (unresolved)`,
      title: r
        ? `${r.display_name} serves ${r.level} steps with ${r.model}`
        : "No AI provider resolves this step yet — see Settings → AI Providers",
      count: 1,
    });
  }
  return [...rows.values()];
}

// ─── Run command ─────────────────────────────────────────────────────────

/** Quote a shell argument only when it needs it. */
function shellQuote(s: string): string {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s)
    ? s
    : `'${s.replaceAll("'", `'\\''`)}'`;
}

/**
 * The exact command the run button is about to execute — identical to
 * what you'd type in a terminal: `cori run <source> key=value …`.
 */
function buildRunCommand(
  source: string,
  parameters: ParameterDef[],
  values: Record<string, unknown>,
): string {
  const parts = ["cori", "run", shellQuote(source)];
  for (const p of parameters) {
    const v = values[p.name];
    if (isBlank(v)) continue;
    parts.push(shellQuote(`${p.name}=${String(v)}`));
  }
  return parts.join(" ");
}

function CommandLine({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(command).catch(() => {});
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };
  return (
    <div className="pane-cmd" title={command}>
      <code className="pane-cmd-text">{command}</code>
      <button
        type="button"
        className="pane-cmd-copy"
        onClick={copy}
        title="Copy the command (y)"
      >
        {copied ? "copied" : "copy"}
      </button>
    </div>
  );
}

/** A parameter's current value, phrased for a human, not a shell. */
function paramValueLabel(value: unknown, type: string): string {
  if (type === "boolean") return value === true ? "yes" : "no";
  return String(value);
}

/**
 * What the collapsed parameter block reads as: each parameter on its own
 * row — name, current value, and whether that value is still the
 * manifest's default. Any row opens the editor, so the next run can be
 * launched with different values without hunting for a mode.
 */
function ParamsSummary({
  parameters,
  values,
  onEdit,
}: {
  parameters: ParameterDef[];
  values: Record<string, unknown>;
  onEdit: () => void;
}) {
  return (
    <div className="pane-params-grid" role="list">
      {parameters.map((p) => {
        const value = values[p.name];
        const blank = isBlank(value);
        const isDefault =
          !blank && p.default != null && String(value) === String(p.default);
        return (
          <button
            key={p.name}
            type="button"
            role="listitem"
            className="pane-params-item"
            onClick={onEdit}
            title={
              p.description
                ? `${p.description}\n\nClick to edit`
                : "Click to edit"
            }
          >
            <span className="pane-params-item-name">
              {p.name}
              {p.required && (
                <span className="param-required" aria-hidden>
                  {" "}
                  *
                </span>
              )}
            </span>
            {blank ? (
              <span
                className={`pane-params-item-value ${p.required ? "is-missing" : "is-unset"}`}
              >
                {p.required ? "required — set a value" : "not set"}
              </span>
            ) : (
              <span className="pane-params-item-value">
                {paramValueLabel(value, p.type)}
              </span>
            )}
            {isDefault && (
              <span className="pane-params-item-default">default</span>
            )}
          </button>
        );
      })}
    </div>
  );
}

// ─── History baseline (medians) ──────────────────────────────────────────

interface HistoryBaseline {
  /** Median over real (non-dry) succeeded runs; null below 3 samples. */
  durationMs: number | null;
  costEur: number | null;
  samples: number;
}

const BASELINE_MIN_SAMPLES = 3;

function medianOf(ns: number[]): number | null {
  if (ns.length === 0) return null;
  const sorted = [...ns].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[mid]
    : (sorted[mid - 1] + sorted[mid]) / 2;
}

function medianBaseline(
  history: RunListEntry[],
  excludeRunId: string | null,
): HistoryBaseline {
  const real = history.filter(
    (h) =>
      !h.dry_run && h.status === "succeeded" && h.run_id !== excludeRunId,
  );
  if (real.length < BASELINE_MIN_SAMPLES) {
    return { durationMs: null, costEur: null, samples: real.length };
  }
  return {
    durationMs: medianOf(real.map((h) => h.duration_ms)),
    costEur: medianOf(real.map((h) => h.cost.total_eur)),
    samples: real.length,
  };
}

/** Signed delta vs the median, or null when it's inside the noise band. */
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
  const slower = delta > 0;
  return (
    <span
      className={`pane-delta ${slower ? "is-over" : "is-under"}`}
      title="vs this workflow's median (real runs)"
    >
      {slower ? "+" : "−"}
      {format(Math.abs(delta))} vs median
    </span>
  );
}

// ─── Header: name, clock, run ────────────────────────────────────────────

function PaneHeader({
  preflight,
  elapsed,
  running,
  onRun,
}: {
  preflight: WorkflowPreflight;
  elapsed: number | null;
  running: boolean;
  onRun: (dryRun: boolean) => void;
}) {
  const blocked = !preflight.ready || preflight.has_builtin_step;
  const blockedTitle = "This workflow cannot run yet — see the note below";
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
      {/* ⏎ is the safe verb. Only the explicit ⌘⏎ (or the violet button)
          touches the world — the accent is reserved for it. */}
      <div className="pane-run-actions">
        <button
          type="button"
          className="btn pane-run"
          onClick={() => onRun(true)}
          disabled={running || blocked}
          title={
            blocked
              ? blockedTitle
              : "Dry run — validates the whole plan, external steps are stubbed (⏎)"
          }
        >
          dry run
        </button>
        <button
          type="button"
          className="btn primary pane-run"
          onClick={() => onRun(false)}
          disabled={running || blocked}
          title={blocked ? blockedTitle : "Run this workflow for real (⌘⏎)"}
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

function runOutcome(run: RunState): "running" | "succeeded" | "failed" {
  if (!run.closed) return "running";
  return run.error != null || run.trace?.status === "failed" ? "failed" : "succeeded";
}

// ─── Summary ─────────────────────────────────────────────────────────────

/**
 * The two lines the run adds up to: how it went, and what it cost — each
 * judged against this workflow's own median where one exists. Held in
 * place rather than mounted, so a rerun never changes the height of the
 * pane under the pointer that pressed run.
 */
function RunSummary({
  run,
  baseline,
}: {
  run: RunState | null;
  baseline: HistoryBaseline;
}) {
  if (!run) return null;
  const trace = run.trace;
  const failed = run.error != null || trace?.status === "failed";
  const cost = trace?.cost?.total_eur;
  const isDry = run.dry || trace?.dry_run === true;
  // Deltas judge real completed runs only — a dry run's numbers are not
  // comparable to the medians of runs that touched the world.
  const durationDelta =
    !isDry && !failed && trace
      ? deltaVsMedian(trace.duration_ms, baseline.durationMs, 500)
      : null;
  const costDelta =
    !isDry && !failed && trace && cost != null
      ? deltaVsMedian(cost, baseline.costEur, 0.005)
      : null;

  return (
    <div className={`pane-summary is-${runOutcome(run)}`}>
      {trace?.result && (
        <ResultCard
          result={trace.result}
          partial={trace.status === "failed"}
          compact
        />
      )}
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
            {isDry && <span className="pill muted">dry run</span>}
            {trace && (
              <span className="pane-summary-aside">
                <span className="sep">· </span>
                {formatDuration(trace.duration_ms)}
              </span>
            )}
            {durationDelta != null && (
              <DeltaChip delta={durationDelta} format={formatDuration} />
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
          {costDelta != null && (
            <DeltaChip delta={costDelta} format={formatCost} />
          )}
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
  baseline,
}: {
  runs: RunListEntry[];
  loading: boolean;
  error: string | null;
  baseline: HistoryBaseline;
}) {
  const ok = runs.filter((r) => r.status === "succeeded").length;
  const failed = runs.filter((r) => r.status === "failed").length;
  return (
    <section className="pane-history" aria-labelledby="workflow-history-title">
      <div className="pane-history-head">
        <span id="workflow-history-title" className="label">
          Last {runs.length > 0 ? `${runs.length} ` : ""}runs
        </span>
        {runs.length > 0 && (
          <span className="pane-history-count">
            {ok} ok{failed > 0 ? ` · ${failed} failed` : ""}
          </span>
        )}
        {baseline.durationMs != null && (
          <span
            className="pane-history-median"
            title={`Median over ${baseline.samples} real runs`}
          >
            median {formatDuration(baseline.durationMs)}
            {baseline.costEur != null && baseline.costEur > 0
              ? ` · ${formatCost(baseline.costEur)}`
              : ""}
          </span>
        )}
        {runs.length > 0 && (
          <button
            type="button"
            className="pane-history-latest"
            onClick={() =>
              void openRun(runs[0].run_id, {
                key: runs[0].key,
                utc: runs[0].utc,
              })
            }
          >
            open latest →
          </button>
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
        {entry.dry_run && <span className="pane-history-dry">dry</span>}
        {entry.result_headline && (
          <span className="pane-history-headline" title={entry.result_headline}>
            {entry.result_headline}
          </span>
        )}
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
            // Drives the canvas node's live per-step clock.
            started_at_ms: performance.now(),
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
            notes: ev.notes,
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

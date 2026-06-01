import { useState } from "react";
import { useNavigate } from "react-router";
import { Channel } from "@tauri-apps/api/core";
import {
  isIpcError,
  recordTrust,
  resolveWorkflow,
  startRun as ipcStartRun,
  type ConsentRequired,
  type ParameterDef,
  type RunEvent,
  type StepSummary,
  type WorkflowPreflight,
} from "../lib/api";

export function meta() {
  return [{ title: "Run a workflow — Cori" }];
}

export default function RunPage() {
  const [source, setSource] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preflight, setPreflight] = useState<WorkflowPreflight | null>(null);
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [dryRun, setDryRun] = useState(false);
  const [consent, setConsent] = useState<ConsentRequired | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const navigate = useNavigate();

  async function inspect(e: React.FormEvent) {
    e.preventDefault();
    if (!source.trim()) return;
    setLoading(true);
    setError(null);
    setPreflight(null);
    setParams({});
    setConsent(null);
    try {
      const pf = await resolveWorkflow({ source });
      setPreflight(pf);
      const defaults: Record<string, unknown> = {};
      for (const p of pf.manifest.parameters) {
        if (p.default !== undefined && p.default !== null) defaults[p.name] = p.default;
      }
      setParams(defaults);
    } catch (e: unknown) {
      if (isIpcError(e) && e.code === "consent_required") {
        setConsent(e.details as ConsentRequired);
      } else {
        setError(e instanceof Error ? e.message : isIpcError(e) ? e.message : String(e));
      }
    } finally {
      setLoading(false);
    }
  }

  async function trustAndRetry() {
    if (!consent) return;
    setSubmitting(true);
    setError(null);
    try {
      await recordTrust({
        host: consent.host,
        repo: consent.repo,
        subpath: consent.subpath,
        ref_str: consent.ref_str,
        sha: consent.sha,
      });
      setConsent(null);
      await inspect({ preventDefault() {} } as React.FormEvent);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : isIpcError(e) ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }

  async function startRun(e: React.FormEvent) {
    e.preventDefault();
    if (!preflight) return;
    setSubmitting(true);
    setError(null);
    try {
      // A no-op channel — run-live.tsx re-subscribes once we navigate
      // there. The replay buffer on the Rust side preserves events that
      // arrive before the next subscriber attaches.
      const channel = new Channel<RunEvent>();
      const res = await ipcStartRun({
        source,
        params,
        dry_run: dryRun,
        on_event: channel,
      });
      navigate(`/runs/live/${encodeURIComponent(res.run_id)}`);
    } catch (e: unknown) {
      if (isIpcError(e) && e.code === "consent_required") {
        setConsent(e.details as ConsentRequired);
      } else {
        setError(e instanceof Error ? e.message : isIpcError(e) ? e.message : String(e));
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <form onSubmit={inspect} className="card">
        <label
          htmlFor="source"
          style={{ display: "block", fontSize: 13, color: "var(--muted)", marginBottom: 6 }}
        >
          Path or git ref
        </label>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            id="source"
            type="text"
            placeholder="./examples/hello_world or github.com/org/workflows/translate@v1"
            value={source}
            onChange={(e) => setSource(e.target.value)}
            style={{
              flex: 1,
              padding: "8px 10px",
              border: "1px solid var(--rule)",
              borderRadius: 6,
              fontFamily: "var(--font-mono)",
              fontSize: 13,
            }}
          />
          <button type="submit" disabled={loading || !source.trim()} className="btn">
            {loading ? "Inspecting…" : "Inspect"}
          </button>
        </div>
        {error && <p className="hint" style={{ color: "var(--red)" }}>{error}</p>}
      </form>

      {consent && (
        <ConsentModal
          consent={consent}
          onTrust={trustAndRetry}
          onCancel={() => setConsent(null)}
          submitting={submitting}
        />
      )}

      {preflight && (
        <>
          <h2>{preflight.manifest.name}</h2>
          {preflight.manifest.description && (
            <p className="hint">{preflight.manifest.description}</p>
          )}

          {preflight.has_builtin_step && (
            <div className="card" style={{ borderColor: "var(--amber)" }}>
              <strong style={{ color: "var(--amber)" }}>
                ⚠ Builtin step deferred in v1
              </strong>
              <p className="hint" style={{ marginBottom: 0 }}>
                This workflow uses a <code>builtin</code> step (
                <code>map</code> / <code>for_each</code> / <code>branch</code> /{" "}
                <code>parallel</code> / <code>wait</code>). The runtime short-
                circuits builtin steps in v1 — the run will produce a
                "deferred" trace entry instead of executing the step body.
              </p>
            </div>
          )}

          {preflight.missing_capabilities.length > 0 && (
            <div className="card" style={{ borderColor: "var(--red)" }}>
              <strong style={{ color: "var(--red)" }}>
                ✗ Missing capabilities
              </strong>
              <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 13 }}>
                {preflight.missing_capabilities.map((m, i) => (
                  <li key={i}>{m}</li>
                ))}
              </ul>
            </div>
          )}

          <Steps steps={preflight.steps} />

          <form onSubmit={startRun}>
            <h2>Parameters</h2>
            {preflight.manifest.parameters.length === 0 ? (
              <div className="empty">No parameters declared.</div>
            ) : (
              <div className="card">
                {preflight.manifest.parameters.map((p) => (
                  <ParamRow
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

            <div className="card" style={{ display: "flex", gap: 16, alignItems: "center" }}>
              <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <input
                  type="checkbox"
                  checked={dryRun}
                  onChange={(e) => setDryRun(e.target.checked)}
                />
                <span>
                  <strong>Dry run</strong> — validate the plan without executing
                </span>
              </label>
              <div style={{ flex: 1 }} />
              <button
                type="submit"
                className="btn primary"
                disabled={submitting || !preflight.ready || preflight.has_builtin_step}
              >
                {submitting ? "Starting…" : dryRun ? "Validate" : "Run workflow"}
              </button>
            </div>
          </form>
        </>
      )}
    </>
  );
}

function Steps({ steps }: { steps: StepSummary[] }) {
  if (steps.length === 0) return null;
  return (
    <>
      <h2>Steps</h2>
      <div className="card" style={{ padding: 0 }}>
        <ol style={{ margin: 0, padding: "8px 0", listStyle: "none" }}>
          {steps.map((s, i) => (
            <li
              key={s.activity_id}
              style={{
                padding: "10px 16px",
                borderBottom: i < steps.length - 1 ? "1px solid var(--rule)" : "none",
              }}
            >
              <span style={{ color: "var(--muted)", fontFamily: "var(--font-mono)" }}>
                {i + 1}.
              </span>{" "}
              <strong>{s.name}</strong>{" "}
              <span className={`pill ${s.kind === "builtin" ? "warn" : "muted"}`}>
                {s.kind}
              </span>
              {s.description && (
                <div className="hint" style={{ marginTop: 2 }}>{s.description}</div>
              )}
            </li>
          ))}
        </ol>
      </div>
    </>
  );
}

function ParamRow({
  param,
  value,
  onChange,
}: {
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  return (
    <div style={{ marginBottom: 14 }}>
      <label
        htmlFor={`p-${param.name}`}
        style={{ display: "block", fontWeight: 500, fontSize: 14, marginBottom: 4 }}
      >
        {param.name}
        {param.required && <span style={{ color: "var(--red)" }}> *</span>}
        <span
          style={{
            color: "var(--muted)",
            fontWeight: 400,
            fontSize: 12,
            marginLeft: 8,
          }}
        >
          {param.type}
        </span>
      </label>
      {param.description && (
        <div
          className="hint"
          style={{ margin: "0 0 6px", color: "var(--muted-strong)" }}
        >
          {param.description}
        </div>
      )}
      <ParamInput param={param} value={value} onChange={onChange} />
    </div>
  );
}

function ParamInput({
  param,
  value,
  onChange,
}: {
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  const id = `p-${param.name}`;
  const baseStyle: React.CSSProperties = {
    padding: "6px 8px",
    border: "1px solid var(--rule)",
    borderRadius: 6,
    fontFamily: param.type === "path" ? "var(--font-mono)" : "var(--font-sans)",
    fontSize: 13,
    width: "100%",
  };

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
        value={value == null ? "" : String(value)}
        onChange={(e) => onChange(e.target.value)}
        style={baseStyle}
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
        type="number"
        value={value == null ? "" : String(value)}
        min={param.min ?? undefined}
        max={param.max ?? undefined}
        onChange={(e) =>
          onChange(e.target.value === "" ? null : Number(e.target.value))
        }
        style={baseStyle}
      />
    );
  }

  return (
    <input
      id={id}
      type="text"
      value={value == null ? "" : String(value)}
      onChange={(e) => onChange(e.target.value)}
      placeholder={param.type === "path" ? "/abs/path or ./relative" : ""}
      style={baseStyle}
    />
  );
}

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
        <h2 style={{ textTransform: "none", color: "var(--fg)", fontSize: 18 }}>
          Trust this remote workflow?
        </h2>
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
          Trusting records consent for (<code>{consent.host}/{consent.repo}</code>,{" "}
          <code>{consent.sha.slice(0, 12)}</code>) in <code>~/.cori/cache/remote/trust.json</code>.
        </p>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 16 }}>
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

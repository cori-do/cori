import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteSchedule,
  enableSchedule,
  isIpcError,
  listSchedules,
  onScheduleFired,
  resolveWorkflow,
  setScheduleEnabled,
  updateSchedule,
  type ParameterDef,
  type ScheduleDto,
} from "../lib/api";
import { formatAbsolute, formatRelative } from "../lib/format";
import {
  isBlank,
  missingRequired,
  ParamField,
  paramDefaults,
} from "../components/param-fields";

export function meta() {
  return [{ title: "Schedules — Cori" }];
}

export async function clientLoader(): Promise<ScheduleDto[]> {
  return listSchedules();
}

export default function Schedules({ loaderData }: { loaderData: ScheduleDto[] }) {
  return <ScheduleList initialSchedules={loaderData} />;
}

/** Schedules content shared by the launcher and the route wrapper. */
export function ScheduleList({
  initialSchedules,
}: {
  initialSchedules?: ScheduleDto[];
}) {
  const [schedules, setSchedules] = useState(initialSchedules ?? []);
  const [loading, setLoading] = useState(initialSchedules === undefined);
  const [showCreate, setShowCreate] = useState(false);
  const [editing, setEditing] = useState<ScheduleDto | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSchedules(await listSchedules());
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (initialSchedules === undefined) void refresh();
  }, [initialSchedules, refresh]);

  // "Next fire" is a countdown: re-render every second so it ticks, and
  // refetch periodically (plus on every local fire) so a sub-minute cron
  // doesn't drift into showing a fire time that has already passed.
  const [, setNow] = useState(0);
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);
  useEffect(() => {
    const poll = setInterval(() => void refresh(), 15_000);
    let unlisten: (() => void) | undefined;
    void onScheduleFired(() => void refresh()).then((u) => {
      unlisten = u;
    });
    return () => {
      clearInterval(poll);
      unlisten?.();
    };
  }, [refresh]);

  async function toggle(s: ScheduleDto) {
    setBusy(s.id);
    setError(null);
    try {
      await setScheduleEnabled({ id: s.id, enabled: !s.enabled });
      await refresh();
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setBusy(null);
    }
  }

  async function remove(s: ScheduleDto) {
    if (!confirm(`Delete schedule for ${s.source}?`)) return;
    setBusy(s.id);
    setError(null);
    try {
      await deleteSchedule({ id: s.id });
      await refresh();
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <div className="toolbar">
        <div className="spacer" />
        <button className="btn primary" onClick={() => setShowCreate(true)}>
          New schedule
        </button>
      </div>

      {error && (
        <div className="card error">
          <pre style={{ whiteSpace: "pre-wrap" }}>{error}</pre>
        </div>
      )}

      {showCreate && (
        <CreateModal
          onClose={() => setShowCreate(false)}
          onCreated={() => {
            setShowCreate(false);
            void refresh();
          }}
        />
      )}

      {editing && (
        <EditModal
          schedule={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void refresh();
          }}
        />
      )}

      {loading ? (
        <div className="empty">Loading schedules…</div>
      ) : schedules.length === 0 ? (
        <div className="empty">
          No schedules yet. A workflow's manifest must declare a{" "}
          <code>schedule:</code> field to be schedulable; click{" "}
          <strong>New schedule</strong> to register one.
        </div>
      ) : (
        <div>
          {schedules.map((s) => (
            <div
              className="card"
              key={s.id}
              style={{ opacity: s.enabled ? 1 : 0.6 }}
            >
              <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
                <h3 style={{ margin: 0, fontFamily: "var(--font-mono)" }}>{s.source}</h3>
                <span className={`pill ${s.enabled ? "ok" : s.paused_reason ? "warn" : "muted"}`}>
                  {s.enabled ? "enabled" : s.paused_reason ? "paused" : "disabled"}
                </span>
                {!s.is_self_identity && (
                  <span className="pill warn">other identity</span>
                )}
              </div>
              {s.paused_reason && (
                <p className="hint" style={{ color: "var(--amber)", margin: "6px 0 0" }}>
                  {s.paused_reason} — see the Inbox tab.
                </p>
              )}
              <dl className="kv" style={{ margin: "12px 0" }}>
                <dt>When</dt>
                <dd>
                  {describeCron(s.schedule, s.schedule_tz)}
                  <span style={{ color: "var(--muted)", marginLeft: 8, fontFamily: "var(--font-mono)", fontSize: 12 }}>
                    {s.schedule}
                    {s.schedule_tz ? ` (${s.schedule_tz})` : " (UTC)"}
                  </span>
                </dd>
                {s.input && Object.keys(s.input).length > 0 && (
                  <>
                    <dt>Input</dt>
                    <dd>
                      <code style={{ fontSize: 12, wordBreak: "break-word" }}>
                        {JSON.stringify(s.input)}
                      </code>
                    </dd>
                  </>
                )}
                <dt>Owner</dt>
                <dd>{s.identity}</dd>
                <dt>Next fire</dt>
                <dd>
                  {s.next_fire_at
                    ? `${formatAbsolute(s.next_fire_at)} (${formatRelative(
                        s.next_fire_at,
                      )})`
                    : "—"}
                </dd>
                {s.last_fire_at && (
                  <>
                    <dt>Last fire</dt>
                    <dd>
                      {formatRelative(s.last_fire_at)}{" "}
                      <span
                        className={`pill ${
                          s.last_status === "succeeded" ? "ok" : "bad"
                        }`}
                      >
                        {s.last_status ?? "?"}
                      </span>
                      {s.last_error && (
                        <div className="hint" style={{ color: "var(--red)" }}>
                          {s.last_error}
                        </div>
                      )}
                    </dd>
                  </>
                )}
                <dt>Created</dt>
                <dd>{formatRelative(s.created_at)}</dd>
              </dl>
              {s.is_self_identity && (
                <div style={{ display: "flex", gap: 8 }}>
                  <button
                    className="btn"
                    disabled={busy === s.id}
                    onClick={() => setEditing(s)}
                  >
                    Edit
                  </button>
                  <button
                    className="btn"
                    disabled={busy === s.id}
                    onClick={() => toggle(s)}
                  >
                    {s.enabled ? "Disable" : "Enable"}
                  </button>
                  <button
                    className="btn"
                    disabled={busy === s.id}
                    onClick={() => remove(s)}
                  >
                    Delete
                  </button>
                </div>
              )}
              {!s.is_self_identity && (
                <p className="hint">
                  Owned by <code>{s.identity}</code>. To modify, open the Console
                  from <code>cori work</code> running under that identity.
                </p>
              )}
            </div>
          ))}
        </div>
      )}
    </>
  );
}

// ─── Plain-language timing model ─────────────────────────────────────────
//
// The picker emits a 6-field cron (sec min hour dom month dow) with named
// weekdays. `parseTiming` inverts exactly the shapes the picker emits —
// anything else opens in "Custom cron" mode with the raw string intact,
// so hand-written expressions are never mangled.

type Timing =
  | { mode: "daily"; time: string }
  | { mode: "weekly"; days: string[]; time: string }
  | { mode: "hourly"; minute: number }
  | { mode: "monthly"; dom: number; time: string }
  | { mode: "custom"; cron: string };

const WEEKDAYS = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"] as const;

function timingToCron(t: Timing): string {
  const hm = (tt: string): [number, number] => {
    const [hh, mm] = tt.split(":");
    return [Number(hh) || 0, Number(mm) || 0];
  };
  switch (t.mode) {
    case "daily": {
      const [hh, mm] = hm(t.time);
      return `0 ${mm} ${hh} * * *`;
    }
    case "weekly": {
      const [hh, mm] = hm(t.time);
      const days = t.days.length ? t.days.join(",") : "*";
      return `0 ${mm} ${hh} * * ${days}`;
    }
    case "hourly":
      return `0 ${t.minute} * * * *`;
    case "monthly": {
      const [hh, mm] = hm(t.time);
      return `0 ${mm} ${hh} ${t.dom} * *`;
    }
    case "custom":
      return t.cron.trim();
  }
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function parseTiming(cron: string): Timing {
  const f = cron.trim().split(/\s+/);
  if (f.length === 6 && f[0] === "0") {
    const [, min, hour, dom, month, dow] = f;
    const timeOk = /^\d+$/.test(min) && /^\d+$/.test(hour);
    const time = timeOk ? `${pad2(Number(hour))}:${pad2(Number(min))}` : null;
    if (time && dom === "*" && month === "*" && dow === "*") {
      return { mode: "daily", time };
    }
    if (
      time &&
      dom === "*" &&
      month === "*" &&
      dow.split(",").every((d) => (WEEKDAYS as readonly string[]).includes(d))
    ) {
      return { mode: "weekly", days: dow.split(","), time };
    }
    if (/^\d+$/.test(min) && hour === "*" && dom === "*" && month === "*" && dow === "*") {
      return { mode: "hourly", minute: Number(min) };
    }
    if (time && /^\d+$/.test(dom) && month === "*" && dow === "*") {
      return { mode: "monthly", dom: Number(dom), time };
    }
  }
  return { mode: "custom", cron };
}

/** One-line human sentence for a cron, used on cards and as live preview. */
function describeCron(cron: string, tz?: string | null): string {
  const t = parseTiming(cron);
  const zone = tz ? "" : " (UTC)";
  switch (t.mode) {
    case "daily":
      return `Every day at ${t.time}${zone}`;
    case "weekly":
      return `Every ${t.days.map(titleDay).join(", ")} at ${t.time}${zone}`;
    case "hourly":
      return `Every hour at :${pad2(t.minute)}${zone}`;
    case "monthly":
      return `Day ${t.dom} of each month at ${t.time}${zone}`;
    case "custom":
      return describeIntervalCron(t.cron) ?? `Custom cron${zone}`;
  }
}

// Recognize hand-written step crons (every N seconds/minutes) that the
// picker has no mode for, so cards still read as a sentence.
function describeIntervalCron(cron: string): string | null {
  const f = cron.trim().split(/\s+/);
  if (f.length !== 6) return null;
  const every = (field: string) => /^\*\/\d+$/.test(field);
  const rest = (from: number) => f.slice(from).every((x) => x === "*");
  if (every(f[0]) && rest(1)) return `Every ${f[0].slice(2)} seconds`;
  if (f[0] === "0" && every(f[1]) && rest(2)) return `Every ${f[1].slice(2)} minutes`;
  return null;
}

function titleDay(d: string): string {
  const names: Record<string, string> = {
    MON: "Monday", TUE: "Tuesday", WED: "Wednesday", THU: "Thursday",
    FRI: "Friday", SAT: "Saturday", SUN: "Sunday",
  };
  return names[d] ?? d;
}

const systemTz = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone ?? "UTC";
  } catch {
    return "UTC";
  }
};

const COMMON_TZS = [
  "Europe/Paris", "Europe/London", "Europe/Berlin", "America/New_York",
  "America/Chicago", "America/Los_Angeles", "Asia/Tokyo", "Asia/Singapore",
  "Australia/Sydney", "UTC",
];

function TimingFields({
  timing,
  onTiming,
  tz,
  onTz,
}: {
  timing: Timing;
  onTiming: (t: Timing) => void;
  tz: string;
  onTz: (tz: string) => void;
}) {
  const cron = useMemo(() => timingToCron(timing), [timing]);
  function setMode(mode: Timing["mode"]) {
    if (mode === "daily") onTiming({ mode, time: "09:00" });
    else if (mode === "weekly") onTiming({ mode, days: ["MON"], time: "09:00" });
    else if (mode === "hourly") onTiming({ mode, minute: 0 });
    else if (mode === "monthly") onTiming({ mode, dom: 1, time: "09:00" });
    else onTiming({ mode: "custom", cron });
  }

  return (
    <>
      <div style={{ marginBottom: 12 }}>
        <label className="label" style={labelStyle}>When</label>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <div className="schedule-mode-picker" role="group" aria-label="Schedule interval">
            {[
              ["daily", "Every day"],
              ["weekly", "Every week"],
              ["hourly", "Every hour"],
              ["monthly", "Every month"],
              ["custom", "Custom cron"],
            ].map(([mode, label]) => (
              <button
                key={mode}
                type="button"
                className={`btn${timing.mode === mode ? " primary" : ""}`}
                aria-pressed={timing.mode === mode}
                onClick={() => setMode(mode as Timing["mode"])}
              >
                {label}
              </button>
            ))}
          </div>

          {(timing.mode === "daily" ||
            timing.mode === "weekly" ||
            timing.mode === "monthly") && (
            <input
              type="time"
              value={timing.time}
              onChange={(e) => onTiming({ ...timing, time: e.target.value })}
              style={inputStyle}
              aria-label="Time of day"
            />
          )}
          {timing.mode === "hourly" && (
            <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 13 }}>
              at minute
              <input
                type="number"
                min={0}
                max={59}
                value={timing.minute}
                onChange={(e) =>
                  onTiming({ mode: "hourly", minute: Math.min(59, Math.max(0, Number(e.target.value) || 0)) })
                }
                style={{ ...inputStyle, width: 70 }}
              />
            </label>
          )}
          {timing.mode === "monthly" && (
            <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 13 }}>
              on day
              <input
                type="number"
                min={1}
                max={31}
                value={timing.dom}
                onChange={(e) =>
                  onTiming({ ...timing, dom: Math.min(31, Math.max(1, Number(e.target.value) || 1)) })
                }
                style={{ ...inputStyle, width: 70 }}
              />
            </label>
          )}
        </div>

        {timing.mode === "weekly" && (
          <div style={{ display: "flex", gap: 4, marginTop: 8, flexWrap: "wrap" }}>
            {WEEKDAYS.map((d) => {
              const active = timing.days.includes(d);
              return (
                <button
                  key={d}
                  type="button"
                  className={`btn${active ? " primary" : ""}`}
                  style={{ padding: "3px 8px", fontSize: 12 }}
                  aria-pressed={active}
                  onClick={() =>
                    onTiming({
                      ...timing,
                      days: active
                        ? timing.days.filter((x) => x !== d)
                        : [...WEEKDAYS.filter((w) => timing.days.includes(w) || w === d)],
                    })
                  }
                >
                  {d.charAt(0) + d.slice(1).toLowerCase()}
                </button>
              );
            })}
          </div>
        )}

        {timing.mode === "custom" && (
          <input
            type="text"
            value={timing.cron}
            onChange={(e) => onTiming({ mode: "custom", cron: e.target.value })}
            placeholder="sec min hour dom month dow  —  e.g. 0 30 9 * * MON-FRI"
            style={{ ...inputStyle, width: "100%", marginTop: 8 }}
            aria-label="Cron expression"
          />
        )}
      </div>

      <div style={{ marginBottom: 12 }}>
        <label htmlFor="tzpick" className="label" style={labelStyle}>
          Timezone
        </label>
        <input
          id="tzpick"
          type="text"
          list="tz-suggestions"
          value={tz}
          onChange={(e) => onTz(e.target.value)}
          style={{ ...inputStyle, width: "100%" }}
        />
        <datalist id="tz-suggestions">
          {[systemTz(), ...COMMON_TZS]
            .filter((z, i, a) => a.indexOf(z) === i)
            .map((z) => (
              <option key={z} value={z} />
            ))}
        </datalist>
      </div>

      <p className="hint" style={{ marginTop: 0 }}>
        {describeCron(cron, tz || null)}
        {tz ? ` — ${tz} wall clock` : ""} ·{" "}
        <code style={{ fontSize: 11.5 }}>{cron}</code>
      </p>
    </>
  );
}

// ─── Schedule input (workflow parameters) ────────────────────────────────
//
// A schedule fires unattended, so the workflow's input is captured when
// the schedule is created/edited — the same parameter form the launcher
// shows before a manual run. Without this, any workflow with required
// parameters would fail schema validation on every single fire.

interface LoadedParams {
  /** Which source these parameters belong to. */
  source: string;
  params: ParameterDef[] | null;
  error: string | null;
}

function ParamsSection({
  loaded,
  loading,
  values,
  onValues,
}: {
  loaded: LoadedParams | null;
  loading: boolean;
  values: Record<string, unknown>;
  onValues: (v: Record<string, unknown>) => void;
}) {
  if (loading) {
    return <p className="hint">Loading workflow parameters…</p>;
  }
  if (!loaded) return null;
  if (loaded.error) {
    return (
      <p className="hint" style={{ color: "var(--amber)" }}>
        Couldn't load workflow parameters: {loaded.error}
      </p>
    );
  }
  if (!loaded.params || loaded.params.length === 0) return null;

  const missing = missingRequired(loaded.params, values);
  return (
    <div style={{ marginBottom: 12 }}>
      <label className="label" style={labelStyle}>
        Input — used for every scheduled run
      </label>
      {loaded.params.map((p) => (
        <ParamField
          key={p.name}
          param={p}
          value={values[p.name]}
          onChange={(v) => onValues({ ...values, [p.name]: v })}
        />
      ))}
      {missing.length > 0 && (
        <p className="hint" style={{ color: "var(--amber)", marginBottom: 0 }}>
          Required: {missing.join(", ")}
        </p>
      )}
    </div>
  );
}

/** Collapse the form's values into the input object sent to the backend. */
function buildInput(
  params: ParameterDef[] | null,
  values: Record<string, unknown>,
): Record<string, unknown> {
  const input: Record<string, unknown> = {};
  for (const p of params ?? []) {
    const v = values[p.name];
    if (!isBlank(v)) input[p.name] = v;
  }
  return input;
}

function CreateModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [source, setSource] = useState("");
  const [timing, setTiming] = useState<Timing>({ mode: "daily", time: "09:00" });
  const [tz, setTz] = useState(systemTz());
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [loaded, setLoaded] = useState<LoadedParams | null>(null);
  const [loadingParams, setLoadingParams] = useState(false);
  const [values, setValues] = useState<Record<string, unknown>>({});

  // Resolve the workflow when the source field settles (blur), so the
  // parameter form appears before the user hits Register.
  async function loadParams() {
    const src = source.trim();
    if (!src || src === loaded?.source) return;
    setLoadingParams(true);
    try {
      const pf = await resolveWorkflow({ source: src });
      setLoaded({ source: src, params: pf.manifest.parameters, error: null });
      setValues(paramDefaults(pf.manifest.parameters));
    } catch (e: unknown) {
      setLoaded({ source: src, params: null, error: formatErr(e) });
      setValues({});
    } finally {
      setLoadingParams(false);
    }
  }

  const missing =
    loaded?.params != null ? missingRequired(loaded.params, values) : [];

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!source.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      const input = buildInput(loaded?.params ?? null, values);
      await enableSchedule({
        source,
        schedule: timingToCron(timing),
        schedule_tz: tz.trim() || undefined,
        input: Object.keys(input).length > 0 ? input : undefined,
      });
      onCreated();
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h2>New schedule</h2>
        <form onSubmit={submit}>
          <div style={{ marginBottom: 12 }}>
            <label htmlFor="src" className="label" style={labelStyle}>
              Source (path or git ref)
            </label>
            <input
              id="src"
              type="text"
              required
              value={source}
              onChange={(e) => setSource(e.target.value)}
              onBlur={() => void loadParams()}
              style={{ ...inputStyle, width: "100%" }}
            />
          </div>

          <ParamsSection
            loaded={loaded}
            loading={loadingParams}
            values={values}
            onValues={setValues}
          />

          <TimingFields timing={timing} onTiming={setTiming} tz={tz} onTz={setTz} />

          {error && (
            <p className="hint" style={{ color: "var(--red)" }}>{error}</p>
          )}
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 16 }}>
            <button type="button" className="btn" onClick={onClose} disabled={submitting}>
              Cancel
            </button>
            <button
              type="submit"
              className="btn primary"
              disabled={submitting || !source.trim() || missing.length > 0}
            >
              {submitting ? "Registering…" : "Register"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function EditModal({
  schedule: s,
  onClose,
  onSaved,
}: {
  schedule: ScheduleDto;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [timing, setTiming] = useState<Timing>(() => parseTiming(s.schedule));
  const [tz, setTz] = useState(s.schedule_tz ?? "");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [loaded, setLoaded] = useState<LoadedParams | null>(null);
  const [loadingParams, setLoadingParams] = useState(true);
  const [values, setValues] = useState<Record<string, unknown>>({});

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const pf = await resolveWorkflow({ source: s.source });
        if (cancelled) return;
        setLoaded({ source: s.source, params: pf.manifest.parameters, error: null });
        setValues({ ...paramDefaults(pf.manifest.parameters), ...(s.input ?? {}) });
      } catch (e: unknown) {
        if (cancelled) return;
        // Source unresolvable right now (e.g. offline for a remote ref):
        // timing stays editable and the stored input is left untouched.
        setLoaded({ source: s.source, params: null, error: formatErr(e) });
      } finally {
        if (!cancelled) setLoadingParams(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [s.source, s.input]);

  const missing =
    loaded?.params != null ? missingRequired(loaded.params, values) : [];

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await updateSchedule({
        id: s.id,
        schedule: timingToCron(timing),
        schedule_tz: tz.trim() || undefined,
        // Only replace the stored input when the parameter form actually
        // loaded; omitting the field keeps whatever the schedule had.
        input: loaded?.params != null ? buildInput(loaded.params, values) : undefined,
      });
      onSaved();
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h2>Edit schedule</h2>
        <p className="hint" style={{ fontFamily: "var(--font-mono)" }}>{s.source}</p>
        <form onSubmit={submit}>
          <ParamsSection
            loaded={loaded}
            loading={loadingParams}
            values={values}
            onValues={setValues}
          />
          <TimingFields timing={timing} onTiming={setTiming} tz={tz} onTz={setTz} />
          {error && (
            <p className="hint" style={{ color: "var(--red)" }}>{error}</p>
          )}
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 16 }}>
            <button type="button" className="btn" onClick={onClose} disabled={submitting}>
              Cancel
            </button>
            <button
              type="submit"
              className="btn primary"
              disabled={submitting || missing.length > 0}
            >
              {submitting ? "Saving…" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

// Field chrome now comes from `.label` / the global input styles; these
// only carry what is per-field (block layout, intrinsic width).
const labelStyle: React.CSSProperties = {
  display: "block",
  marginBottom: 7,
};

const inputStyle: React.CSSProperties = { width: "100%" };

function formatErr(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

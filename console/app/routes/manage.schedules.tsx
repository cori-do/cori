import { useMemo, useState } from "react";
import { useRevalidator } from "react-router";
import {
  deleteSchedule,
  enableSchedule,
  isIpcError,
  listSchedules,
  setScheduleEnabled,
  updateSchedule,
  type ScheduleDto,
} from "../lib/api";
import { formatAbsolute, formatRelative } from "../lib/format";

export function meta() {
  return [{ title: "Schedules — Cori" }];
}

export async function clientLoader(): Promise<ScheduleDto[]> {
  return listSchedules();
}

export default function Schedules({ loaderData }: { loaderData: ScheduleDto[] }) {
  const schedules = loaderData;
  const revalidator = useRevalidator();
  const [showCreate, setShowCreate] = useState(false);
  const [editing, setEditing] = useState<ScheduleDto | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function toggle(s: ScheduleDto) {
    setBusy(s.id);
    setError(null);
    try {
      await setScheduleEnabled({ id: s.id, enabled: !s.enabled });
      revalidator.revalidate();
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
      revalidator.revalidate();
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
            revalidator.revalidate();
          }}
        />
      )}

      {editing && (
        <EditModal
          schedule={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            revalidator.revalidate();
          }}
        />
      )}

      {schedules.length === 0 ? (
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
                    Edit timing
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
      return `Custom cron${zone}`;
  }
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
  return (
    <>
      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>When</label>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <select
            value={timing.mode}
            onChange={(e) => {
              const mode = e.target.value as Timing["mode"];
              if (mode === "daily") onTiming({ mode, time: "09:00" });
              else if (mode === "weekly") onTiming({ mode, days: ["MON"], time: "09:00" });
              else if (mode === "hourly") onTiming({ mode, minute: 0 });
              else if (mode === "monthly") onTiming({ mode, dom: 1, time: "09:00" });
              else onTiming({ mode: "custom", cron });
            }}
            style={inputStyle}
          >
            <option value="daily">Every day</option>
            <option value="weekly">Every week</option>
            <option value="hourly">Every hour</option>
            <option value="monthly">Every month</option>
            <option value="custom">Custom cron</option>
          </select>

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
        <label htmlFor="tzpick" style={labelStyle}>Timezone</label>
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

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!source.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      await enableSchedule({
        source,
        schedule: timingToCron(timing),
        schedule_tz: tz.trim() || undefined,
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
        <h2 style={{ textTransform: "none", color: "var(--fg)", fontSize: 18 }}>
          New schedule
        </h2>
        <form onSubmit={submit}>
          <div style={{ marginBottom: 12 }}>
            <label htmlFor="src" style={labelStyle}>
              Source (path or git ref)
            </label>
            <input
              id="src"
              type="text"
              required
              value={source}
              onChange={(e) => setSource(e.target.value)}
              style={{ ...inputStyle, width: "100%" }}
            />
          </div>

          <TimingFields timing={timing} onTiming={setTiming} tz={tz} onTz={setTz} />

          {error && (
            <p className="hint" style={{ color: "var(--red)" }}>{error}</p>
          )}
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 16 }}>
            <button type="button" className="btn" onClick={onClose} disabled={submitting}>
              Cancel
            </button>
            <button type="submit" className="btn primary" disabled={submitting || !source.trim()}>
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

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await updateSchedule({
        id: s.id,
        schedule: timingToCron(timing),
        schedule_tz: tz.trim() || undefined,
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
        <h2 style={{ textTransform: "none", color: "var(--fg)", fontSize: 18 }}>
          Edit timing
        </h2>
        <p className="hint" style={{ fontFamily: "var(--font-mono)" }}>{s.source}</p>
        <form onSubmit={submit}>
          <TimingFields timing={timing} onTiming={setTiming} tz={tz} onTz={setTz} />
          {error && (
            <p className="hint" style={{ color: "var(--red)" }}>{error}</p>
          )}
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 16 }}>
            <button type="button" className="btn" onClick={onClose} disabled={submitting}>
              Cancel
            </button>
            <button type="submit" className="btn primary" disabled={submitting}>
              {submitting ? "Saving…" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

const labelStyle: React.CSSProperties = {
  display: "block",
  fontSize: 13,
  color: "var(--muted)",
  marginBottom: 4,
};

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: "6px 8px",
  border: "1px solid var(--rule)",
  borderRadius: 6,
  fontFamily: "var(--font-mono)",
  fontSize: 13,
};

function formatErr(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

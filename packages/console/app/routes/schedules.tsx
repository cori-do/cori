import { useState } from "react";
import { useRevalidator } from "react-router";
import {
  apiDelete,
  apiGet,
  apiPatch,
  apiPost,
  ApiError,
  type CreateScheduleBody,
  type ScheduleDto,
} from "../lib/api";
import { formatAbsolute, formatRelative } from "../lib/format";

export function meta() {
  return [{ title: "Schedules — Cori Console" }];
}

export async function clientLoader(): Promise<ScheduleDto[]> {
  return apiGet<ScheduleDto[]>("/api/schedules");
}

export default function Schedules({ loaderData }: { loaderData: ScheduleDto[] }) {
  const schedules = loaderData;
  const revalidator = useRevalidator();
  const [showCreate, setShowCreate] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function toggle(s: ScheduleDto) {
    setBusy(s.id);
    setError(null);
    try {
      await apiPatch(`/api/schedules/${encodeURIComponent(s.id)}`, {
        enabled: !s.enabled,
      });
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
      await apiDelete(`/api/schedules/${encodeURIComponent(s.id)}`);
      revalidator.revalidate();
    } catch (e: unknown) {
      setError(formatErr(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
        <h1 style={{ flex: 1 }}>Schedules</h1>
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
                <span className={`pill ${s.enabled ? "ok" : "muted"}`}>
                  {s.enabled ? "enabled" : "disabled"}
                </span>
                {!s.is_self_identity && (
                  <span className="pill warn">other identity</span>
                )}
              </div>
              <dl className="kv" style={{ margin: "12px 0" }}>
                <dt>Cron</dt>
                <dd>
                  {s.schedule}
                  {s.schedule_tz ? ` (${s.schedule_tz})` : ""}
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

function CreateModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [source, setSource] = useState("");
  const [schedule, setSchedule] = useState("");
  const [scheduleTz, setScheduleTz] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!source.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      const body: CreateScheduleBody = { source };
      if (schedule.trim()) body.schedule = schedule.trim();
      if (scheduleTz.trim()) body.schedule_tz = scheduleTz.trim();
      await apiPost("/api/schedules", body);
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
          Register a schedule
        </h2>
        <form onSubmit={submit}>
          <p className="hint">
            Cron + timezone fall back to the manifest's <code>schedule</code> /{" "}
            <code>schedule_tz</code> if you leave them blank. Workflows without
            a <code>schedule:</code> field require an explicit cron here.
          </p>
          <div style={{ marginBottom: 12 }}>
            <label
              htmlFor="src"
              style={{ display: "block", fontSize: 13, color: "var(--muted)", marginBottom: 4 }}
            >
              Source (path or git ref)
            </label>
            <input
              id="src"
              type="text"
              required
              value={source}
              onChange={(e) => setSource(e.target.value)}
              style={inputStyle}
            />
          </div>
          <div style={{ marginBottom: 12 }}>
            <label
              htmlFor="cron"
              style={{ display: "block", fontSize: 13, color: "var(--muted)", marginBottom: 4 }}
            >
              Cron (5 or 6 fields)
            </label>
            <input
              id="cron"
              type="text"
              placeholder="0 9 * * *  (manifest default if blank)"
              value={schedule}
              onChange={(e) => setSchedule(e.target.value)}
              style={inputStyle}
            />
          </div>
          <div style={{ marginBottom: 12 }}>
            <label
              htmlFor="tz"
              style={{ display: "block", fontSize: 13, color: "var(--muted)", marginBottom: 4 }}
            >
              Timezone (IANA, optional)
            </label>
            <input
              id="tz"
              type="text"
              placeholder="Europe/Paris (manifest default if blank)"
              value={scheduleTz}
              onChange={(e) => setScheduleTz(e.target.value)}
              style={inputStyle}
            />
          </div>
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

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: "6px 8px",
  border: "1px solid var(--rule)",
  borderRadius: 6,
  fontFamily: "var(--font-mono)",
  fontSize: 13,
};

function formatErr(e: unknown): string {
  if (e instanceof ApiError) {
    const body = e.body as { error?: string } | null;
    return body?.error ?? e.message;
  }
  if (e instanceof Error) return e.message;
  return String(e);
}

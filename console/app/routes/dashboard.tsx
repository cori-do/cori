import { useEffect, useState } from "react";
import { Link, useRevalidator } from "react-router";
import {
  getStackStatus,
  getStatus,
  listRecentWorkflows,
  onStackStatus,
  type RecentWorkflow,
  type StackStatus,
  type StatusResponse,
} from "../lib/api";
import { formatRelative } from "../lib/format";

interface DashboardData {
  status: StatusResponse;
  recents: RecentWorkflow[];
}

export async function clientLoader(): Promise<DashboardData> {
  const [status, recents] = await Promise.all([
    getStatus(),
    listRecentWorkflows(),
  ]);
  return { status, recents };
}

function identityLabel(id: StatusResponse["identity"]): string {
  if (id.kind === "person") return id.user_id;
  if (id.kind === "service") return `service:${id.pool}`;
  return "unknown";
}

interface StackIndicator {
  label: string;
  className: string;
}

function stackIndicator(
  live: StackStatus | undefined,
  loaderReachable: boolean,
): StackIndicator {
  if (live) {
    switch (live.state) {
      case "starting":
        return { label: "Starting…", className: "warn" };
      case "up":
        return { label: "✓ reachable", className: "ok" };
      case "degraded":
        return {
          label: `Degraded${live.reason ? `: ${live.reason}` : ""}`,
          className: "warn",
        };
      case "down":
        return {
          label: `✗ down${live.reason ? `: ${live.reason}` : ""}`,
          className: "bad",
        };
    }
  }
  // No live event yet — fall back to the loader's reachable snapshot.
  return loaderReachable
    ? { label: "✓ reachable", className: "ok" }
    : { label: "Starting…", className: "warn" };
}

export default function Dashboard({
  loaderData,
}: {
  loaderData: DashboardData;
}) {
  const { status, recents } = loaderData;
  const [liveStatus, setLiveStatus] = useState<StackStatus | undefined>(undefined);
  const revalidator = useRevalidator();

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let lastState: StackStatus["state"] | undefined;

    // Helper applied to both the one-shot snapshot and live updates.
    // Revalidates the loader on the first `→ up` transition so the
    // endpoint URL, capabilities, and workers refresh against the now-
    // online Temporal.
    const apply = (s: StackStatus) => {
      if (cancelled) return;
      setLiveStatus(s);
      if (s.state === "up" && lastState !== "up") {
        revalidator.revalidate();
      }
      lastState = s.state;
    };

    // Seed with the current snapshot in case the supervisor already
    // fired its events before we mounted.
    getStackStatus()
      .then((s) => apply(s))
      .catch(() => {
        /* command not yet available; live events will catch up */
      });

    // Subscribe to live transitions.
    onStackStatus(apply)
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {
        /* listen() can throw if the event runtime isn't ready; ignore. */
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // The revalidator is stable across renders; intentionally omitted.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const indicator = stackIndicator(liveStatus, status.reachable);

  return (
    <>
      <div className="status-strip">
        <div className="card">
          <div className="label">Temporal endpoint</div>
          <div className="value mono">{status.endpoint}</div>
          <div className={`value ${indicator.className}`}>{indicator.label}</div>
        </div>
        <div className="card">
          <div className="label">Identity</div>
          <div className="value">{identityLabel(status.identity)}</div>
          <div className="value mono" style={{ fontSize: 12, color: "var(--muted)" }}>
            {status.task_queue}
          </div>
        </div>
        <div className="card">
          <div className="label">Workers online</div>
          <div className="value">{status.workers.length}</div>
        </div>
        <div className="card">
          <div className="label">Capabilities</div>
          <div className="value">
            {status.self_report.capabilities.filter((c) => c.authed).length}
            <span style={{ color: "var(--muted)" }}>
              {" / "}
              {status.self_report.capabilities.length}
            </span>
          </div>
        </div>
      </div>

      <h2>Capabilities on this machine</h2>
      {status.self_report.capabilities.length === 0 ? (
        <div className="empty">No capabilities discovered yet.</div>
      ) : (
        <div className="cap-grid">
          {status.self_report.capabilities.map((c) => (
            <div className="cap" key={`${c.kind}-${c.id}`}>
              <span className={c.authed ? "check" : "cross"}>
                {c.authed ? "✓" : "✗"}
              </span>
              <span className="name">{c.id}</span>
              <span className="meta">{c.kind}</span>
            </div>
          ))}
        </div>
      )}

      <h2>Recent workflows</h2>
      {recents.length === 0 ? (
        <div className="empty">
          No runs yet. Start one with <code>cori run &lt;path-or-ref&gt;</code> from the CLI.
        </div>
      ) : (
        <table className="runs">
          <thead>
            <tr>
              <th>Workflow</th>
              <th>Last run</th>
              <th>Status</th>
              <th>Runs</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {recents.slice(0, 10).map((w) => (
              <tr key={w.key}>
                <td>
                  <strong>{w.workflow_id}</strong>
                  <div style={{ color: "var(--muted)", fontSize: 12, fontFamily: "var(--font-mono)" }}>
                    {w.key}
                  </div>
                </td>
                <td>{formatRelative(w.last_run_at)}</td>
                <td>
                  <span className={`pill ${pillFor(w.last_status)}`}>{w.last_status}</span>
                </td>
                <td>{w.run_count}</td>
                <td>
                  <Link to={`/runs?workflow_id=${encodeURIComponent(w.workflow_id)}`}>
                    history →
                  </Link>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  return "muted";
}

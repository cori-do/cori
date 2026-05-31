import { Link } from "react-router";
import {
  apiGet,
  type RecentWorkflow,
  type StatusResponse,
} from "../lib/api";
import { formatRelative } from "../lib/format";

interface DashboardData {
  status: StatusResponse;
  recents: RecentWorkflow[];
}

export async function clientLoader(): Promise<DashboardData> {
  const [status, recents] = await Promise.all([
    apiGet<StatusResponse>("/api/status"),
    apiGet<RecentWorkflow[]>("/api/workflows/recent"),
  ]);
  return { status, recents };
}

function identityLabel(id: StatusResponse["identity"]): string {
  if (id.Person) return id.Person.user_id;
  if (id.Service) return `service:${id.Service.pool}`;
  return "unknown";
}

export default function Dashboard({
  loaderData,
}: {
  loaderData: DashboardData;
}) {
  const { status, recents } = loaderData;
  const reachable = status.reachable;
  return (
    <>
      <h1>Dashboard</h1>

      <div className="status-strip">
        <div className="card">
          <div className="label">Temporal endpoint</div>
          <div className="value mono">{status.endpoint}</div>
          <div className={`value ${reachable ? "ok" : "bad"}`}>
            {reachable ? "✓ reachable" : "✗ not reachable"}
          </div>
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

import { Link, useSearchParams } from "react-router";
import { apiGet, type RunListEntry } from "../lib/api";
import { formatCost, formatDuration, formatRelative } from "../lib/format";

interface LoaderData {
  runs: RunListEntry[];
  filter: string | null;
}

export async function clientLoader({
  request,
}: {
  request: Request;
}): Promise<LoaderData> {
  const url = new URL(request.url);
  const filter = url.searchParams.get("workflow_id");
  const qs = new URLSearchParams({ limit: "100" });
  if (filter) qs.set("workflow_id", filter);
  const runs = await apiGet<RunListEntry[]>(`/api/runs?${qs.toString()}`);
  return { runs, filter };
}

export default function Runs({ loaderData }: { loaderData: LoaderData }) {
  const { runs, filter } = loaderData;
  const [, setSearchParams] = useSearchParams();
  return (
    <>
      <h1>{filter ? `Runs — ${filter}` : "Runs"}</h1>
      {filter && (
        <p className="hint">
          Filtered to <code>{filter}</code>.{" "}
          <button
            type="button"
            onClick={() => setSearchParams({})}
            style={{
              background: "none",
              border: "none",
              color: "var(--accent)",
              cursor: "pointer",
              padding: 0,
            }}
          >
            clear filter
          </button>
        </p>
      )}

      {runs.length === 0 ? (
        <div className="empty">
          No runs recorded yet. Start one from{" "}
          <Link to="/run">Run</Link> or via{" "}
          <code>cori run &lt;path-or-ref&gt;</code>.
        </div>
      ) : (
        <table className="runs">
          <thead>
            <tr>
              <th>When</th>
              <th>Workflow</th>
              <th>Status</th>
              <th>Duration</th>
              <th>Cost</th>
              <th>Run id</th>
            </tr>
          </thead>
          <tbody>
            {runs.map((r) => (
              <tr key={r.run_id} className="row-link">
                <RowCell run={r}>
                  <span title={r.started_at}>{formatRelative(r.started_at)}</span>
                </RowCell>
                <RowCell run={r}>
                  <strong>{r.workflow_id}</strong>
                </RowCell>
                <RowCell run={r}>
                  <span className={`pill ${pillFor(r.status)}`}>{r.status}</span>
                </RowCell>
                <RowCell run={r}>{formatDuration(r.duration_ms)}</RowCell>
                <RowCell run={r}>{formatCost(r.cost?.total_eur)}</RowCell>
                <RowCell run={r} mono>
                  {r.run_id.slice(0, 8)}
                </RowCell>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <p className="hint">
        Click any row for the full step-by-step trace. Same data{" "}
        <code>cori runs show &lt;run_id&gt;</code> prints.
      </p>
    </>
  );
}

function RowCell({
  run,
  children,
  mono,
}: {
  run: RunListEntry;
  children: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <td className={mono ? "mono" : undefined}>
      <Link
        to={`/runs/${encodeURIComponent(run.key)}/${encodeURIComponent(run.utc)}`}
        style={{
          color: "inherit",
          textDecoration: "none",
          display: "block",
        }}
      >
        {children}
      </Link>
    </td>
  );
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  return "muted";
}

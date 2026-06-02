import { useSearchParams } from "react-router";
import { listRuns, type RunListEntry } from "../lib/api";
import { formatCost, formatDuration, formatRelative } from "../lib/format";
import { openRun } from "../lib/windows";

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
  const runs = await listRuns({
    workflow_id: filter ?? undefined,
    limit: 100,
  });
  return { runs, filter };
}

export default function Runs({ loaderData }: { loaderData: LoaderData }) {
  const { runs, filter } = loaderData;
  const [, setSearchParams] = useSearchParams();
  return (
    <>
      {filter && (
        <div className="toolbar">
          <p className="hint" style={{ margin: 0 }}>
            Filtered to <code>{filter}</code>
          </p>
          <div className="spacer" />
          <button
            type="button"
            className="btn"
            onClick={() => setSearchParams({})}
          >
            Clear filter
          </button>
        </div>
      )}

      {runs.length === 0 ? (
        <div className="empty">
          No runs recorded yet. Launch one from the Cori launcher (recents
          or path/ref input), or via{" "}
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
              <tr
                key={r.run_id}
                className="row-link"
                onClick={() =>
                  void openRun(r.run_id, { key: r.key, utc: r.utc })
                }
                style={{ cursor: "pointer" }}
              >
                <td>
                  <span title={r.started_at}>{formatRelative(r.started_at)}</span>
                </td>
                <td>
                  <strong>{r.workflow_id}</strong>
                </td>
                <td>
                  <span className={`pill ${pillFor(r.status)}`}>{r.status}</span>
                </td>
                <td>{formatDuration(r.duration_ms)}</td>
                <td>{formatCost(r.cost?.total_eur)}</td>
                <td className="mono">{r.run_id.slice(0, 8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <p className="hint">
        Click any row to open the full step-by-step trace in its own window.
        Same data <code>cori runs show &lt;run_id&gt;</code> prints.
      </p>
    </>
  );
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  return "muted";
}

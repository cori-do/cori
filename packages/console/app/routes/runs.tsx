import { Link, useSearchParams } from "react-router";
import { apiGet, type RunTrace } from "../lib/api";
import { formatCost, formatDuration, formatRelative } from "../lib/format";

interface LoaderData {
  runs: RunTrace[];
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
  const runs = await apiGet<RunTrace[]>(`/api/runs?${qs.toString()}`);
  return { runs, filter };
}

function runHistoryKey(run: RunTrace): string | null {
  // Derive the run-history key from the source. For Phase 2 the
  // server tells us the key implicitly via the on-disk directory
  // path — but our list endpoint only returns the trace contents.
  // We embed the key in the URL via the source path; if unknown,
  // skip the link.
  const src = (run as unknown as { source?: { kind?: string; path?: string } }).source;
  if (!src) return null;
  // For local runs, the key is `<folder_name>-<8hex>`. We can't
  // reconstruct it without the hash, so we link via /runs/?run_id=…
  // pattern when needed. For Phase 2 keep it simple: don't link.
  return null;
}

export default function Runs({ loaderData }: { loaderData: LoaderData }) {
  const { runs, filter } = loaderData;
  const [searchParams, setSearchParams] = useSearchParams();
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
          No runs recorded yet. Start one with{" "}
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
              <tr key={r.run_id}>
                <td title={r.started_at}>{formatRelative(r.started_at)}</td>
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
        Direct deep-link by run key:{" "}
        <code>/runs/&lt;dir&gt;/&lt;timestamp&gt;</code> — use{" "}
        <code>cori runs show &lt;run_id&gt;</code> to find the path.
      </p>
    </>
  );
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  return "muted";
}

import { listWorkers, type WorkersResponse } from "../lib/api";

export function meta() {
  return [{ title: "Workers — Cori" }];
}

export async function clientLoader(): Promise<WorkersResponse> {
  return listWorkers();
}

export default function Workers({ loaderData }: { loaderData: WorkersResponse }) {
  const { workers, this_queue } = loaderData;
  return (
    <>
      <p className="hint" style={{ marginTop: 0 }}>
        Cluster reports from <code>~/.cori/cluster/</code>. The Console's own
        machine is <code>{this_queue}</code>.
      </p>

      {workers.length === 0 ? (
        <div className="empty">
          No workers visible on the cluster. Start one with{" "}
          <code>cori work</code> or <code>cori work --shared &lt;pool&gt;</code>.
        </div>
      ) : (
        <div>
          {workers.map((w) => (
            <div className="card" key={w.task_queue}>
              <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
                <h3 style={{ margin: 0 }}>
                  {workerLabel(w.identity)}{" "}
                  <span className={`pill ${w.kind === "shared" ? "warn" : "muted"}`}>
                    {w.kind}
                  </span>
                  {w.is_self && (
                    <span className="pill ok" style={{ marginLeft: 4 }}>
                      this machine
                    </span>
                  )}
                </h3>
                <code style={{ color: "var(--muted)", fontSize: 12 }}>{w.task_queue}</code>
              </div>
              <h4
                style={{
                  margin: "12px 0 6px",
                  fontSize: 11,
                  textTransform: "uppercase",
                  color: "var(--muted)",
                  letterSpacing: "0.06em",
                }}
              >
                Capabilities
              </h4>
              {w.capabilities.length === 0 ? (
                <div className="hint">(none reported)</div>
              ) : (
                <div className="cap-grid">
                  {w.capabilities.map((c) => (
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
            </div>
          ))}
        </div>
      )}
    </>
  );
}

function workerLabel(id: WorkersResponse["workers"][number]["identity"]): string {
  if (id.kind === "person") return id.user_id;
  if (id.kind === "service") return `service:${id.pool}`;
  return "unknown";
}

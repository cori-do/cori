import { Link } from "react-router";
import {
  apiGet,
  type ActivityTrace,
  type RunTrace,
} from "../lib/api";
import {
  formatAbsolute,
  formatCost,
  formatDuration,
  formatRelative,
} from "../lib/format";

interface LoaderArgs {
  params: { key?: string; utc?: string };
}

export async function clientLoader({ params }: LoaderArgs): Promise<RunTrace> {
  const key = params.key;
  const utc = params.utc;
  if (!key || !utc) {
    throw new Response("missing run path", { status: 400 });
  }
  const filename = utc.endsWith(".json") ? utc : `${utc}.json`;
  return apiGet<RunTrace>(
    `/api/runs/${encodeURIComponent(key)}/${encodeURIComponent(filename)}`,
  );
}

export default function RunDetail({ loaderData }: { loaderData: RunTrace }) {
  const t = loaderData;
  return (
    <>
      <p className="hint">
        <Link to="/runs">← all runs</Link>
      </p>
      <h1>
        {t.workflow_id}{" "}
        <span className={`pill ${pillFor(t.status)}`}>{t.status}</span>
      </h1>

      <div className="card">
        <dl className="kv">
          <dt>Run id</dt>
          <dd>{t.run_id}</dd>
          <dt>Trigger</dt>
          <dd>{t.trigger}</dd>
          {t.workflow_content_hash && (
            <>
              <dt>Content</dt>
              <dd>{t.workflow_content_hash.slice(0, 12)}</dd>
            </>
          )}
          <dt>Started</dt>
          <dd>
            {formatAbsolute(t.started_at)} ({formatRelative(t.started_at)})
          </dd>
          <dt>Duration</dt>
          <dd>{formatDuration(t.duration_ms)}</dd>
          {t.cost && t.cost.total_eur > 0 && (
            <>
              <dt>Cost</dt>
              <dd>
                {formatCost(t.cost.total_eur)} ({t.cost.input_tokens} in /{" "}
                {t.cost.output_tokens} out)
              </dd>
            </>
          )}
          {t.error && (
            <>
              <dt>Error</dt>
              <dd style={{ color: "var(--red)" }}>{t.error}</dd>
            </>
          )}
        </dl>
      </div>

      <h2>Steps</h2>
      {t.activities.length === 0 ? (
        <div className="empty">No activities recorded.</div>
      ) : (
        <div className="timeline">
          {t.activities.map((a, i) => (
            <StepRow key={a.activity_id} index={i + 1} a={a} />
          ))}
        </div>
      )}
    </>
  );
}

function StepRow({ index, a }: { index: number; a: ActivityTrace }) {
  const failed = a.status === "failed";
  return (
    <div className={`step ${failed ? "failed" : ""}`}>
      <div className="num">{index}.</div>
      <div>
        <div className="name">{a.step_name}</div>
        <div className="meta">
          {a.kind}
          {" · "}
          {a.attempts > 1 ? `${a.attempts} attempts` : "1 attempt"}
          {a.task_queue ? ` · ${a.task_queue}` : ""}
        </div>
        {a.error && (
          <div className="meta" style={{ color: "var(--red)" }}>
            {a.error}
          </div>
        )}
        <details>
          <summary>output</summary>
          <pre>{JSON.stringify(a.output, null, 2)}</pre>
        </details>
      </div>
      <div className="right">
        <div>
          <span className={`pill ${pillFor(a.status)}`}>{a.status}</span>
        </div>
        <div>{formatDuration(a.duration_ms)}</div>
        {a.cost_eur != null && a.cost_eur > 0 && (
          <div>{formatCost(a.cost_eur)}</div>
        )}
      </div>
    </div>
  );
}

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  if (status === "skipped") return "muted";
  return "muted";
}

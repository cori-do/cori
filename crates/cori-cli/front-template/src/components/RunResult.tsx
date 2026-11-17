import type { RunView } from "@cori-do/client";
import { CheckCircle2, Clock, Hourglass, XCircle } from "lucide-react";
import { formatValue } from "../lib/api";
import { Badge, ErrorBanner, cx } from "./ui";

/** Renders any run: running (its steps so far, or the decision it waits for), failed (message and failing step) or
 *  succeeded (headline, fields, sections). */
export function RunResult({ run, showSteps = false, onRetry }: { run: RunView<any, any>; showSteps?: boolean; onRetry?: () => void }) {
  if (run.status === "running") {
    return (
      <div className="space-y-3">
        <div className="flex items-center gap-2 text-sm text-zinc-600 dark:text-zinc-300">
          {run.pendingApproval
            ? <><Hourglass className="size-4" aria-hidden /> Waiting for a decision ({run.pendingApproval.to === "user:initiator" ? "yours" : run.pendingApproval.to})…</>
            : <><Clock className="size-4 animate-pulse" aria-hidden /> Running…</>}
        </div>
        {run.steps && <Steps run={run} />}
      </div>
    );
  }
  if (run.status !== "succeeded") {
    const step = run.errorDetail?.step;
    return (
      <div className="space-y-3">
        <ErrorBanner title={step ? `Failed at ${step}` : "The run failed"} message={run.errorDetail?.message ?? run.error ?? "Unknown error"} onRetry={onRetry} />
        {showSteps && run.steps && <Steps run={run} />}
      </div>
    );
  }
  const r = run.result;
  return (
    <div className="space-y-4">
      {r?.headline && <p className="text-base font-semibold">{r.headline}</p>}
      {!!r?.fields?.length && (
        <dl className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-2">
          {r.fields.map((f) => (
            <div key={f.label}>
              <dt className="text-xs font-medium tracking-wide text-zinc-500 uppercase dark:text-zinc-400">{f.label}</dt>
              <dd className="mt-0.5 text-sm tabular-nums">{formatValue(f.value)}</dd>
            </div>
          ))}
        </dl>
      )}
      {r?.sections?.map((s) => (
        <div key={s.label}>
          <h4 className="text-xs font-medium tracking-wide text-zinc-500 uppercase dark:text-zinc-400">{s.label}</h4>
          <p className="mt-1 text-sm whitespace-pre-wrap">{formatValue(s.value)}</p>
        </div>
      ))}
      {showSteps && run.steps && <Steps run={run} />}
    </div>
  );
}

function Steps({ run }: { run: RunView<any, any> }) {
  return (
    <ol className="flex flex-wrap gap-1.5">
      {run.steps!.map((s) => (
        <li key={s.name}>
          <Badge tone={s.status === "ok" ? "green" : s.status === "failed" ? "red" : "neutral"} className={cx(s.status === "not_taken" && "opacity-60")}>
            {s.status === "ok" ? <CheckCircle2 className="size-3" aria-hidden /> : s.status === "failed" ? <XCircle className="size-3" aria-hidden /> : null}
            {s.name}
          </Badge>
        </li>
      ))}
    </ol>
  );
}

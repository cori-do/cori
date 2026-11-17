import { useMemo, useState } from "react";
import { Activity, CornerDownRight, Search } from "lucide-react";
import { api } from "../../lib/api";
import { ago, between, duration, words } from "../../lib/format";
import { useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import type { RunSummary } from "../../lib/types";
import { Card, cx, Empty, ErrorNote, Input, RunStatus, Segmented, Select, SkeletonRows, Time } from "../../ui/core";
import { PersonName } from "../../ui/people";

// The runs of an app: every run for its builders, a person's own runs otherwise (the server filters). A child run sits
// under the run that started it.

type Filter = "all" | "running" | "failed" | "succeeded";

export function Runs({ app, builds }: { app: string; builds: boolean }) {
  const q = useQuery<RunSummary[]>(`/v1/apps/${app}/runs`, () => api<RunSummary[]>(`/v1/apps/${app}/runs`), { every: 10_000 });
  const [status, setStatus] = useState<Filter>("all");
  const [wf, setWf] = useState("");
  const [text, setText] = useState("");
  const runs = q.data ?? [];
  const count = (s: Filter) => runs.filter((r) => !r.parent_run && (s === "all" || r.status === s)).length;
  const workflows = useMemo(() => [...new Set(runs.map((r) => r.workflow))].sort(), [runs]);
  const children = useMemo(() => {
    const m: Record<string, RunSummary[]> = {};
    runs.forEach((r) => { if (r.parent_run) (m[r.parent_run] ||= []).push(r); });
    return m;
  }, [runs]);
  const shown = runs.filter((r) => !r.parent_run && (status === "all" || r.status === status) && (!wf || r.workflow === wf) && (!text || r.run_id.includes(text.trim())));

  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={5} />;
  if (!runs.length) return (
    <Empty icon={<Activity />} title={builds ? "No run yet" : "You haven't run anything yet"}>
      {builds ? "Runs appear here as people use the app, its schedules fire and its webhooks are called." : "What you start in the app, or from its Overview, shows here with its progress and result."}
    </Empty>
  );

  return (
    <>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <Segmented label="Status" value={status} onChange={setStatus} options={[
          { value: "all", label: "All", count: count("all") },
          { value: "running", label: "Running", count: count("running") },
          { value: "failed", label: "Failed", count: count("failed") },
          { value: "succeeded", label: "Succeeded", count: count("succeeded") },
        ]} />
        {workflows.length > 1 && (
          <Select h="sm" className="w-auto" value={wf} onChange={(e) => setWf(e.target.value)} aria-label="Workflow">
            <option value="">Every workflow</option>
            {workflows.map((w) => <option key={w} value={w}>{words(w)}</option>)}
          </Select>
        )}
        <div className="relative ml-auto w-full sm:w-56">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
          <Input h="sm" className="pl-9" placeholder="Find a run by its id" aria-label="Find a run by its id" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
      </div>
      <Card className="overflow-x-auto">
        <table className="w-full min-w-[560px] text-left text-[14px]">
          <thead>
            <tr className="border-b border-line text-[13px] text-fg-3">
              <th className="py-2.5 pr-3 pl-4 font-medium">Status</th>
              <th className="px-3 py-2.5 font-medium">Workflow</th>
              {builds && <th className="px-3 py-2.5 font-medium">Started by</th>}
              <th className="px-3 py-2.5 font-medium">Started</th>
              <th className="py-2.5 pr-4 pl-3 text-right font-medium">Duration</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {shown.flatMap((r) => [r, ...(children[r.run_id] || [])]).map((r) => <Row key={r.run_id} app={app} r={r} builds={builds} child={!!r.parent_run} />)}
          </tbody>
        </table>
        {!shown.length && <p className="px-4 py-8 text-center text-fg-3">No run matches.</p>}
      </Card>
      <p className="mt-2 text-[13px] text-fg-4">The last 100 runs. This list refreshes itself.</p>
    </>
  );
}

function Row({ app, r, builds, child }: { app: string; r: RunSummary; builds: boolean; child: boolean }) {
  const ms = between(r.started_at, r.ended_at ?? (r.status === "running" ? undefined : r.started_at));
  return (
    <tr className="group relative transition-colors hover:bg-surface-2/60">
      <td className={cx("py-2.5 pr-3 pl-4", child && "pl-9")}>
        <span className="flex items-center gap-2">{child && <CornerDownRight className="size-3.5 text-fg-4" aria-hidden />}<RunStatus status={r.status} /></span>
      </td>
      <td className="px-3 py-2.5">
        <a href={href("app", app, "runs", r.run_id)} className="font-medium text-fg after:absolute after:inset-0">{words(r.workflow)}</a>
      </td>
      {builds && <td className="px-3 py-2.5 text-fg-2"><PersonName id={r.user} /></td>}
      <td className="px-3 py-2.5 whitespace-nowrap text-fg-3"><Time at={r.started_at}>{ago(r.started_at)}</Time></td>
      <td className="py-2.5 pr-4 pl-3 text-right whitespace-nowrap text-fg-3 tabular-nums">{r.status === "running" ? "…" : duration(ms)}</td>
    </tr>
  );
}

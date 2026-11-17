import { ArrowLeft, Copy, Loader2 } from "lucide-react";
import { api } from "../../lib/api";
import { decisionHref, useDecisions } from "../../lib/decisions";
import { between, duration, when, words } from "../../lib/format";
import { peek, useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import { stepRows } from "../../lib/steps";
import { useTitle } from "../../lib/title";
import type { RunDetail as Detail, RunEvent, RunSteps, Step } from "../../lib/types";
import { Badge, ButtonLink, Callout, Card, copy, ErrorNote, Facts, IconButton, runTone, Section, Skeleton, StatusText, Time } from "../../ui/core";
import { FileValue, isFileValue, Payload } from "../../ui/value";
import { StepList } from "./RunSteps";

// One run: how it went, step by step (each step opens on what it received and gave back), what it was given and what it
// gave back. A running run refreshes itself.

export function RunDetail({ app, appName, id, builds }: { app: string; appName: string; id: string; builds: boolean }) {
  const key = `/v1/apps/${app}/runs/${id}`;
  // asked again every two seconds while it runs
  const first = useQuery<Detail>(key, () => api<Detail>(key), { every: peek<Detail>(key)?.status === "running" ? 2000 : undefined });
  const running = first.data?.status === "running";
  const events = useQuery<RunEvent[]>(`${key}/events`, () => api<RunEvent[]>(`${key}/events`).catch(() => []), { every: running ? 2000 : undefined });
  // the run's workflow as its release defines it, and each step's data (none from a server without it)
  const steps = useQuery<RunSteps | null>(`${key}/steps`, () => api<RunSteps>(`${key}/steps`).catch(() => null), { every: running ? 2000 : undefined });
  const waiting = useDecisions(app).items.filter((d) => d.approval?.run_id === id);
  const r = first.data;
  const wf = steps.data?.workflow;
  const name = wf?.name || (r ? words(r.workflow) : "");
  useTitle(r ? name : "Run", appName);

  const back = <a href={href("app", app, "runs")} className="mb-4 inline-flex items-center gap-1.5 text-[13px] text-fg-3 hover:text-fg"><ArrowLeft className="size-4" />{builds ? "Runs" : "My runs"}</a>;
  if (first.error && !r) return <>{back}<ErrorNote retry={first.reload}>{first.error}</ErrorNote></>;
  if (!r) return <>{back}<Skeleton className="h-7 w-64" /><Skeleton className="mt-6 h-40 w-full" /></>;

  const trace: Step[] = r.steps || (events.data || []).filter((e) => e.kind === "step").map((e) => e.payload);
  const rows = stepRows({ plan: steps.data, trace, outputs: r.outputs, running });
  const failed = rows.flatMap(function all(x): typeof rows { return [x, ...x.children.flatMap(all)]; }).find((x) => x.state === "failed")?.name;
  const asked = new URLSearchParams(location.hash.split("?")[1] || "").get("step") || undefined;
  const t = runTone(r.status);
  const ms = between(r.started_at, r.ended_at || undefined);
  return (
    <>
      {back}
      <div className="mb-6 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-3">
            <h2 className="text-xl font-semibold tracking-tight">{name}</h2>
            <Badge tone={t.tone} dot>{t.label}</Badge>
          </div>
          {wf?.description && <p className="mt-1 max-w-2xl text-[14px] text-fg-3">{wf.description}</p>}
          <div className="mt-1 flex items-center gap-1 text-[13px] text-fg-3">
            <code className="truncate">{id}</code>
            <IconButton label="Copy the run id" size="xs" onClick={() => copy(id, "Run id copied")}><Copy /></IconButton>
          </div>
        </div>
      </div>

      {waiting.map((d) => (
        <Callout key={d.key} tone="warning" className="mb-4" title="Waiting for your decision" action={<ButtonLink href={decisionHref(d)} variant="primary" size="xs">Decide</ButtonLink>}>
          {d.title}
        </Callout>
      ))}
      {!waiting.length && running && <Callout className="mb-4">This run is under way. This page follows it as it goes.</Callout>}

      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_300px]">
        <div className="min-w-0 space-y-5">
          {r.result && (
            <Card className="p-5">
              <div className="text-[13px] text-fg-3">Result</div>
              <div className="mt-1 text-[17px] font-semibold text-fg">{r.result.headline}</div>
              {(r.result.fields || []).length > 0 && (
                <dl className="mt-3 grid grid-cols-[minmax(7rem,auto)_1fr] gap-x-6 gap-y-1.5 text-[14px]">
                  {r.result.fields!.map((f, i) => <div key={i} className="contents"><dt className="text-fg-3">{f.label}</dt><dd className="min-w-0 break-words">{isFileValue(f.value) ? <FileValue value={f.value} app={app} /> : typeof f.value === "object" ? <code className="text-[12.5px]">{JSON.stringify(f.value)}</code> : String(f.value ?? "—")}</dd></div>)}
                </dl>
              )}
            </Card>
          )}
          {r.error && <Callout tone="danger" title="It failed"><span className="font-mono text-[12.5px] break-words whitespace-pre-wrap">{r.error}</span></Callout>}

          <Section title="Steps" description={rows.length ? "Open a step to see what it does, what it received and what it gave back." : undefined}>
            {rows.length ? (
              <Card className="p-2">
                <StepList rows={rows} app={app} builds={builds} running={running} open={asked || failed} />
                {running && !steps.data?.steps.length && <div className="flex items-center gap-3 px-3 py-2.5 text-[13px] text-fg-3"><Loader2 className="size-4 animate-spin" aria-hidden />Next step…</div>}
              </Card>
            ) : <p className="text-fg-3">{running ? "Starting…" : "No step recorded."}</p>}
          </Section>

          <Section title="Parameters"><Card className="p-5"><Payload value={r.params} empty="It took no parameter." app={app} /></Card></Section>
        </div>

        <Card className="h-fit p-5">
          <Facts items={[
            ["Status", <StatusText tone={t.tone} pulse={t.pulse}>{t.label}</StatusText>],
            ["App", appName],
            ...(wf?.release ? [["Release", wf.release] as [string, string]] : []),
            ["Started", <Time at={r.started_at}>{when(r.started_at)}</Time>],
            ["Duration", running ? "Running" : duration(ms) || "—"],
            ["Steps", String(trace.filter((s) => s.status !== "not_taken").length)],
          ]} />
        </Card>
      </div>
    </>
  );
}

import type { ReactNode } from "react";
import { ArrowRight, Clock, ExternalLink, Globe, Play, Rocket, Webhook } from "lucide-react";
import { api } from "../../lib/api";
import { decisionHref, useDecisions } from "../../lib/decisions";
import { ago, cronWords, eur, plural, words } from "../../lib/format";
import { personName } from "../../lib/people";
import { useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import { useMyApp } from "../../lib/session";
import type { AppDetail, Meters, PromotedWorkflow, RunSummary, Version } from "../../lib/types";
import { Badge, Button, ButtonLink, Callout, Card, CopyField, cx, Empty, List, RunStatus, Section, Skeleton, Stat, Time } from "../../ui/core";
import { PersonName } from "../../ui/people";
import { goLive, goLiveLabel } from "./releases-lib";
import { runWorkflow } from "./RunWorkflow";

// An app's overview. Its builders see what to do next (deploy, try, go live), how it runs and what it costs; the people
// who use it see how to open it, what they can start and their own runs.

export function Overview({ app, d }: { app: string; d: AppDetail }) {
  return d.build === false ? <UseOverview app={app} d={d} /> : <BuildOverview app={app} d={d} />;
}

const workflowsOf = (d: AppDetail): PromotedWorkflow[] => (Array.isArray(d.promoted_workflows) ? d.promoted_workflows : []);

function BuildOverview({ app, d }: { app: string; d: AppDetail }) {
  const versions = useQuery<Version[]>(`/v1/apps/${app}/versions`, () => api<Version[]>(`/v1/apps/${app}/versions`));
  const meters = useQuery<Meters>(`/v1/apps/${app}/meters`, () => api<Meters>(`/v1/apps/${app}/meters`));
  const runs = useQuery<RunSummary[]>(`/v1/apps/${app}/runs`, () => api<RunSummary[]>(`/v1/apps/${app}/runs`));
  const decisions = useDecisions(app).items;
  const since = Date.now() - 86_400_000;
  const today = (runs.data || []).filter((r) => r.started_at && Date.parse(r.started_at) > since);
  const failed = today.filter((r) => r.status === "failed").length;
  const month = meters.data?.llm.month;

  return (
    <>
      <NextStep app={app} d={d} versions={versions.data} />

      <div className="mt-6 grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Stat label="Runs, last 24 h" value={runs.data ? today.length : "–"} hint={failed ? `${failed} failed` : runs.data ? "none failed" : undefined} tone={failed ? "danger" : undefined} href={href("app", app, "runs")} />
        <Stat label="Waiting for you" value={decisions.length} hint={decisions.length ? "decisions to make" : "nothing to decide"} tone={decisions.length ? "warning" : undefined} href={href("inbox", app)} />
        <Stat label="AI spend this month" value={month ? eur(month.spent_eur) : "–"} tone={month?.blocked ? "danger" : undefined}
          hint={month ? (month.blocked ? "budget spent: AI steps fail" : month.budget_eur != null ? `of ${eur(month.budget_eur)}` : "no budget set") : undefined} />
        <Stat label="Releases" value={versions.data ? versions.data.length : "–"} hint={d.promoted_release ? `${d.promoted_release} live` : "none live"} href={href("app", app, "releases")} />
      </div>

      <Workflows app={app} d={d} canRunAll />

      <Section title="Recent runs" actions={<a href={href("app", app, "runs")} className="link inline-flex items-center gap-1 text-[13px] font-medium">All runs<ArrowRight className="size-3.5" /></a>}>
        <RecentRuns app={app} runs={runs.data} showUser />
      </Section>
    </>
  );
}

/** The one thing to do next with the app, as its builders see it. */
function NextStep({ app, d, versions }: { app: string; d: AppDetail; versions?: Version[] }) {
  if (!versions) return <Card className="p-5"><Skeleton className="h-5 w-56" /><Skeleton className="mt-3 h-4 w-80" /></Card>;
  const live = versions.find((v) => v.section === "live");
  const next = versions.find((v) => v.section === "newer");
  const frame = (tone: "brand" | "success" | "neutral", icon: ReactNode, title: ReactNode, body: ReactNode, actions?: ReactNode) => (
    <Card className="flex flex-col gap-4 p-5 sm:flex-row sm:items-center">
      <span className={cx("flex size-10 shrink-0 items-center justify-center rounded-xl [&_svg]:size-5",
        tone === "brand" ? "bg-brand-50 text-brand-700 dark:bg-brand-500/15 dark:text-brand-100" : tone === "success" ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-500/10 dark:text-emerald-300" : "bg-surface-2 text-fg-3")}>{icon}</span>
      <div className="min-w-0 flex-1">
        <div className="font-semibold text-fg">{title}</div>
        <div className="mt-0.5 text-[13.5px] text-fg-3">{body}</div>
      </div>
      {actions && <div className="flex shrink-0 flex-wrap gap-2">{actions}</div>}
    </Card>
  );
  if (!versions.length) return frame("neutral", <Rocket />, "Deploy a first release",
    <>Ask your agent to deploy it, or run this in the app's folder. You try it on its own preview data before anyone uses it.</>,
    <CopyField value="cori deploy" className="w-full sm:w-48" />);
  if (next) {
    const label = goLiveLabel(next);
    const title = next.go_live === "pending" ? `${next.release} waits for an approver`
      : next.go_live === "request" ? `${next.release} is ready: an approver makes it live`
      : `${next.release} is ready to go live`;
    return frame("brand", <Rocket />, title,
      <>{next.message ? `“${next.message}” · ` : ""}deployed by {personName(next.created_by)} <Time at={next.created_at}>{ago(next.created_at)}</Time>
        {next.go_live === "pending" && next.gate.by ? <> · requested by {personName(next.gate.by)}</> : null}</>,
      <>
        <ButtonLink href={next.preview.url} external icon={<ExternalLink />}>Try the preview</ButtonLink>
        {label ? <Button variant="primary" onClick={() => goLive(app, next, live, d.front_url)}>{label}</Button>
          : <ButtonLink href={href("app", app, "releases", next.release)}>Details</ButtonLink>}
      </>);
  }
  if (live) return frame("success", <Globe />, `${live.release} is live`,
    <>Since <Time at={live.live_periods.at(-1)?.from}>{ago(live.live_periods.at(-1)?.from)}</Time>. Deploy a new release whenever you're ready: it gets its own preview first.</>,
    d.front_url && <ButtonLink href={d.front_url} external icon={<ExternalLink />}>Open the app</ButtonLink>);
  const older = versions[0];
  return frame("neutral", <Rocket />, "Nothing is live", <>The newest release is {older.release}.</>,
    goLiveLabel(older) && <Button variant="primary" onClick={() => goLive(app, older, undefined, d.front_url)}>{goLiveLabel(older)}</Button>);
}

/** How a workflow starts, in words. */
function triggers(w: PromotedWorkflow): { icon: ReactNode; text: string }[] {
  const out: { icon: ReactNode; text: string }[] = [];
  if (w.endpoint) {
    const roles = w.endpoint.allow?.roles || [];
    const pub = w.endpoint.auth === "none" || w.endpoint.auth === "public";
    out.push({ icon: pub ? <Webhook /> : <Play />, text: pub ? "Webhook" : roles.length ? `On demand · ${roles.join(", ")}` : "On demand" });
  }
  const sched = w.schedule as unknown;
  if (typeof sched === "string" && sched) out.push({ icon: <Clock />, text: cronWords(sched) });
  return out;
}

function Workflows({ app, d, canRunAll }: { app: string; d: AppDetail; canRunAll?: boolean }) {
  const wfs = workflowsOf(d);
  return (
    <Section title={canRunAll ? "Workflows" : "What you can start"} description={canRunAll ? (d.promoted ? `As the live release ${d.promoted_release || ""} defines them.` : undefined) : undefined}>
      {wfs.length ? (
        <List>
          {wfs.map((w) => (
            <div key={w.id} className="flex flex-wrap items-center gap-x-4 gap-y-1.5 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="font-medium text-fg">{words(w.id)}</div>
                <div className="mt-0.5 flex flex-wrap gap-x-3 gap-y-1 text-[13px] text-fg-3">
                  {triggers(w).map((t, i) => <span key={i} className="inline-flex items-center gap-1.5 [&_svg]:size-3.5">{t.icon}{t.text}</span>)}
                  {!triggers(w).length && <span>Started by other workflows</span>}
                  {(w.parameters || []).length > 0 && <span>{plural(w.parameters!.length, "parameter")}</span>}
                </div>
              </div>
              {w.endpoint && <Button size="xs" icon={<Play />} onClick={() => runWorkflow(app, w)}>Run</Button>}
            </div>
          ))}
        </List>
      ) : (
        <Empty compact title={d.promoted ? (canRunAll ? "No workflow" : "Nothing to start here") : "Nothing live yet"}>
          {d.promoted ? (canRunAll ? "The live release declares no workflow." : "Your roles start no workflow directly: use the app itself.") : canRunAll ? "Workflows show here once a release is live." : "The app is not live yet."}
        </Empty>
      )}
    </Section>
  );
}

function RecentRuns({ app, runs, showUser }: { app: string; runs?: RunSummary[]; showUser?: boolean }) {
  if (!runs) return <Card className="p-4"><Skeleton className="h-4 w-1/2" /></Card>;
  const top = runs.filter((r) => !r.parent_run).slice(0, 6);
  if (!top.length) return <Empty compact title="No run yet">Runs show here as soon as the app is used.</Empty>;
  return (
    <List>
      {top.map((r) => (
        <a key={r.run_id} href={href("app", app, "runs", r.run_id)} className="flex items-center gap-4 px-4 py-2.5 transition-colors hover:bg-surface-2/60">
          <RunStatus status={r.status} className="w-28 shrink-0 text-[13px]" />
          <span className="min-w-0 flex-1 truncate font-medium text-fg">{words(r.workflow)}</span>
          {showUser && <span className="hidden truncate text-[13px] text-fg-3 sm:block"><PersonName id={r.user} /></span>}
          <span className="w-24 shrink-0 text-right text-[13px] text-fg-3"><Time at={r.started_at}>{ago(r.started_at)}</Time></span>
        </a>
      ))}
    </List>
  );
}

function UseOverview({ app, d }: { app: string; d: AppDetail }) {
  const mine = useMyApp(app);
  const runs = useQuery<RunSummary[]>(`/v1/apps/${app}/runs`, () => api<RunSummary[]>(`/v1/apps/${app}/runs`));
  const decisions = useDecisions(app).items;
  return (
    <>
      {!d.front_url && <Callout className="mb-6" title={`${d.name || app} is not live yet`}>Its builders are still working on it. It opens here once a release is live.</Callout>}

      {decisions.length > 0 && (
        <Section title="Waiting for you">
          <List>
            {decisions.map((x) => (
              <a key={x.key} href={decisionHref(x)} className="flex items-center gap-3 px-4 py-3 hover:bg-surface-2/60">
                <span className="min-w-0 flex-1 truncate font-medium text-fg">{x.title}</span>
                <Badge tone="warning">Decide</Badge>
              </a>
            ))}
          </List>
        </Section>
      )}

      <Workflows app={app} d={d} />

      {mine && mine.stores.length > 0 && (
        <Section title="Data">
          <Card className="flex flex-wrap items-center gap-3 p-4">
            <p className="min-w-0 flex-1 text-[14px] text-fg-2">See and edit the data your role reaches, as a spreadsheet: {mine.stores.join(", ")}.</p>
            <ButtonLink href={href("app", app, "data")}>Open the data</ButtonLink>
          </Card>
        </Section>
      )}

      <Section title="Your recent runs" actions={<a href={href("app", app, "runs")} className="link inline-flex items-center gap-1 text-[13px] font-medium">All your runs<ArrowRight className="size-3.5" /></a>}>
        <RecentRuns app={app} runs={runs.data} />
      </Section>
    </>
  );
}

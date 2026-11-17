import { useState } from "react";
import { ArrowRight, ExternalLink, Plus, Search } from "lucide-react";
import { decisionHref, useDecisions, useWaitingIn } from "../lib/decisions";
import { plural } from "../lib/format";
import { personOf, usePeople } from "../lib/people";
import { href } from "../lib/router";
import { useSession } from "../lib/session";
import { useTitle } from "../lib/title";
import type { AppSummary, MyApp } from "../lib/types";
import { AppMark } from "../shell/Shell";
import { Button, Card, cx, Empty, Input, List, PageHeader, Section, Skeleton, StatusText } from "../ui/core";
import { GetStarted } from "./GetStarted";
import { OwnOrganization } from "./OwnOrganization";
import { DecisionRow } from "./inbox/parts";

// Home: what waits for the person, and their apps. A builder who has not shipped yet also gets the first steps; someone
// who may not create apps here, an organization of their own to start when the identity provider offers one.

const SHOWN_DECISIONS = 4;

export function Home() {
  useTitle("Home");
  const s = useSession();
  usePeople();
  const { items } = useDecisions();
  const [q, setQ] = useState("");
  const built = new Map(s.built.map((b) => [b.id, b]));
  const apps = s.apps.filter((a) => !q || `${a.name || ""} ${a.id}`.toLowerCase().includes(q.toLowerCase()));
  const first = (personOf(s.me?.sub)?.name || s.me?.email?.replace(/@.*/, "") || "").split(/\s+/)[0];
  const hour = new Date().getHours();
  const hello = hour < 5 ? "Good evening" : hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
  const org = s.me?.organization;

  return (
    <>
      <PageHeader title={first ? `${hello}, ${first[0].toUpperCase()}${first.slice(1)}` : hello}
        description={s.loading ? undefined : items.length ? `${plural(items.length, "decision")} ${items.length === 1 ? "waits" : "wait"} for you${org?.name ? ` in ${org.name}` : ""}.` : `Nothing waits for you${org?.name ? ` in ${org.name}` : ""}.`}
        actions={s.builds && <Button variant="primary" icon={<Plus />} onClick={() => { location.hash = href("connect", "new"); }}>New app</Button>} />

      <OwnOrganization />
      {s.builds && <GetStarted />}

      {items.length > 0 && (
        <Section title="Waiting for you" actions={<a href={href("inbox")} className="link inline-flex items-center gap-1 text-[13px] font-medium">Open inbox<ArrowRight className="size-3.5" /></a>}>
          <List>
            {items.slice(0, SHOWN_DECISIONS).map((d) => <DecisionRow key={d.key} d={d} href={decisionHref(d)} />)}
            {items.length > SHOWN_DECISIONS && (
              <a href={href("inbox")} className="block px-4 py-2.5 text-[13px] font-medium text-fg-3 hover:bg-surface-2 hover:text-fg">
                {plural(items.length - SHOWN_DECISIONS, "more decision")} in your inbox
              </a>
            )}
          </List>
        </Section>
      )}

      <Section title="Your apps" description={s.me?.portal_url && s.apps.length > 0 ? <>Your colleagues open theirs from the <a href={s.me.portal_url} target="_blank" rel="noopener noreferrer" className="link">apps portal</a>.</> : undefined}
        actions={s.apps.length > 6 && (
        <div className="relative w-56">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
          <Input h="sm" className="pl-9" placeholder="Filter apps" aria-label="Filter apps" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      )}>
        {s.loading ? (
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">{[0, 1, 2].map((i) => <Card key={i} className="h-[132px] p-4"><Skeleton className="size-10 rounded-xl" /><Skeleton className="mt-4 h-3.5 w-2/3" /><Skeleton className="mt-2 h-3 w-1/2" /></Card>)}</div>
        ) : s.apps.length === 0 ? (
          <Empty title={s.builds ? "No app yet" : "No app shared with you yet"}
            action={s.builds && <Button variant="primary" icon={<Plus />} onClick={() => { location.hash = href("connect", "new"); }}>Create your first app</Button>}>
            {s.builds ? "Your coding agent builds and deploys apps here. Each one appears here with its releases, runs and data."
              : s.me?.guest ? "The apps shared with you appear here." : "When someone gives you a role in one of the organization's apps, it appears here."}
          </Empty>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {apps.map((a) => <AppCard key={a.id} a={a} b={built.get(a.id)} />)}
            {apps.length === 0 && <p className="text-fg-3">No app matches “{q}”.</p>}
          </div>
        )}
      </Section>
    </>
  );
}

function AppCard({ a, b }: { a: MyApp; b?: AppSummary }) {
  const waiting = useWaitingIn().inApp(a);
  const ready = !!b?.last_version && b.last_version.hash !== a.promoted;
  const facts = [
    a.build && b ? `${plural(b.runs_24h, "run")} today` : a.my_runs ? `${plural(a.my_runs, "run")} by you` : null,
    a.roles.length ? `You're ${a.roles.join(", ")}` : a.build ? "You build it" : null,
  ].filter(Boolean);
  return (
    <div className="group relative flex flex-col rounded-xl bg-surface p-4 shadow-xs ring-1 ring-line transition-shadow hover:shadow-md hover:ring-line-2">
      <div className="flex items-start gap-3">
        <AppMark app={a} size="lg" />
        <div className="min-w-0 flex-1">
          <a href={href("app", a.id)} className="block truncate font-semibold text-fg after:absolute after:inset-0 after:rounded-xl">{a.name || a.id}</a>
          <div className="line-clamp-2 text-[13px] text-fg-3" title={a.description || undefined}>{a.description || a.id}</div>
        </div>
        {a.front_url && (
          <a href={a.front_url} target="_blank" rel="noreferrer" className="relative z-10 inline-flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[13px] font-medium text-fg-2 ring-1 ring-line ring-inset hover:bg-surface-2 hover:text-fg">
            Open<ExternalLink className="size-3.5" aria-hidden />
          </a>
        )}
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px]">
        {a.promoted ? <StatusText tone="success">Live{a.promoted_release ? ` · ${a.promoted_release}` : ""}</StatusText> : <StatusText tone="neutral">Not live yet</StatusText>}
        {a.build && ready && <StatusText tone="brand">New release ready</StatusText>}
        {waiting > 0 && <StatusText tone="warning">{waiting} waiting for you</StatusText>}
      </div>
      {facts.length > 0 && <div className={cx("mt-1.5 truncate text-[13px] text-fg-3")}>{facts.join(" · ")}</div>}
    </div>
  );
}

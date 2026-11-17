import { ExternalLink, Link2, MoreHorizontal, Terminal } from "lucide-react";
import { api } from "../../lib/api";
import { usePeople } from "../../lib/people";
import { useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import { useMyApp, useSession } from "../../lib/session";
import { useTitle } from "../../lib/title";
import type { AppDetail } from "../../lib/types";
import { appTabs } from "../../shell/nav";
import { AppMark } from "../../shell/Shell";
import { ButtonLink, copy, Empty, ErrorNote, IconButton, Skeleton, StatusText, TabLinks } from "../../ui/core";
import { Menu } from "../../ui/menu";
import { DataTab } from "../data/DataTab";
import { Access } from "./Access";
import { AppSettings } from "./AppSettings";
import { Overview } from "./Overview";
import { Releases } from "./Releases";
import { RunDetail } from "./RunDetail";
import { Runs } from "./Runs";

// One app: its header (what it is, whether it is live, a way to open it) and its tabs. Its builders see how it ships;
// the people who use it, what they do with it.

export function AppPage({ app, rest }: { app: string; rest: string[] }) {
  const s = useSession();
  usePeople(); // ids read as names in every tab, once the people are known
  const mine = useMyApp(app);
  const q = useQuery<AppDetail>(`/v1/apps/${app}`, () => api<AppDetail>(`/v1/apps/${app}`));
  const [tab = "overview", ...sub] = rest;
  const d = q.data;
  const builds = d ? d.build !== false : !!mine?.build;
  const tabs = appTabs(app, mine ?? (d ? { id: app, build: builds, roles: d.roles || [], stores: [], waiting_for_me: 0, my_runs: 0 } : null), { admin: !!s.me?.can?.admin });
  const name = d?.name || mine?.name || app;
  const label = tabs.find((t) => t.key === tab)?.label;
  useTitle(tab === "overview" ? undefined : label, name);

  if (q.error && !d) {
    const missing = /unknown app|not found/i.test(q.error);
    return missing
      ? <Empty title="This app does not exist" action={<a href={href()} className="link">Go home</a>}>It may have been deleted, or the link is mistyped.</Empty>
      : <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  }

  return (
    <>
      <header className="mb-1 flex flex-wrap items-center gap-x-4 gap-y-3">
        <AppMark app={{ id: app, name, promoted: d?.promoted ?? mine?.promoted }} size="lg" />
        <div className="min-w-0 flex-1">
          {d || mine ? <h1 className="truncate text-[22px] leading-tight font-semibold tracking-tight">{name}</h1> : <Skeleton className="h-6 w-48" />}
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-fg-3">
            <span>{app}</span>
            {d && (d.promoted ? <StatusText tone="success">Live{d.promoted_release ? ` · ${d.promoted_release}` : ""}</StatusText> : <StatusText tone="neutral">Not live yet</StatusText>)}
            {d?.protected && <span title="every new release needs an approver">Approval required to go live</span>}
            {!builds && (d?.roles || []).length > 0 && <span>You're {(d?.roles || []).join(", ")}</span>}
          </div>
        </div>
        <div className="flex items-center gap-2">
          {d?.front_url && <ButtonLink href={d.front_url} external variant={builds ? "secondary" : "primary"} icon={<ExternalLink />}>Open app</ButtonLink>}
          <Menu items={[
            { label: "Copy link to this page", icon: <Link2 />, onSelect: () => copy(location.href, "Link copied") },
            d?.front_url && { label: "Copy the app's address", icon: <ExternalLink />, onSelect: () => copy(d.front_url!, "Address copied") },
            builds && { label: "Copy pull command", icon: <Terminal />, onSelect: () => copy(`cori pull ${app} ./${app}`, "Pull command copied") },
          ]}>
            {(p) => <IconButton {...p} label="More"><MoreHorizontal /></IconButton>}
          </Menu>
        </div>
      </header>

      <div className="mb-6 border-b border-line">
        <TabLinks label={`${name} sections`} active={tab === "runs" && sub.length ? "runs" : tab} tabs={tabs} />
      </div>

      {/* the tab's content in a box of its own: its first section sits right under the tabs */}
      {!d ? <div className="space-y-3"><Skeleton className="h-24 w-full" /><Skeleton className="h-40 w-full" /></div> : <div>{
        tab === "overview" ? <Overview app={app} d={d} />
        : tab === "releases" ? <Releases app={app} d={d} open={sub[0]} />
        : tab === "runs" ? (sub[0] ? <RunDetail app={app} appName={name} id={sub.join("/")} builds={builds} /> : <Runs app={app} builds={builds} />)
        : tab === "data" ? <DataTab app={app} d={d} parts={sub} builder={builds} />
        : tab === "access" ? <Access app={app} d={d} />
        : tab === "settings" ? <AppSettings app={app} d={d} />
        : <Empty title="This page does not exist" action={<a href={href("app", app)} className="link">Back to {name}</a>} />
      }</div>}
    </>
  );
}

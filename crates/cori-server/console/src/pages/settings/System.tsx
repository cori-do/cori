import { useState } from "react";
import { CheckCircle2, XCircle } from "lucide-react";
import { api, errMsg } from "../../lib/api";
import { ago, plural, short, tseg } from "../../lib/format";
import { useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import { useSession } from "../../lib/session";
import type { Doctor, IdentityCatalog, IdentityEvent, Pool, StoreInstance } from "../../lib/types";
import { Badge, Card, cx, Empty, ErrorNote, List, Section, Select, SkeletonRows, Time } from "../../ui/core";
import { ClientState, EventsList } from "../../ui/identity";

// How the platform is doing, for operators: its checks, the sync with the identity provider, the apps' databases and the
// workers. Mostly read-only: the deployment's own configuration changes these.

export function System() {
  return (
    <>
      <Health />
      <IdentitySync />
      <Databases />
      <Workers />
    </>
  );
}

function Health() {
  const q = useQuery<Doctor | { error: string }>("/v1/doctor", () => api<Doctor>("/v1/doctor").catch((e) => ({ error: errMsg(e) })));
  if (!q.data) return <SkeletonRows rows={3} />;
  if ("error" in q.data) return <ErrorNote>{q.data.error}</ErrorNote>;
  const bad = q.data.checks.filter((c) => !c.ok);
  return (
    <Section title="Health">
      <Card className={cx("mb-3 flex items-center gap-3 p-4", bad.length ? "ring-rose-300 dark:ring-rose-500/30" : "")}>
        {bad.length ? <XCircle className="size-5 text-rose-500" /> : <CheckCircle2 className="size-5 text-emerald-500" />}
        <span className="font-medium">{bad.length ? `${plural(bad.length, "check")} failing` : "Everything works"}</span>
        <span className="text-[13px] text-fg-3">{plural(q.data.checks.length, "check")}</span>
      </Card>
      <List>
        {[...bad, ...q.data.checks.filter((c) => c.ok)].map((c) => (
          <div key={c.name} className="flex items-start gap-3 px-4 py-2.5">
            {c.ok ? <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-500" aria-label="ok" /> : <XCircle className="mt-0.5 size-4 shrink-0 text-rose-500" aria-label="failing" />}
            <span className="w-44 shrink-0 font-medium">{c.name}</span>
            <span className="min-w-0 flex-1 text-[13.5px] break-words text-fg-3">{c.detail}</span>
          </div>
        ))}
      </List>
    </Section>
  );
}

/** What the client hook told the identity provider: apps waiting on it, and its events (an admin sends one again). */
function IdentitySync() {
  const { me } = useSession();
  const [status, setStatus] = useState("");
  const cat = useQuery<IdentityCatalog | null>("/v1/identity/catalog", () => api<IdentityCatalog>("/v1/identity/catalog").catch(() => null));
  const events = useQuery<IdentityEvent[]>(cat.data?.hook ? `/v1/identity/events?limit=30${status ? `&status=${status}` : ""}` : null,
    () => api<IdentityEvent[]>(`/v1/identity/events?limit=30${status ? `&status=${status}` : ""}`).catch(() => []));
  if (!cat.data?.hook) return null;
  const waiting = cat.data.apps.filter((a) => a.client_status !== "ready" || a.roles.some((r) => r.state === "pending"));
  return (
    <Section title="Sign-in sync" description={`Each app gets its sign-in client from your identity provider${cat.data.role_events ? ", which also hears of every role an app declares" : ""}.`}
      actions={<Select h="xs" className="w-auto" value={status} onChange={(e) => setStatus(e.target.value)} aria-label="Events to show">
        <option value="">Every event</option><option value="queued">Waiting</option><option value="dead">Given up</option><option value="done">Delivered</option>
      </Select>}>
      {waiting.length > 0 && (
        <List className="mb-3">
          {waiting.map((a) => (
            <div key={a.app} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3">
              <a href={href("app", a.app, "settings")} className="w-40 truncate font-medium hover:underline">{a.app}</a>
              <ClientState status={a.client_status} clientId={a.client_id} note={a.client_note} />
              {a.roles.some((r) => r.state === "pending") && <span className="text-[13px] text-fg-3">Roles not confirmed: {a.roles.filter((r) => r.state === "pending").map((r) => r.role).join(", ")}</span>}
            </div>
          ))}
        </List>
      )}
      {(events.data || []).length ? <EventsList events={events.data || []} showApp retry={!!me?.can?.admin} reload={events.reload} />
        : <Empty compact title={status ? "No such event" : "No event yet"}>Creating an app sends the first.</Empty>}
      <p className="mt-2 text-[13px] text-fg-4">{plural(cat.data.apps.length, "app")}. An event the identity provider does not take is sent again (after 10 s, doubling, at most hourly), then given up.</p>
    </Section>
  );
}

function Databases() {
  const q = useQuery<StoreInstance[]>("/v1/stores", () => api<StoreInstance[]>("/v1/stores").catch(() => []));
  const [all, setAll] = useState(false);
  if (!q.data) return null;
  const rows = all ? q.data : q.data.filter((s) => s.target === "live");
  return (
    <Section title="Databases" description="Each app's data: one database per store, live and per preview."
      actions={q.data.some((s) => s.target !== "live") && <button type="button" className="cursor-pointer text-[13px] text-fg-3 hover:text-fg" onClick={() => setAll(!all)}>{all ? "Live only" : "Include previews"}</button>}>
      {rows.length ? (
        <List>
          {rows.map((s) => (
            <div key={`${s.app}/${s.target}/${s.name}`} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5">
              <a href={s.target === "local" ? href("app", s.app) : href("app", s.app, "data", tseg(s.target), s.name)} className="w-56 truncate font-medium hover:underline">{s.app} · {s.name}</a>
              {s.target === "live" ? <Badge tone="success" dot>Live</Badge> : s.target === "local" ? <Badge>Local</Badge> : <Badge tone="brand">Preview {short(s.target).slice(0, 7)}</Badge>}
              <code className="min-w-0 flex-1 truncate text-[12.5px] text-fg-3">{s.database}</code>
              <span className="text-[13px] text-fg-4"><Time at={s.updated_at}>{ago(s.updated_at)}</Time></span>
            </div>
          ))}
        </List>
      ) : <Empty compact title="No database yet">A deploy creates the preview data of each store; going live creates the live one.</Empty>}
    </Section>
  );
}

function Workers() {
  const q = useQuery<Pool[]>("/v1/pools", () => api<Pool[]>("/v1/pools").catch(() => []));
  if (!q.data) return null;
  return (
    <Section title="Worker pools" description="Shared workers run every app. A named pool runs the steps that must run inside your network.">
      {q.data.length ? (
        <List>
          {q.data.map((p) => {
            const fresh = p.last_seen && Date.now() - Date.parse(p.last_seen) < 120_000;
            return (
              <div key={p.name} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5">
                <span className="w-40 truncate font-medium">{p.name}</span>
                <Badge tone={fresh ? "success" : "warning"} dot>{fresh ? "Online" : p.last_seen ? `Last seen ${ago(p.last_seen)}` : "Never seen"}</Badge>
                <code className="min-w-0 flex-1 truncate text-[12px] text-fg-3">{JSON.stringify(p.capabilities || {})}</code>
              </div>
            );
          })}
        </List>
      ) : <Empty compact title="Shared workers only">For steps that must run inside your network: <code>cori pools create &lt;name&gt;</code>.</Empty>}
    </Section>
  );
}

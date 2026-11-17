import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, CheckCheck, ExternalLink, Inbox as InboxIcon } from "lucide-react";
import { errMsg } from "../lib/api";
import { decide, decisionHref, useDecisions, type Decision } from "../lib/decisions";
import { ago, day, plural, words } from "../lib/format";
import { personName, usePeople } from "../lib/people";
import { href, navigate } from "../lib/router";
import { useSession } from "../lib/session";
import { useTitle } from "../lib/title";
import { Button, Callout, Card, cx, Empty, ErrorNote, Facts, Select, SkeletonRows, Textarea, Time } from "../ui/core";
import { confirm } from "../ui/dialog";
import { PersonName } from "../ui/people";
import { toast } from "../ui/toast";
import { Payload } from "../ui/value";
import { DecisionRow, KIND, KindIcon } from "./inbox/parts";

// The inbox: every decision waiting for the person, across their apps, decided in place with what it is about in view.
// `#/inbox/<app>` keeps one app's (where Slack and email links land); `#/inbox/<app>/<item>` opens one.

const wideScreen = () => typeof matchMedia === "function" && matchMedia("(min-width: 1024px)").matches;

export function InboxPage({ app, item }: { app?: string; item?: string }) {
  const s = useSession();
  usePeople();
  // the list's filter: the app of `#/inbox/<app>`, kept while an item of another app is open
  const [filter, setFilter] = useState(item ? "" : app || "");
  useEffect(() => { if (!item) setFilter(app || ""); }, [app, item]);
  const { items, loading, error, reload } = useDecisions();
  const [decided, setDecided] = useState<Set<string>>(new Set());
  const list = useMemo(() => items.filter((d) => !decided.has(d.key) && (!filter || d.app === filter)), [items, decided, filter]);
  const current = item ? items.find((d) => d.app === app && d.item === item && !decided.has(d.key)) : undefined;
  useTitle(list.length ? `Inbox (${list.length})` : "Inbox");

  // a wide screen opens the first decision at once; a phone shows the list first
  useEffect(() => {
    if (!item && list.length && wideScreen()) navigate(decisionHref(list[0]), { replace: true });
  }, [item, list]);

  // j / k move through the list
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.metaKey || e.ctrlKey || e.altKey || /input|textarea|select/i.test(t.tagName) || t.isContentEditable) return;
      if (e.key !== "j" && e.key !== "k") return;
      const i = current ? list.findIndex((d) => d.key === current.key) : -1;
      const next = list[e.key === "j" ? Math.min(list.length - 1, i + 1) : Math.max(0, i - 1)];
      if (next) navigate(decisionHref(next), { replace: true });
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [list, current]);

  function done(d: Decision) {
    const i = list.findIndex((x) => x.key === d.key);
    setDecided((s) => new Set(s).add(d.key));
    const rest = list.filter((x) => x.key !== d.key);
    const next = rest[Math.min(i, rest.length - 1)];
    navigate(next && wideScreen() ? decisionHref(next) : href("inbox", filter || undefined), { replace: true });
  }

  const apps = [...new Set(items.filter((d) => !decided.has(d.key)).map((d) => d.app))];
  const appName = (id: string) => s.apps.find((a) => a.id === id)?.name || items.find((d) => d.app === id)?.appName || id;
  const showDetail = !!item;

  return (
    <div className="-mt-2 sm:-mt-4">
      <div className={cx("mb-5 flex flex-wrap items-end justify-between gap-3", showDetail && "max-lg:hidden")}>
        <div>
          <h1 className="text-[22px] leading-tight font-semibold tracking-tight">Inbox</h1>
          <p className="mt-1 text-fg-3">{loading && !items.length ? "Loading…" : list.length ? `${plural(list.length, "decision")} waiting for you` : "Nothing waiting for you"}</p>
        </div>
        {(apps.length > 1 || filter) && (
          <Select h="sm" className="w-auto" value={filter} aria-label="Show the decisions of" onChange={(e) => navigate(href("inbox", e.target.value || undefined))}>
            <option value="">All apps</option>
            {[...new Set([...apps, ...(filter ? [filter] : [])])].map((a) => <option key={a} value={a}>{appName(a)}</option>)}
          </Select>
        )}
      </div>

      {error && !items.length ? <ErrorNote retry={reload}>{error}</ErrorNote>
        : loading && !items.length ? <SkeletonRows rows={3} />
        : !list.length && !current ? (
          <Empty icon={<CheckCheck />} title="You're all caught up"
            action={filter ? <a href={href("inbox")} className="link text-[13px] font-medium">Show every app</a> : undefined}>
            Approvals, go-lives and data changes routed to you show up here. Slack and email tell you when one arrives.
          </Empty>
        ) : (
          <div className="grid items-start gap-5 lg:grid-cols-[minmax(300px,380px)_minmax(0,1fr)]">
            <Card className={cx("divide-y divide-line overflow-hidden", showDetail && "max-lg:hidden")}>
              {list.map((d) => <DecisionRow key={d.key} d={d} href={decisionHref(d)} active={current?.key === d.key} showApp={!filter} />)}
              {!list.length && <div className="px-4 py-6 text-center text-[13px] text-fg-3">Nothing else waiting</div>}
            </Card>
            {current ? <Detail key={current.key} d={current} onDone={() => done(current)} />
              : item ? (
                <Card className="p-6">
                  <a href={href("inbox", filter || undefined)} className="mb-4 inline-flex items-center gap-1.5 text-[13px] text-fg-3 hover:text-fg lg:hidden"><ArrowLeft className="size-4" />Inbox</a>
                  <Empty compact icon={<InboxIcon />} title="This decision is no longer waiting">Someone decided it already, or it was withdrawn.</Empty>
                </Card>
              ) : <div className="hidden lg:block" />}
          </div>
        )}
    </div>
  );
}

function Detail({ d, onDone }: { d: Decision; onDone: () => void }) {
  const { me } = useSession();
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState<"approve" | "decline" | null>(null);
  const p = d.approval?.payload ?? {};
  const mine = !!me && d.by === me.sub;

  async function go(approve: boolean) {
    if (approve && d.kind === "deletion" && !(await confirm({
      title: `Delete ${d.appName} for good?`, danger: true, confirm: "Delete app", typeToConfirm: d.app,
      body: <p>Every release, the live and preview data, schedules, runs and approvals of {d.appName} are deleted. Only the audit log keeps its history.</p>,
    }))) return;
    setBusy(approve ? "approve" : "decline");
    try {
      await decide(d, approve, note);
      toast.success(approve ? (d.kind === "go-live" ? `${String(p.release ?? "The release")} is going live` : d.kind === "deletion" ? `${d.appName} deleted` : d.kind === "guest" ? `${d.request?.email} can use ${d.appName}` : "Approved") : "Declined");
      onDone();
    } catch (e) { toast.error(errMsg(e)); }
    setBusy(null);
  }

  const approveLabel = d.kind === "go-live" ? "Approve and go live" : d.kind === "deletion" ? "Delete app" : d.kind === "guest" ? "Let them in" : "Approve";
  const runId = d.approval?.run_id;

  return (
    <Card className="flex min-h-[420px] animate-fade flex-col lg:sticky lg:top-6">
      <div className="border-b border-line px-5 py-4 sm:px-6">
        <a href={href("inbox")} className="mb-3 inline-flex items-center gap-1.5 text-[13px] text-fg-3 hover:text-fg lg:hidden"><ArrowLeft className="size-4" />Inbox</a>
        <div className="flex items-start gap-3.5">
          <KindIcon kind={d.kind} size="lg" />
          <div className="min-w-0 flex-1">
            <div className="text-[13px] text-fg-3">{KIND[d.kind].label} · <a href={href("app", d.app)} className="hover:text-fg hover:underline">{d.appName}</a></div>
            <h2 className="mt-0.5 text-lg leading-snug font-semibold text-fg">{d.title}</h2>
            <div className="mt-1 text-[13px] text-fg-3">
              {d.by ? <>Asked by <PersonName id={d.by} known={d.byKnown} /></> : "Asked"} <Time at={d.at}>{ago(d.at)}</Time>
            </div>
          </div>
        </div>
      </div>

      <div className="flex-1 space-y-6 px-5 py-5 sm:px-6">
        {d.kind === "step" && <>
          <Block title="What you're asked to approve"><Payload value={p.payload ?? stripMeta(p)} empty="The workflow sent no details." app={d.app} /></Block>
          <Facts items={[
            ["Workflow", words(d.approval?.workflow || "")],
            ["Step", words(d.approval?.step || "")],
            ["Routed to", routedTo(d.approval?.to)],
            ...(typeof p.timeout === "string" ? [["Waits up to", spelled(p.timeout)] as [string, string]] : []),
          ]} />
          {runId && <a href={href("app", d.app, "runs", runId)} className="link inline-flex items-center gap-1 text-[13px] font-medium">View the run<ExternalLink className="size-3.5" /></a>}
        </>}

        {d.kind === "go-live" && <>
          <Block title="What changes when it goes live"><Changes delta={p.delta} /></Block>
          {typeof p.reason === "string" && <Callout title="Why it needs an approver">{p.reason}</Callout>}
          <a href={href("app", d.app, "releases", String(p.release ?? ""))} className="link inline-flex items-center gap-1 text-[13px] font-medium">Review the release<ExternalLink className="size-3.5" /></a>
        </>}

        {d.kind === "data" && <>
          <Block title="The change"><Payload value={p.fields ?? p.payload} empty="No details were given." /></Block>
          <Facts items={[
            ["Data", words(String(p.store ?? ""))],
            ["Where", p.target && p.target !== "live" ? "Preview data" : <span className="font-medium text-amber-700 dark:text-amber-300">Live data</span>],
            ["Routed to", routedTo(d.approval?.to)],
            ...(typeof p.expires_at === "string" ? [["Expires", `${ago(p.expires_at)}`] as [string, string]] : []),
          ]} />
        </>}

        {d.kind === "deletion" && (
          <Callout tone="danger" title={`Approving deletes ${d.appName} for good`}>
            Its {plural(Number(p.versions ?? 0), "release")}, its live and preview data, schedules, runs and approvals go. Only the audit log keeps its history.
          </Callout>
        )}

        {d.kind === "guest" && d.request && <>
          <Facts items={[
            ["Person", d.request.email],
            ["App", d.appName],
            ["Role", (d.request.roles || []).join(", ") || "—"],
            ["Until", d.request.until ? day(d.request.until) : "No end date"],
            ...(d.request.note ? [["Note", `“${d.request.note}”`] as [string, string]] : []),
          ]} />
          <Callout>They join as a guest: they reach {d.appName} with this role, and nothing else of the organization.</Callout>
        </>}
      </div>

      <div className="sticky bottom-0 space-y-3 rounded-b-xl border-t border-line bg-surface px-5 py-4 sm:px-6">
        {mine && d.kind !== "guest" && <p className="text-[13px] text-fg-3">You asked for this one: another person may have to decide it.</p>}
        <Textarea rows={2} value={note} onChange={(e) => setNote(e.target.value)} placeholder={d.kind === "guest" ? "A note for the person who asked (optional)" : "Add a note, or why you decline (optional)"} aria-label="Note" />
        <div className="flex flex-wrap items-center justify-end gap-2">
          <span className="mr-auto hidden text-xs text-fg-4 sm:block">j / k to move</span>
          <Button variant="secondary" loading={busy === "decline"} disabled={!!busy} onClick={() => go(false)}>{d.kind === "guest" ? "Refuse" : "Decline"}</Button>
          <Button variant={d.kind === "deletion" ? "danger" : "primary"} loading={busy === "approve"} disabled={!!busy} onClick={() => go(true)}>{approveLabel}</Button>
        </div>
      </div>
    </Card>
  );
}

const Block = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <section>
    <h3 className="mb-2.5 text-[13px] font-medium text-fg-3">{title}</h3>
    {children}
  </section>
);

/** A duration as the workflow wrote it (`7d`, `48h`, `30m`), in words. */
const spelled = (t: string) => {
  const m = /^(\d+)\s*(d|h|m|s)$/.exec(t.trim());
  if (!m) return t;
  const n = Number(m[1]), unit = { d: "day", h: "hour", m: "minute", s: "second" }[m[2] as "d" | "h" | "m" | "s"];
  return `${n} ${unit}${n === 1 ? "" : "s"}`;
};

/** Who a step is routed to, in words: a role, the person who started the run, the approvers. */
function routedTo(to?: string) {
  if (!to) return "—";
  if (to === "user:initiator") return "The person who started it";
  if (to.startsWith("user:")) return personName(to.slice(5));
  if (to === "approver") return "Approvers";
  return <>People with the <b className="font-medium">{to}</b> role</>;
}

/** A payload without the fields the inbox already shows. */
function stripMeta(p: Record<string, unknown>) {
  const { kind: _k, step: _s, to: _t, timeout: _o, workflow: _w, requested_by: _r, ...rest } = p;
  return rest;
}

/** A release's change review, in words. */
export function Changes({ delta }: { delta: unknown }) {
  const d = (delta && typeof delta === "object" ? delta : {}) as { new_or_changed_writes?: string[]; added_workflows?: string[]; removed_workflows?: string[]; migrations?: string[] };
  const rows: [string, string, string][] = [
    ...(d.new_or_changed_writes || []).map((w): [string, string, string] => ["New or changed write", w, "warning"]),
    ...(d.migrations || []).map((m): [string, string, string] => ["Runs on live data", m, "warning"]),
    ...(d.added_workflows || []).map((w): [string, string, string] => ["Adds workflow", w, "brand"]),
    ...(d.removed_workflows || []).map((w): [string, string, string] => ["Removes workflow", w, "neutral"]),
  ];
  if (!rows.length) return <p className="text-fg-3">No new write, no migration, no workflow added or removed.</p>;
  return (
    <ul className="space-y-2">
      {rows.map(([what, name, tone], i) => (
        <li key={i} className="flex items-start gap-2.5">
          <span className={cx("mt-1.5 size-1.5 shrink-0 rounded-full", tone === "warning" ? "bg-amber-500" : tone === "brand" ? "bg-brand-500" : "bg-fg-4")} />
          <span>{what} <code className="rounded bg-surface-2 px-1 py-0.5 text-[12.5px] text-fg">{name}</code></span>
        </li>
      ))}
    </ul>
  );
}

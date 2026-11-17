import { useState } from "react";
import { Database, ExternalLink } from "lucide-react";
import { api, authMode, errMsg, post } from "../../lib/api";
import { ago, short, tseg } from "../../lib/format";
import { invalidate, useQuery } from "../../lib/query";
import { href, navigate } from "../../lib/router";
import type { AppDetail, Approval, StoreInstance, Version } from "../../lib/types";
import { Badge, Button, ButtonLink, Callout, cx, Empty, ErrorNote, Select, Spinner } from "../../ui/core";
import { confirm } from "../../ui/dialog";
import { toast } from "../../ui/toast";
import { resetPreview } from "../app/releases-lib";
import { StoreBrowser } from "./StoreBrowser";

// An app's data as spreadsheets, read and written as the person's role through the same generated tools and guarded
// query a workflow gets: nothing its rules refuse is shown or writable. Its builders also see each release's preview
// data, the schema and SQL. Route: #/app/<app>/data/<live|short hash>/<store>/<table>

export function DataTab({ app, d, parts, builder }: { app: string; d: AppDetail; parts: string[]; builder: boolean }) {
  const [tsel, ssel, tbl] = parts;
  const insts = useQuery<StoreInstance[]>(`/v1/apps/${app}/stores`, () => api<StoreInstance[]>(`/v1/apps/${app}/stores`));
  const versions = useQuery<Version[]>(builder ? `/v1/apps/${app}/versions` : null, () => api<Version[]>(`/v1/apps/${app}/versions`).catch(() => []));
  const approvals = useQuery<Approval[]>(`/v1/apps/${app}/approvals`, () => api<Approval[]>(`/v1/apps/${app}/approvals`).catch(() => []));
  // bumped after a reset so the browser reloads the recreated databases
  const [generation, setGeneration] = useState(0);
  if (insts.error && !insts.data) return <ErrorNote retry={insts.reload}>{insts.error}</ErrorNote>;
  if (!insts.data) return <Spinner />;

  const byTarget: Record<string, StoreInstance[]> = {};
  insts.data.filter((i) => i.target !== "local").forEach((i) => (byTarget[i.target] ||= []).push(i));
  const vs = versions.data || [];
  const versionOf = (h: string) => vs.find((v) => v.hash === h);
  const targets = Object.keys(byTarget).sort((a, b) => a === "live" ? -1 : b === "live" ? 1 : (versionOf(b)?.created_at || "").localeCompare(versionOf(a)?.created_at || ""));
  if (!targets.length) return (
    <Empty icon={<Database />} title={builder ? "No data yet" : "Nothing to show yet"}>
      {builder && d.stores_configured === false
        ? <>This server has no database for the apps' data. Set <code>CORI_STORE_DATABASE_URL</code> on the server (a Postgres role with CREATEDB and CREATEROLE), restart it, then deploy again.</>
        : builder ? <>Declare the app's data in <code>stores/&lt;name&gt;/</code> (migrations, roles, seed): each deploy gets its own seeded preview data, going live creates or migrates the live data.</>
        : "This app keeps no data you can open."}
    </Empty>
  );

  const target = targets.find((t) => tseg(t) === tsel) || targets[0];
  const stores = byTarget[target];
  const inst = stores.find((s) => s.name === ssel) || stores[0];
  const live = target === "live", v = versionOf(target);
  const here = (t: string, store?: string, table?: string) => href("app", app, "data", tseg(t), store, table);
  const pending = (approvals.data || []).filter((a) => a.id.startsWith("store:") && a.payload.store === inst.name && (a.payload.target || "live") === target).length;
  const label = (t: string) => {
    if (t === "live") return `Live data${d.promoted_release ? ` · ${d.promoted_release}` : ""}`;
    const tv = versionOf(t);
    return `Preview data · ${tv?.release || short(t)}${tv?.message ? ` · ${tv.message}` : ""}`;
  };
  const refresh = () => { invalidate(`/v1/apps/${app}/stores`); setGeneration((g) => g + 1); };

  async function resetLive() {
    if (!(await confirm({ title: "Reset the live data?", danger: true, confirm: "Delete all live data", typeToConfirm: app,
      body: <><p className="font-medium text-rose-700 dark:text-rose-400">Dev mode: this deletes all data in the live stores of {app}.</p><p>They are recreated from the live release's migrations and seed files. Preview data is not touched.</p></> }))) return;
    try {
      const r = await post<{ live?: { store: string; seeds_applied?: string[] }[] }>(`/v1/apps/${app}/stores/reset`);
      toast.success(`Live data reset: ${(r.live || []).map((x) => x.store).join(", ") || "no store"}`);
    } catch (e) { toast.error(errMsg(e)); }
    refresh();
  }

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2.5">
        {builder && targets.length > 1 ? (
          <Select h="sm" className="w-auto max-w-[min(100%,360px)]" value={here(target, inst.name, tbl)} onChange={(e) => navigate(e.target.value)} aria-label="Which data">
            {targets.map((t) => <option key={t} value={t === target ? here(t, inst.name, tbl) : here(t, byTarget[t].some((s) => s.name === inst.name) ? inst.name : undefined)}>{label(t)}</option>)}
          </Select>
        ) : builder && <span className="text-[13px] font-medium text-fg-2">{label(target)}</span>}
        {stores.length > 1 && (
          <span className="inline-flex overflow-hidden rounded-lg ring-1 ring-line ring-inset">
            {stores.map((s) => (
              <a key={s.name} href={here(target, s.name)} aria-current={s.name === inst.name ? "page" : undefined}
                className={cx("px-3 py-1.5 text-[13px]", s.name === inst.name ? "bg-brand-600 font-medium text-white" : "bg-surface text-fg-2 hover:bg-surface-2")}>{s.name}</a>
            ))}
          </span>
        )}
        {builder && (live ? <Badge tone="warning" dot title="what people use: a change you make here applies at once">Live data</Badge> : <Badge dot title="this release's own test data: nothing you change here reaches the live data">Preview data</Badge>)}
        <span className="ml-auto flex flex-wrap items-center gap-2">
          {pending > 0 && <a href={href("inbox", app)}><Badge tone="warning">{pending} change{pending > 1 ? "s" : ""} waiting for approval</Badge></a>}
          {builder && !live && v && <>
            <ButtonLink href={v.preview_url} external size="xs" variant="ghost" icon={<ExternalLink />}>Open the preview</ButtonLink>
            <Button size="xs" onClick={async () => { await resetPreview(app, v); refresh(); }}>Reset preview data</Button>
          </>}
          {builder && live && authMode() === "dev" && <Button size="xs" variant="danger-soft" onClick={resetLive}>Reset live data…</Button>}
        </span>
      </div>
      {builder && <p className="mb-3 text-[13px] text-fg-3"><code>{inst.database}</code> · {inst.seeded ? "seeded · " : ""}migrated {ago(inst.updated_at)}</p>}
      {!builder && live && <Callout className="mb-3">What you see and change is limited by your role in {d.name || app}{(d.roles || []).length ? ` (${(d.roles || []).join(", ")})` : ""}.</Callout>}
      <StoreBrowser key={`${target}|${inst.name}|${generation}`} store={{ app, target, store: inst.name }} table={tbl} here={(table) => here(target, inst.name, table)} builder={builder} />
    </>
  );
}

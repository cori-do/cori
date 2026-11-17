import { useState, type ReactNode } from "react";
import { Plus, RotateCw } from "lucide-react";
import { api, del, errMsg, post, put } from "../../lib/api";
import { eur, plural, sizeText } from "../../lib/format";
import { invalidate, useQuery } from "../../lib/query";
import { href, navigate } from "../../lib/router";
import { useSession } from "../../lib/session";
import type { AppDetail, AppIdentity, Meters, Secret } from "../../lib/types";
import { Badge, Button, Callout, Card, CopyField, Facts, Input, RadioCards, Section, Skeleton } from "../../ui/core";
import { confirm, openDialog } from "../../ui/dialog";
import { ClientState, EventsList, RoleState } from "../../ui/identity";
import { SecretForm, SecretsList } from "../../ui/secrets";
import { toast } from "../../ui/toast";
import { Accounts } from "../../ui/connections";

// An app's settings: when going live needs an approver, its credentials, the accounts its runs act with, its AI budget,
// its storage, its sign-in, and deleting it.

export function AppSettings({ app, d }: { app: string; d: AppDetail }) {
  const { me } = useSession();
  const mayEdit = !!me && (me.sub === d.owner || !!me.can?.admin);
  const name = d.name || app;
  return (
    <div className="max-w-3xl">
      <Section title="About">
        <Card className="p-5">
          <Facts items={[
            ["Name", name],
            ["Id", <code className="text-[13px]">{app}</code>],
            ["Address", d.front_url ? <a href={d.front_url} target="_blank" rel="noreferrer" className="link break-all">{d.front_url}</a> : <span className="text-fg-3">Once a release is live</span>],
            ["Get its code", <CopyField value={`cori pull ${app} ./${app}`} className="max-w-sm" />],
          ]} />
        </Card>
      </Section>
      <Policy app={app} d={d} mayEdit={mayEdit} />
      <Credentials app={app} name={name} mayEdit={mayEdit} />
      {!!me?.providers?.length && (
        <Section title="Connected accounts" description={<>The account its runs act with when a connector says <code>account = "app"</code> in its cori.toml: for its schedules, or a mailbox its people share. Otherwise each run acts with its person's own. {mayEdit ? "" : "Its owner or an admin connects it."}</>}>
          <Accounts base={`/v1/apps/${encodeURIComponent(app)}/connections`} mayEdit={mayEdit} whose="app" />
        </Section>
      )}
      {me?.can?.admin && <Budget app={app} />}
      {(mayEdit || me?.can?.admin) && <Storage app={app} admin={!!me?.can?.admin} />}
      <SignIn app={app} />
      {mayEdit && <Danger app={app} d={d} />}
    </div>
  );
}

function Policy({ app, d, mayEdit }: { app: string; d: AppDetail; mayEdit: boolean }) {
  const [busy, setBusy] = useState(false);
  async function set(on: boolean) {
    if (on === !!d.protected) return;
    setBusy(true);
    try { await post(`/v1/apps/${app}/protect`, { protected: on }); toast.success(on ? "Every release now needs an approver" : "Only releases that add or change a write need an approver"); }
    catch (e) { toast.error(errMsg(e)); }
    invalidate(`/v1/apps/${app}`, "/v1/me/apps");
    setBusy(false);
  }
  return (
    <Section title="Going live" description={mayEdit ? "When a new release needs an approver before people use it. Rolling back never does." : "Its owner or an admin changes this."}>
      <RadioCards name="policy" value={d.protected ? "every" : "writes"} disabled={!mayEdit || busy} onChange={(v) => set(v === "every")} options={[
        { value: "writes", title: "When a release adds or changes a write", body: "Builders make releases live themselves, unless a release posts, pays, updates data or deletes something new. Recommended." },
        { value: "every", title: "For every release", body: "An approver decides each new release. The app cannot be deleted while this is on." },
      ]} />
    </Section>
  );
}

function Credentials({ app, name, mayEdit }: { app: string; name: string; mayEdit: boolean }) {
  const q = useQuery<Secret[] | null>(`/v1/secrets?app=${app}`, () => api<Secret[]>(`/v1/secrets?app=${encodeURIComponent(app)}`).catch(() => null));
  if (q.data === null) return null;
  async function add() {
    const v = await openDialog<{ name: string; value: string }>((close) => <SecretForm scope={name} close={close} />);
    if (!v) return;
    try { await post(`/v1/secrets`, { name: v.name, value: v.value, app }); toast.success(`${v.name} saved`); } catch (e) { toast.error(errMsg(e)); }
    q.reload();
  }
  return (
    <Section title="Credentials" description="Keys and passwords its connectors, webhooks and schedules use. Values are sealed and never shown."
      actions={mayEdit && <Button size="xs" icon={<Plus />} onClick={add}>Add</Button>}>
      {!q.data ? <Skeleton className="h-14 w-full" /> : <SecretsList secrets={q.data} empty={<>Its runs use the organization's credentials. {mayEdit ? "Add one for this app only, or" : "Its owner or an admin adds one, or"} run <code>cori secrets set &lt;name&gt; --app {app}</code>.</>} />}
    </Section>
  );
}

function Budget({ app }: { app: string }) {
  const q = useQuery<Meters>(`/v1/apps/${app}/meters`, () => api<Meters>(`/v1/apps/${app}/meters`));
  const m = q.data?.llm.month;
  const initial = m?.budget_eur == null ? "" : String(m.budget_eur);
  const [v, setV] = useState<string | null>(null);
  const value = v ?? initial;
  const [busy, setBusy] = useState(false);
  async function save() {
    const n = value.trim() === "" ? null : Number(value.replace(",", "."));
    if (n !== null && !(n >= 0)) return toast.error("A budget is an amount of at least 0, or empty for no limit");
    setBusy(true);
    try { await put(`/v1/apps/${app}/llm/budget`, { monthly_eur: n }); toast.success(n === null ? "No AI budget limit" : `AI budget: ${eur(n)} a month`); setV(null); }
    catch (e) { toast.error(errMsg(e)); }
    invalidate(`/v1/apps/${app}/meters`, "/v1/llm");
    setBusy(false);
  }
  return (
    <Section title="AI budget" description="What its AI steps may spend each month. Once spent, they fail until the next month or a higher budget.">
      <Card className="flex flex-wrap items-end gap-4 p-5">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] text-fg-3">Spent this month</div>
          <div className="mt-0.5 text-lg font-semibold tabular-nums">{m ? eur(m.spent_eur) : "–"}{m?.blocked && <Badge tone="danger" className="ml-2 align-middle">Budget spent</Badge>}</div>
          {m && <div className="text-[13px] text-fg-3">{plural(m.calls, "call")}, {(m.tokens_in + m.tokens_out).toLocaleString()} tokens</div>}
        </div>
        <form className="flex items-end gap-2" onSubmit={(e) => { e.preventDefault(); save(); }}>
          <label className="block">
            <span className="mb-1.5 block text-[13px] font-medium">Monthly budget (€)</span>
            <Input className="w-36 tabular-nums" inputMode="decimal" placeholder="No limit" value={value} onChange={(e) => setV(e.target.value)} />
          </label>
          <Button type="submit" variant="primary" loading={busy} disabled={value === initial}>Save</Button>
        </form>
      </Card>
    </Section>
  );
}

/** What the app takes, and its limit: its own, which an admin sets, within the organization's for each app. */
function Storage({ app, admin }: { app: string; admin: boolean }) {
  const q = useQuery<Meters>(`/v1/apps/${app}/meters`, () => api<Meters>(`/v1/apps/${app}/meters`));
  const s = q.data?.storage;
  const initial = s?.own == null ? "" : sizeText(s.own);
  const [v, setV] = useState<string | null>(null);
  const value = v ?? initial;
  const [busy, setBusy] = useState(false);
  const full = !!s && s.limit != null && s.bytes >= s.limit;
  async function save() {
    const t = value.trim();
    setBusy(true);
    try {
      const r = await put<{ own: number | null; limit: number | null }>(`/v1/apps/${app}/limits`, { storage_bytes: t === "" ? null : /^\d+$/.test(t) ? Number(t) : t });
      toast.success(r.own == null ? (r.limit == null ? "No storage limit" : `The organization's limit for each app applies: ${sizeText(r.limit)}`) : `Storage limit: ${sizeText(r.own)}`);
      setV(null);
    } catch (e) { toast.error(errMsg(e)); }
    invalidate(`/v1/apps/${app}/meters`);
    setBusy(false);
  }
  const org = s?.organization;
  return (
    <Section title="Storage" description="What its data, releases and files take, measured every few minutes. Past its limit, its deploys, uploads and new data are refused; reading and deleting still work.">
      <Card className="flex flex-wrap items-end gap-4 p-5">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] text-fg-3">Takes</div>
          <div className="mt-0.5 text-lg font-semibold tabular-nums">
            {s ? sizeText(s.bytes) : "–"}{s?.limit != null && <span className="text-[13px] font-normal text-fg-3"> of {sizeText(s.limit)}</span>}
            {full && <Badge tone="danger" className="ml-2 align-middle">Full</Badge>}
          </div>
          {s?.limit != null && (
            <div className="mt-2 h-1.5 w-full max-w-xs overflow-hidden rounded-full bg-surface-2 ring-1 ring-line" role="meter" aria-valuemin={0} aria-valuemax={s.limit} aria-valuenow={s.bytes} aria-label="Storage used">
              <div className={full ? "h-full bg-red-500" : "h-full bg-brand-500"} style={{ width: `${Math.min(100, (s.bytes / s.limit) * 100)}%` }} />
            </div>
          )}
          {s && <div className="mt-2 text-[13px] text-fg-3">
            {s.per_app != null ? <>The organization allows each app {sizeText(s.per_app)}. </> : null}
            The organization takes {sizeText(org?.bytes)}{org?.limit != null ? <> of {sizeText(org.limit)}</> : null}.
          </div>}
        </div>
        {admin && (
          <form className="flex items-end gap-2" onSubmit={(e) => { e.preventDefault(); save(); }}>
            <label className="block">
              <span className="mb-1.5 block text-[13px] font-medium">Its own limit</span>
              <Input className="w-36 tabular-nums" placeholder={s?.per_app != null ? sizeText(s.per_app) : "No limit"} value={value} onChange={(e) => setV(e.target.value)} />
            </label>
            <Button type="submit" variant="primary" loading={busy} disabled={value === initial}>Save</Button>
          </form>
        )}
      </Card>
    </Section>
  );
}

/** The app's own sign-in client and roles at the identity provider (advanced: shown only when the deployment has the hook). */
function SignIn({ app }: { app: string }) {
  const q = useQuery<AppIdentity | null>(`/v1/apps/${app}/identity`, () => api<AppIdentity>(`/v1/apps/${app}/identity`).catch(() => null));
  const ident = q.data;
  if (!ident?.hook) return null;
  const roles = ident.roles || [];
  async function again() {
    try {
      const r = await post<{ client_status: string; client_id?: string | null; note?: string | null }>(`/v1/apps/${app}/oidc-client`);
      toast(r.client_status === "ready" ? "The sign-in is ready" : r.client_status === "pending" ? `Still being set up${r.note ? `: ${r.note}` : ""}` : "The identity provider gave no sign-in client");
    } catch (e) { toast.error(errMsg(e)); }
    q.reload();
  }
  return (
    <Section title="Sign-in" description="People sign in to the app through your identity provider, with the app's own client; its roles are kept in sync there."
      actions={ident.may_sync && <Button size="xs" icon={<RotateCw />} onClick={again}>Sync again</Button>}>
      <Card className="divide-y divide-line">
        <Line label="Sign-in client"><ClientState status={ident.client_status} clientId={ident.client_id} note={ident.client_note} /></Line>
        <Line label="Roles">
          {!ident.role_events ? <span className="text-fg-3">Created by your identity team, not synced one by one</span>
            : roles.length ? <span className="flex flex-wrap gap-x-4 gap-y-1.5">{roles.map((r) => <span key={r.role} className="inline-flex items-center gap-2"><b className="font-medium">{r.role}</b><RoleState r={r} /></span>)}</span>
            : <span className="text-fg-3">None yet: {ident.roles_at === "promote" ? "going live" : "a deploy"} tells your identity provider of them</span>}
        </Line>
        {ident.admin_url && <Line label="Grant roles"><a href={ident.admin_url} target="_blank" rel="noopener noreferrer" className="link">In your identity provider</a></Line>}
      </Card>
      {ident.events.length > 0 && (
        <details className="mt-3">
          <summary className="cursor-pointer text-[13px] font-medium text-fg-3 hover:text-fg">What was sent to your identity provider</summary>
          <div className="mt-3"><EventsList events={ident.events.slice(0, 10)} /></div>
        </details>
      )}
    </Section>
  );
}

const Line = ({ label, children }: { label: string; children: ReactNode }) => (
  <div className="flex flex-wrap items-start gap-x-6 gap-y-1 px-4 py-3"><span className="w-32 shrink-0 text-[13px] text-fg-3">{label}</span><span className="min-w-0 flex-1">{children}</span></div>
);

function Danger({ app, d }: { app: string; d: AppDetail }) {
  const name = d.name || app;
  async function remove() {
    if (d.protected) { toast.error("Every release of this app needs an approver: change that under Going live first"); return; }
    if (!(await confirm({ title: `Delete ${name}?`, danger: true, confirm: "Delete app", typeToConfirm: app,
      body: <><p>Every release, its live and preview data, schedules, runs and approvals are deleted. Only the audit log keeps its history.</p><p>An approver decides; this cannot be undone.</p></> }))) return;
    try {
      const r = await del<{ status?: string; approver_roles?: string[] }>(`/v1/apps/${app}?confirm=${encodeURIComponent(app)}`);
      if (r.status === "pending_approval") toast.success("Deletion requested: an approver decides it from their inbox");
      else { toast.success(`${name} deleted`); navigate(href()); }
    } catch (e) { toast.error(errMsg(e)); }
    invalidate("/v1/me", "/v1/apps", "decisions");
  }
  return (
    <Section title="Delete the app">
      <Callout tone="danger" action={<Button variant="danger" onClick={remove}>Delete…</Button>}>
        Removes {name}, its releases and all of its data, live data included. An approver decides.
      </Callout>
    </Section>
  );
}

import { useState } from "react";
import { ExternalLink, Plus, UserPlus, X } from "lucide-react";
import { api, del, errMsg, post, put } from "../../lib/api";
import { personName, usePeople } from "../../lib/people";
import { invalidate, useQuery } from "../../lib/query";
import { useSession } from "../../lib/session";
import type { AccessRequest, AppAccessInfo, AppDetail } from "../../lib/types";
import { outcome, RequestsList } from "../../ui/access";
import { Avatar, Badge, Button, Callout, Card, cx, ErrorNote, Field, IconButton, Input, List, Section, Segmented, Select, SkeletonRows } from "../../ui/core";
import { confirm, Modal, openDialog } from "../../ui/dialog";
import { PeoplePicker, PersonChip, PersonPicker } from "../../ui/people";
import { toast } from "../../ui/toast";

// Who uses the app (the people holding each of its roles) and who builds it (its owner and builders). Its owner and admins
// share its roles with members, or with someone outside the organization once an admin approves; each change is a
// request the identity provider answers (or the organization's access webhook). What the deployment says of giving roles
// (`message`: how long a change takes to reach people) shows above them, and where they are given.

export function Access({ app, d }: { app: string; d: AppDetail }) {
  const { me } = useSession();
  usePeople();
  const q = useQuery<AppAccessInfo | null>(`/v1/apps/${app}/access`, () => api<AppAccessInfo>(`/v1/apps/${app}/access`));
  const mayEdit = !!me && (me.sub === d.owner || !!me.can?.admin);
  const after = () => invalidate(`/v1/apps/${app}`, "/v1/people", "/v1/me");
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={3} />;
  const acc = q.data;
  const roles = acc.roles.filter((r) => r.declared || r.holders.length);
  const declared = roles.filter((r) => r.declared);
  const where = acc.admin_url;
  const name = d.name || app;
  const message = acc.message || null;

  async function share(role?: string) {
    const v = await openDialog<Share>((close) => <ShareForm app={name} roles={declared} role={role} admin={!!me?.can?.admin} message={message} close={close} />);
    if (!v) return;
    if ("email" in v) {
      try {
        const [msg, bad] = outcome(`${name} shared with ${v.email}`, await post<AccessRequest>(`/v1/apps/${app}/access`, { email: v.email, roles: [v.role], granted: true, until: v.until || undefined, note: v.note || undefined }));
        (bad ? toast.error : toast.success)(msg);
      } catch (e) { toast.error(errMsg(e)); }
      return after();
    }
    const said: [string, boolean][] = [];
    for (const person of v.people) {
      try { said.push(outcome(`${personName(person)} · ${v.role}`, await post<AccessRequest>(`/v1/apps/${app}/access`, { person, roles: [v.role], granted: true }))); }
      catch (e) { said.push([`${personName(person)}: ${errMsg(e)}`, true]); }
    }
    const notDone = said.filter(([m]) => !m.endsWith(": done"));
    (said.some(([, b]) => b) ? toast.error : toast.success)(notDone.length ? notDone.map(([m]) => m).join("; ") : `${v.role} given to ${v.people.length === 1 ? personName(v.people[0]) : `${v.people.length} people`}`);
    after();
  }
  async function takeBack(role: string, h: { id: string; name?: string | null; email?: string | null }) {
    const who = h.name || h.email || personName(h.id);
    if (!(await confirm({ title: `Take ${role} back from ${who}?`, confirm: "Take it back", danger: true,
      body: <><p>They lose what <b>{role}</b> opens in {name}{message ? "." : " at their next sign-in."}</p>{message && <p className="text-fg-3">{message}</p>}</> }))) return;
    try { const [msg, bad] = outcome(`${role} taken back from ${who}`, await post<AccessRequest>(`/v1/apps/${app}/access`, { person: h.id, roles: [role], granted: false })); (bad ? toast.error : toast.success)(msg); }
    catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function builders() {
    const v = await openDialog<string[]>((close) => <PeoplePicker title={`Who builds ${name}`} chosen={d.builders || []} exclude={d.owner ? [d.owner] : []} close={close}
      description={<>Builders deploy releases, try previews and read all of the app's data (read-only; reads of live data are logged). The owner always is one.</>} />);
    if (v === null) return;
    try { await put(`/v1/apps/${app}/builders`, { builders: v }); toast.success("Builders saved"); } catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function handOver() {
    const v = await openDialog<string>((close) => <PersonPicker title={`Transfer ${name}`} exclude={d.owner ? [d.owner] : []} confirm="Transfer" close={close}
      description={<>The owner decides who builds the app, shares its roles and can delete it. {d.owner === me?.sub ? "You stay one of its builders." : "Its current owner stays one of its builders."}</>} />);
    if (!v) return;
    try {
      await put(`/v1/apps/${app}/builders`, { builders: [...new Set([...(d.builders || []), ...(d.owner ? [d.owner] : [])])].filter((b) => b !== v), owner: v });
      toast.success(`${name} now belongs to ${personName(v)}`);
    } catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function withdraw(r: AccessRequest) {
    try { await del(`/v1/access-requests/${encodeURIComponent(r.id)}`); toast.success(`The share with ${r.email} is withdrawn`); } catch (e) { toast.error(errMsg(e)); }
    after();
  }
  const pending = acc.requests.filter((r) => r.status !== "done" || Date.now() - Date.parse(r.updated_at) < 3600_000);

  return (
    <>
      <Section title={`Who uses ${name}`} description="People get the app, its workflows and its data through its roles."
        actions={acc.can.grant && declared.length > 0 && <Button variant="primary" icon={<UserPlus />} onClick={() => share()}>Share</Button>}>
        {acc.can.grant ? message && <Callout className="mb-3">{message}</Callout> : (
          <Callout className="mb-3" action={!acc.grants_here && where && <a href={where} target="_blank" rel="noopener noreferrer" className="link inline-flex items-center gap-1 text-[13px] font-medium">Open it<ExternalLink className="size-3.5" /></a>}>
            {acc.grants_here ? "Its owner and admins give its roles." : "Its roles are given in your identity provider."}{message && <> {message}</>}
          </Callout>
        )}
        {roles.length ? (
          <List>
            {roles.map((r) => (
              <div key={r.role} className="px-4 py-3.5">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium text-fg">{r.role}</span>
                  {!r.declared && <Badge tone="warning" title="no release declares it any more">No longer declared</Badge>}
                  <span className="min-w-0 flex-1 truncate text-[13px] text-fg-3">{r.description}</span>
                  {acc.can.grant && r.declared && <Button size="xs" variant="ghost" icon={<Plus />} onClick={() => share(r.role)}>Add</Button>}
                </div>
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {r.holders.map((h) => (
                    <span key={h.id} className="inline-flex items-center gap-1.5 rounded-full bg-surface-2 py-0.5 pr-1 pl-0.5 text-[13px] text-fg ring-1 ring-line" title={h.email || h.id}>
                      <Avatar name={h.name || h.email || h.id} size="xs" />
                      {h.name || h.email || personName(h.id)}
                      {h.guest && <span className="text-[11px] font-medium text-amber-700 dark:text-amber-300">guest</span>}
                      {acc.can.revoke
                        ? <IconButton label={`Take ${r.role} back from ${h.name || h.email || h.id}`} size="2xs" onClick={() => takeBack(r.role, h)}><X className="size-3" /></IconButton>
                        : <span className="w-1" />}
                    </span>
                  ))}
                  {!r.holders.length && <span className="text-[13px] text-fg-4">{acc.source === "directory" ? "Nobody yet" : "As your identity provider grants it"}</span>}
                </div>
              </div>
            ))}
          </List>
        ) : <Callout>No role yet: the live release declares them (who may open the app, start its workflows, read its data).</Callout>}
      </Section>

      <Section title={`Who builds ${name}`} description="They deploy releases, try previews and read the app's data.">
        <Card className="divide-y divide-line">
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <span className="w-20 shrink-0 text-[13px] text-fg-3">Owner</span>
            <span className="min-w-0 flex-1">{d.owner ? <PersonChip id={d.owner} /> : <span className="text-fg-3">Nobody: any builder</span>}</span>
            {acc.owner_left && <Badge tone="warning">Left the organization</Badge>}
            {mayEdit && <Button size="xs" variant="ghost" onClick={handOver}>Transfer</Button>}
          </div>
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <span className="w-20 shrink-0 text-[13px] text-fg-3">Builders</span>
            <span className="flex min-w-0 flex-1 flex-wrap gap-x-4 gap-y-2">
              {(d.builders || []).length ? (d.builders || []).map((b) => <PersonChip key={b} id={b} />) : <span className="text-fg-3">Only the owner</span>}
            </span>
            {mayEdit && <Button size="xs" variant="ghost" onClick={builders}>Edit</Button>}
          </div>
        </Card>
      </Section>

      {pending.length > 0 && (
        <Section title="Requests" description="Changes asked of your identity provider, and where each stands.">
          <RequestsList requests={pending} actions={(r) => r.status === "awaiting_approval" && (r.asked_by === me?.sub || me?.can?.admin) ? <Button size="xs" variant="ghost" onClick={() => withdraw(r)}>Withdraw</Button> : null} />
        </Section>
      )}
    </>
  );
}

type Share = { people: string[]; role: string } | { email: string; role: string; until: string; note: string };
const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s.]+$/;

/** Shares a role of the app with members, or with someone outside the organization (a guest, once an admin approves).
 *  `message`: what the deployment says of giving roles, in place of "at their next sign-in". */
function ShareForm({ app, roles, role, admin, message, close }: { app: string; roles: AppAccessInfo["roles"]; role?: string; admin: boolean; message: string | null; close: (v: Share | null) => void }) {
  const people = usePeople();
  const [outside, setOutside] = useState<"members" | "outside">("members");
  const [r, setR] = useState(role || roles[0]?.role || "");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [q, setQ] = useState("");
  const [email, setEmail] = useState("");
  const [until, setUntil] = useState("");
  const [note, setNote] = useState("");
  const held = new Set(roles.find((x) => x.role === r)?.holders.map((h) => h.id) || []);
  const shown = people.filter((p) => !p.guest && !held.has(p.id) && (!q || `${p.name || ""} ${p.email || ""}`.toLowerCase().includes(q.toLowerCase())));
  const toggle = (id: string) => setPicked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  const chosen = [...picked].filter((id) => !held.has(id));
  const today = new Date().toISOString().slice(0, 10);
  const validOutside = EMAIL.test(email.trim()) && !!r && (!until || until > today);
  const isOutside = outside === "outside";
  const submit = () => {
    if (isOutside) { if (validOutside) close({ email: email.trim().toLowerCase(), role: r, until, note: note.trim() }); }
    else if (chosen.length && r) close({ people: chosen, role: r });
  };
  return (
    <Modal size="lg" title={`Share ${app}`} onClose={() => close(null)} onSubmit={submit}
      footer={<>
        <Button variant="ghost" onClick={() => close(null)}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={isOutside ? !validOutside : !chosen.length || !r}>
          {isOutside ? (admin ? "Share" : "Ask an admin") : chosen.length > 1 ? `Give ${r} to ${chosen.length} people` : `Give ${r || "the role"}`}
        </Button>
      </>}>
      <Segmented label="With" value={outside} onChange={setOutside} options={[{ value: "members", label: "People in the organization" }, { value: "outside", label: "Someone outside" }]} />
      <Field label="Role">
        <Select value={r} onChange={(e) => setR(e.target.value)}>
          {roles.map((x) => <option key={x.role} value={x.role}>{x.role}{x.description ? ` — ${x.description}` : ""}</option>)}
        </Select>
      </Field>
      {isOutside ? <>
        <Field label="Their email address"><Input type="email" autoFocus value={email} placeholder="pat@partner.example" onChange={(e) => setEmail(e.target.value)} /></Field>
        <div className="grid gap-4 sm:grid-cols-[11rem_1fr]">
          <Field label="Until" optional><Input type="date" min={today} value={until} onChange={(e) => setUntil(e.target.value)} /></Field>
          <Field label="Why they need it" optional><Input value={note} placeholder="For the admin who approves" onChange={(e) => setNote(e.target.value)} /></Field>
        </div>
        <p className="text-[13px] text-fg-3">They join as a guest: they reach {app} with this role, and nothing else of the organization{admin ? "." : ", once an admin approves."}</p>
        {message && <p className="text-[13px] text-fg-3">{message}</p>}
      </> : <>
        <Input placeholder="Search people" aria-label="Search people" value={q} onChange={(e) => setQ(e.target.value)} />
        <div className="max-h-64 overflow-y-auto rounded-xl ring-1 ring-line">
          {shown.map((p) => (
            <label key={p.id} className={cx("flex cursor-pointer items-center gap-3 border-b border-line px-3 py-2 last:border-0", picked.has(p.id) ? "bg-brand-50/60 dark:bg-brand-500/10" : "hover:bg-surface-2")}>
              <input type="checkbox" checked={picked.has(p.id)} onChange={() => toggle(p.id)} className="size-4" />
              <PersonChip id={p.id} name={p.name} email={p.email} size="md" />
            </label>
          ))}
          {shown.length === 0 && <div className="px-3 py-6 text-center text-fg-3">{q ? "Nobody matches." : "Everyone already has it."}</div>}
        </div>
        <p className="text-[13px] text-fg-3">{message || "They get it at their next sign-in."}</p>
      </>}
    </Modal>
  );
}

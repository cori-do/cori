import { useState } from "react";
import { ExternalLink, MoreHorizontal, Search, UserPlus } from "lucide-react";
import { api, del, errMsg, post, put } from "../../lib/api";
import { brand } from "../../lib/brand";
import { ago, day, plural } from "../../lib/format";
import { personName, usePeople, usePeopleList } from "../../lib/people";
import { invalidate, useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import { useSession } from "../../lib/session";
import type { AccessRequest, AppAccessInfo, Invitation, Invitations, Leaving, MemberRole, PersonRow } from "../../lib/types";
import { outcome, powersText, RequestsList, RoleChip, roleChoice } from "../../ui/access";
import { Avatar, Badge, Button, Callout, cx, Empty, ErrorNote, Field, IconButton, Input, List, Segmented, Select, SkeletonRows, Textarea, Time } from "../../ui/core";
import { confirm, Modal, openDialog } from "../../ui/dialog";
import { Menu } from "../../ui/menu";
import { PersonName } from "../../ui/people";
import { toast } from "../../ui/toast";

// The organization's people: every member sees their colleagues; its admins invite people, set their roles and remove
// them, as far as the identity provider takes those changes (`can`). Each change is a request it answers.

type View = "people" | "invitations" | "requests";

export function Members() {
  const { me } = useSession();
  const admin = !!me?.can?.admin;
  const q = usePeopleList();
  const invs = useQuery<Invitations | null>(admin ? "/v1/invitations" : null, () => api<Invitations>("/v1/invitations").catch(() => null));
  const [view, setView] = useState<View>("people");
  const [text, setText] = useState("");
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={4} />;
  const list = q.data;
  const can = list.can || {};
  const after = () => invalidate("/v1/people", "/v1/invitations", "/v1/me");
  const say = ([msg, bad]: [string, boolean]) => (bad ? toast.error : toast.success)(msg);
  const people = list.people.filter((p) => !text || `${p.name || ""} ${p.email || ""} ${(p.roles || []).join(" ")}`.toLowerCase().includes(text.toLowerCase()));
  const pendingInvitations = (invs.data?.invitations || []) as Invitation[];
  const requests = list.requests.filter((r) => r.kind !== "invite" || r.status !== "done");

  async function invite() {
    const v = await openDialog<InviteRequest>((close) => <InviteForm roles={list.member_roles} close={close} />);
    if (!v) return;
    const said: [string, boolean][] = [];
    for (const email of v.emails) {
      try { said.push(outcome(email, await post<AccessRequest>("/v1/invitations", { email, roles: [v.role], grants: v.app ? [{ app: v.app, roles: [v.appRole] }] : [] }))); }
      catch (e) { said.push([`${email}: ${errMsg(e)}`, true]); }
    }
    const notDone = said.filter(([m]) => !m.endsWith(": done"));
    say([notDone.length ? notDone.map(([m]) => m).join("; ") : v.emails.length > 1 ? `${v.emails.length} people invited` : `${v.emails[0]} invited`, said.some(([, b]) => b)]);
    after();
  }
  async function setRoles(p: PersonRow) {
    const v = await openDialog<string[]>((close) => <RolesForm person={p} roles={list.member_roles} close={close} />);
    if (!v) return;
    try { say(outcome(`${p.name || p.email || p.id}'s roles`, await put<AccessRequest>(`/v1/people/${encodeURIComponent(p.id)}/roles`, { roles: v }))); }
    catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function remove(p: PersonRow) {
    const leave = await api<Leaving>(`/v1/people/${encodeURIComponent(p.id)}/leaving`).catch(() => ({ owns: p.apps.owns, builds: p.apps.builds, agent_sessions: 0, waiting: [] }) as Leaving);
    const owners = await openDialog<Record<string, string>>((close) => <RemoveForm person={p} leave={leave} me={me?.sub || ""} close={close} />);
    if (!owners) return;
    try { say(outcome(`Removing ${p.name || p.email || p.id}`, await del<AccessRequest>(`/v1/people/${encodeURIComponent(p.id)}`, { owners }))); }
    catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function invitation(i: Invitation, what: "resend" | "cancel") {
    if (what === "cancel" && !(await confirm({ title: `Cancel ${i.email}'s invitation?`, confirm: "Cancel invitation", danger: true, body: <p>Its link stops working. You can invite them again later.</p> }))) return;
    try {
      const r = what === "cancel" ? await del<AccessRequest>(`/v1/invitations/${encodeURIComponent(i.id)}`) : await post<AccessRequest>(`/v1/invitations/${encodeURIComponent(i.id)}/resend`);
      say(outcome(`${what === "cancel" ? "Cancelling" : "Resending"} ${i.email}'s invitation`, r));
    } catch (e) { toast.error(errMsg(e)); }
    after();
  }

  return (
    <>
      {list.problem && <Callout tone="warning" className="mb-4">Your identity provider did not answer ({list.problem}): these are the people who signed in here.</Callout>}
      {list.source === "seen" && !list.problem && (
        <Callout className="mb-4" action={list.admin_url && <a href={list.admin_url} target="_blank" rel="noopener noreferrer" className="link inline-flex items-center gap-1 text-[13px] font-medium">Open it<ExternalLink className="size-3.5" /></a>}>
          Your identity provider tells {brand.name} nothing about people: these are the people who signed in here. People and roles are managed there.
        </Callout>
      )}
      {list.source === "directory" && admin && !can.set_roles && !can.invite && (
        <Callout className="mb-4">{brand.name} reads your people from your identity provider; their roles are changed there.</Callout>
      )}

      <div className="mb-4 flex flex-wrap items-center gap-2">
        {admin && (pendingInvitations.length > 0 || requests.length > 0) && (
          <Segmented label="Show" value={view} onChange={setView} options={[
            { value: "people", label: "People", count: list.people.length },
            ...(pendingInvitations.length ? [{ value: "invitations" as View, label: "Invitations", count: pendingInvitations.length }] : []),
            ...(requests.length ? [{ value: "requests" as View, label: "Requests", count: requests.length }] : []),
          ]} />
        )}
        {view === "people" && (
          <div className="relative w-full sm:w-64">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
            <Input h="sm" className="pl-9" placeholder="Search people" aria-label="Search people" value={text} onChange={(e) => setText(e.target.value)} />
          </div>
        )}
        {can.invite && <Button variant="primary" className="ml-auto" icon={<UserPlus />} onClick={invite}>Invite people</Button>}
      </div>

      {view === "people" && (people.length ? (
        <List>
          {people.map((p) => <PersonLine key={p.id} p={p} roles={list.member_roles} me={me?.sub} can={can} onRoles={() => setRoles(p)} onRemove={() => remove(p)} />)}
        </List>
      ) : <Empty compact title={list.people.length ? "Nobody matches" : "Nobody yet"}>{list.people.length ? "Try another name or address." : "People appear here once they sign in."}</Empty>)}

      {view === "invitations" && (
        <List>
          {pendingInvitations.map((i) => (
            <div key={i.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
              <Avatar name={i.email} size="md" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 truncate font-medium text-fg">{i.email}{i.guest && <Badge tone="warning">guest</Badge>}</div>
                <div className="text-[13px] text-fg-3">
                  {(i.roles || []).join(", ") || "member"}{(i.grants || []).map((g) => ` · ${g.app || g.client_id}: ${g.roles.join(", ")}`).join("")}
                  {i.inviter && <> · invited by <PersonName id={i.inviter} /></>}{i.expires_at && <> · expires {day(i.expires_at)}</>}
                </div>
              </div>
              <span className="flex gap-1">
                {can.resend_invitations && <Button size="xs" variant="ghost" onClick={() => invitation(i, "resend")}>Resend</Button>}
                {can.cancel_invitations && <Button size="xs" variant="danger-soft" onClick={() => invitation(i, "cancel")}>Cancel</Button>}
              </span>
            </div>
          ))}
        </List>
      )}

      {view === "requests" && <RequestsList requests={requests} />}
      {!admin && requests.length > 0 && <div className="mt-8"><h3 className="mb-3 text-[15px] font-semibold">Your requests</h3><RequestsList requests={requests} /></div>}
    </>
  );
}

function PersonLine({ p, roles, me, can, onRoles, onRemove }: { p: PersonRow; roles: MemberRole[]; me?: string; can: { set_roles?: boolean; remove?: boolean }; onRoles: () => void; onRemove: () => void }) {
  // each app once, with every way the person is in it: "team-requests (owner, manager)"
  const byApp = new Map<string, string[]>();
  const add = (a: string, r: string) => byApp.set(a, [...(byApp.get(a) || []), r]);
  p.apps.owns.forEach((a) => add(a, "owner"));
  p.apps.builds.filter((a) => !p.apps.owns.includes(a)).forEach((a) => add(a, "builder"));
  p.apps.uses.forEach((u) => u.roles.forEach((r) => add(u.app, r)));
  const apps = [...byApp.entries()].map(([a, rs]): [string, string] => [a, rs.join(", ")]);
  const label = p.name || p.email || p.id;
  const actions = (can.set_roles || (can.remove && p.id !== me));
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 sm:flex-nowrap">
      <Avatar name={label} size="md" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-1.5 font-medium text-fg">
          <span className="truncate">{label}</span>
          {p.id === me && <span className="text-[13px] font-normal text-fg-3">(you)</span>}
          {p.guest && <Badge tone="warning" title="reaches the apps shared with them, nothing else">Guest</Badge>}
          {(p.roles || []).filter((r) => !roles.find((x) => x.name === r)?.guest).map((r) => <RoleChip key={r} name={r} roles={roles} />)}
        </div>
        <div className="truncate text-[13px] text-fg-3">
          {p.name && p.email ? p.email : null}
          {p.name && p.email && apps.length ? " · " : null}
          {apps.length > 0 && <span title={apps.map(([a, r]) => `${a} · ${r}`).join("\n")}>{plural(apps.length, "app")}: {apps.slice(0, 3).map(([a, r], i) => <span key={`${a}/${r}`}>{i ? ", " : ""}<a href={href("app", a)} className="hover:text-fg hover:underline">{a}</a> ({r})</span>)}{apps.length > 3 ? `, +${apps.length - 3}` : ""}</span>}
        </div>
        {p.guest && (p.shares || []).length > 0 && <div className="mt-0.5 text-[13px] text-fg-3">{(p.shares || []).map((s) => `${s.app} shared by ${personName(s.by)}${s.until ? ` until ${day(s.until)}` : ""}`).join(" · ")}</div>}
      </div>
      <div className="hidden w-28 shrink-0 text-right text-[13px] text-fg-3 md:block">{p.last_seen ? <Time at={p.last_seen}>{ago(p.last_seen)}</Time> : "Not signed in yet"}</div>
      {actions ? (
        <Menu items={[
          can.set_roles && { label: "Change roles…", onSelect: onRoles },
          can.remove && p.id !== me && "sep",
          can.remove && p.id !== me && { label: "Remove from the organization…", danger: true, onSelect: onRemove },
        ]}>
          {(t) => <IconButton {...t} label={`Actions for ${label}`} size="xs"><MoreHorizontal /></IconButton>}
        </Menu>
      ) : <span className="hidden w-7 sm:block" />}
    </div>
  );
}

type InviteRequest = { emails: string[]; role: string; app: string; appRole: string };
const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s.]+$/;

function InviteForm({ roles, close }: { roles: MemberRole[]; close: (v: InviteRequest | null) => void }) {
  const { apps } = useSession();
  const offered = roles.filter((r) => r.offered);
  const [emails, setEmails] = useState("");
  const [role, setRole] = useState(offered.find((r) => r.name === "member")?.name ?? offered[0]?.name ?? "member");
  const [app, setApp] = useState("");
  const [appRole, setAppRole] = useState("");
  const appRoles = useQuery<AppAccessInfo["roles"]>(app ? `/v1/apps/${app}/access#roles` : null, () => api<AppAccessInfo>(`/v1/apps/${app}/access`).then((a) => a.roles.filter((r) => r.declared)));
  const list = emails.split(/[\s,;]+/).map((e) => e.trim()).filter(Boolean);
  const wrong = list.filter((e) => !EMAIL.test(e));
  const valid = list.length > 0 && !wrong.length && !!role && (!app || !!appRole);
  return (
    <Modal size="lg" title="Invite people" description="Each gets an email to join the organization." onClose={() => close(null)}
      onSubmit={() => valid && close({ emails: [...new Set(list.map((e) => e.toLowerCase()))], role, app, appRole })}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary" disabled={!valid}>{list.length > 1 ? `Invite ${list.length} people` : "Invite"}</Button></>}>
      <Field label="Email addresses" hint="Separate them with commas or new lines." error={wrong.length ? `Not an address: ${wrong.join(", ")}` : undefined}>
        <Textarea autoFocus rows={3} value={emails} placeholder="ann@example.com, bob@example.com" onChange={(e) => setEmails(e.target.value)} />
      </Field>
      <Field label="Role in the organization" hint={`In ${brand.name}: ${powersText(roles.find((r) => r.name === role))}.`}>
        <Select value={role} onChange={(e) => setRole(e.target.value)}>
          {(offered.length ? offered : [{ name: "member", powers: [], offered: true }]).map((r) => <option key={r.name} value={r.name}>{roleChoice(r.name, roles)}</option>)}
        </Select>
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="And a role in an app" optional>
          <Select value={app} onChange={(e) => { setApp(e.target.value); setAppRole(""); }}>
            <option value="">No app</option>
            {apps.filter((a) => a.build).map((a) => <option key={a.id} value={a.id}>{a.name || a.id}</option>)}
          </Select>
        </Field>
        {app && (
          <Field label="Its role">
            <Select value={appRole} onChange={(e) => setAppRole(e.target.value)}>
              <option value="">Choose…</option>
              {(appRoles.data || []).map((r) => <option key={r.role} value={r.role}>{r.role}{r.description ? ` — ${r.description}` : ""}</option>)}
            </Select>
          </Field>
        )}
      </div>
    </Modal>
  );
}

function RolesForm({ person, roles, close }: { person: PersonRow; roles: MemberRole[]; close: (v: string[] | null) => void }) {
  const offered = roles.filter((r) => r.offered || (person.roles || []).includes(r.name));
  const [picked, setPicked] = useState<Set<string>>(() => new Set(person.roles || []));
  const toggle = (n: string) => setPicked((s) => { const x = new Set(s); if (x.has(n)) x.delete(n); else x.add(n); return x; });
  return (
    <Modal title={`${person.name || person.email || person.id}'s roles`} description="What they may do in the organization. Their roles in each app are given on the app's Access tab." onClose={() => close(null)} onSubmit={() => picked.size && close([...picked])}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary" disabled={!picked.size}>Save</Button></>}>
      {person.guest && <Callout tone="warning">They are a guest: a role makes them a member, who sees their colleagues and the apps given to them.</Callout>}
      <div className="overflow-hidden rounded-xl ring-1 ring-line">
        {offered.map((r) => (
          <label key={r.name} className={cx("flex cursor-pointer items-start gap-3 border-b border-line px-3.5 py-2.5 last:border-0", picked.has(r.name) ? "bg-brand-50/60 dark:bg-brand-500/10" : "hover:bg-surface-2")}>
            <input type="checkbox" checked={picked.has(r.name)} onChange={() => toggle(r.name)} className="mt-0.5 size-4" />
            <span className="min-w-0">
              <span className="block font-medium text-fg">{r.name}{!r.offered && <span className="ml-1.5 text-xs font-normal text-fg-3">not offered by your identity provider</span>}</span>
              <span className="block text-[13px] text-fg-3">{r.description ? `${r.description} · ` : ""}{powersText(r)}</span>
            </span>
          </label>
        ))}
        {offered.length === 0 && <div className="px-3 py-3 text-fg-3">Your identity provider names no role.</div>}
      </div>
    </Modal>
  );
}

function RemoveForm({ person, leave, me, close }: { person: PersonRow; leave: Leaving; me: string; close: (v: Record<string, string> | null) => void }) {
  const people = usePeople().filter((x) => x.id !== person.id && !x.guest);
  const [owners, setOwners] = useState<Record<string, string>>(() => Object.fromEntries(leave.owns.map((a) => [a, me])));
  const who = person.name || person.email || person.id;
  return (
    <Modal size="lg" title={`Remove ${who}?`} onClose={() => close(null)} onSubmit={() => close(owners)}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="danger">Remove</Button></>}>
      <p>{who} leaves the organization: their roles in it and in its apps go, and their sessions stop working at once. Their account stays with your identity provider.</p>
      {leave.owns.length > 0 && (
        <div className="space-y-2">
          <div className="text-[13px] font-medium text-fg">The apps they own go to</div>
          {leave.owns.map((a) => (
            <label key={a} className="flex items-center gap-3">
              <code className="w-36 shrink-0 truncate text-fg">{a}</code>
              <Select className="min-w-0 flex-1" value={owners[a]} onChange={(e) => setOwners({ ...owners, [a]: e.target.value })} aria-label={`New owner of ${a}`}>
                {people.map((x) => <option key={x.id} value={x.id}>{x.name || x.email || x.id}{x.id === me ? " (you)" : ""}</option>)}
              </Select>
            </label>
          ))}
        </div>
      )}
      {leave.builds.length > 0 && <p>They stop building {leave.builds.join(", ")}.</p>}
      {leave.agent_sessions > 0 && <p>Their {plural(leave.agent_sessions, "agent session")} {leave.agent_sessions === 1 ? "is" : "are"} revoked.</p>}
      {leave.waiting.length > 0 && <Callout tone="warning">{plural(leave.waiting.length, "approval")} of runs they started {leave.waiting.length === 1 ? "waits" : "wait"} on them: an admin decides {leave.waiting.length === 1 ? "it" : "them"} once they are gone.</Callout>}
    </Modal>
  );
}

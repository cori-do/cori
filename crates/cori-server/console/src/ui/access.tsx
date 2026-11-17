import type { ReactNode } from "react";
import { authMode, errMsg, post, switchOrganization } from "../lib/api";
import { brand } from "../lib/brand";
import { ago } from "../lib/format";
import { invalidate } from "../lib/query";
import type { AccessRequest, MemberRole } from "../lib/types";
import { Badge, List, Time, type Tone } from "./core";
import { confirm } from "./dialog";
import { PersonName } from "./people";
import { toast } from "./toast";

// People and access, as the Members page and an app's Access tab show them: member roles and what they give, and the
// changes asked of the identity provider (each a request it answers: done at once, later, or refused).

const POWER_TEXT: Record<string, string> = {
  admin: "people, settings and every app",
  operator: "connectors, credentials and system",
  auditor: "the audit log and usage",
  billing: "usage and budgets",
  builder: "creates and builds apps",
  store_admin: "the apps' databases",
  approver: "approves go-lives and deletions",
};

/** What a member role gives in Cori, in words. */
export function powersText(r?: MemberRole): string {
  const powers = (r?.powers || []).map((p) => POWER_TEXT[p] || p);
  return powers.length ? powers.join(", ") : "the apps given to them";
}

/** A member role as a choice: its name and what it is for. */
export function roleChoice(name: string, roles: MemberRole[]): string {
  const r = roles.find((x) => x.name === name);
  return `${name} — ${r?.description || powersText(r)}`;
}

export function RoleChip({ name, roles }: { name: string; roles: MemberRole[] }) {
  const r = roles.find((x) => x.name === name);
  const tone: Tone = r?.powers.includes("admin") ? "danger" : r?.powers.length ? "brand" : "neutral";
  return <Badge tone={tone} title={`${name}${r?.description ? ` — ${r.description}` : ""} · in ${brand.name}: ${powersText(r)}`}>{name}</Badge>;
}

const STATE: Record<AccessRequest["status"], [string, Tone, string]> = {
  awaiting_approval: ["Waiting for an admin", "warning", "an admin approves or refuses it"],
  sent: ["Sending", "brand", "on its way to your identity provider"],
  pending: ["Pending", "warning", "your identity provider took it and answers later"],
  done: ["Done", "success", "your identity provider made the change"],
  refused: ["Refused", "danger", "your identity provider refused it"],
  given_up: ["Not answered", "danger", "your identity provider did not answer in time"],
  cancelled: ["Withdrawn", "neutral", "withdrawn before it was made"],
  ended: ["Ended", "neutral", "a guest's access past its end date"],
};
export function RequestState({ r }: { r: AccessRequest }) {
  const [label, tone, title] = STATE[r.status] || [r.status, "neutral" as Tone, ""];
  return <Badge tone={tone} dot title={title}>{label}</Badge>;
}

const roles = (r: AccessRequest) => (r.roles || []).join(", ");
/** A request in words: what it asks, of whom. */
export function RequestWhat({ r }: { r: AccessRequest }): ReactNode {
  const who = r.person ? <PersonName id={r.person} /> : <>{r.email}</>;
  switch (r.kind) {
    case "invite": return <>Invite {r.email}{r.guest ? " as a guest" : roles(r) ? ` as ${roles(r)}` : ""}</>;
    case "roles": return <>{who}'s roles: {roles(r) || "none"}</>;
    case "remove": return <>Remove {who}</>;
    case "leave": return <>{who} leaves</>;
    case "grant": return <>Give {roles(r)} in {r.app} to {who}</>;
    case "share": return <>Share {r.app} ({roles(r)}) with {r.email}</>;
    case "end": return <>{who}'s access to {r.app} ends</>;
    case "revoke": return <>Take {roles(r)} in {r.app} back from {who}</>;
    case "cancel_invitation": return <>Cancel an invitation</>;
    case "resend_invitation": return <>Resend an invitation</>;
    case "organization": return <>Rename the organization</>;
    default: return <>{r.event}</>;
  }
}

/** Requests, newest first, with where each stands; `actions` for what the person may do with one. */
export function RequestsList({ requests, actions }: { requests: AccessRequest[]; actions?: (r: AccessRequest) => ReactNode }) {
  return (
    <List>
      {requests.map((r) => (
        <div key={r.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3">
          <div className="min-w-0 flex-1">
            <div className="text-fg"><RequestWhat r={r} /></div>
            <div className="text-[13px] text-fg-3">Asked by <PersonName id={r.asked_by} /> <Time at={r.asked_at}>{ago(r.asked_at)}</Time>{r.answer ? ` · ${r.answer}` : ""}</div>
          </div>
          <RequestState r={r} />
          {actions?.(r)}
        </div>
      ))}
    </List>
  );
}

/** What a request just made came to, for a toast: [message, is it bad news]. */
export function outcome(what: string, r: Partial<AccessRequest>): [string, boolean] {
  switch (r.status) {
    case "done": return [`${what}: done`, false];
    case "pending": return [`${what}: pending in your identity provider${r.answer ? ` (${r.answer})` : ""}`, false];
    case "sent": return [`${what}: sent to your identity provider`, false];
    case "awaiting_approval": return [`${what}: waiting for an admin's approval`, false];
    default: return [`${what}: ${STATE[r.status as AccessRequest["status"]]?.[0]?.toLowerCase() || r.status}${r.answer ? ` — ${r.answer}` : ""}`, true];
  }
}

/** Leaves the organization once the person confirms; when the identity provider made it, signs in to another one. */
export async function leaveOrganization(label: string) {
  if (!(await confirm({ title: `Leave ${label}?`, danger: true, confirm: "Leave", body: <p>You lose your roles in it and in its apps. Its admins can invite you again.</p> }))) return;
  try {
    const r = await post<AccessRequest>("/v1/organization/leave");
    const [msg, bad] = outcome(`Leaving ${label}`, r);
    (bad ? toast.error : toast.success)(msg);
    if (r.status === "done" && authMode() === "builtin") await switchOrganization();
  } catch (e) { toast.error(errMsg(e)); }
  invalidate("/v1/organization", "/v1/me");
}

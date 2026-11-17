import { api, post } from "./api";
import { href } from "./router";
import { words } from "./format";
import { personName, type Known } from "./people";
import { invalidate, useQuery } from "./query";
import { useSession } from "./session";
import type { AccessRequest, Approval, MyApp } from "./types";

// Everything waiting for the person's decision, as one list in plain words: a workflow's human step, a release's go-live,
// a parked change to an app's data, an app's deletion, and (admins) an app shared outside the organization.

export type DecisionKind = "step" | "go-live" | "data" | "deletion" | "guest";
export type Decision = {
  /** unique across apps: `<app>/<item>` */
  key: string;
  /** its id within its app's inbox: the approval's id, or `guest:<request id>` */
  item: string;
  kind: DecisionKind;
  app: string;
  appName: string;
  title: string;
  /** one line under the title */
  summary?: string;
  by?: string;
  /** what the list says of who asked (an approval's `requester`), for those who cannot read the people */
  byKnown?: Known;
  at?: string;
  approval?: Approval;
  request?: AccessRequest;
};

const pick = (o: unknown, keys: string[]): string | undefined => {
  if (!o || typeof o !== "object") return undefined;
  for (const k of keys) { const v = (o as Record<string, unknown>)[k]; if (typeof v === "string" && v.trim()) return v.trim(); }
  return undefined;
};

export function fromApproval(a: Approval, app: MyApp | { id: string; name?: string }): Decision {
  const appName = app.name || app.id;
  const base = { key: `${app.id}/${a.id}`, item: a.id, app: app.id, appName, by: a.requested_by, byKnown: a.requester, at: a.created_at, approval: a };
  const p = a.payload || {};
  if (a.kind === "promotion") {
    const release = String(p.release ?? p.short ?? "a release");
    return { ...base, kind: "go-live", title: `Make ${release} live`, summary: typeof p.reason === "string" ? p.reason : "A release waits for an approver before it goes live" };
  }
  if (a.kind === "app_deletion") {
    return { ...base, kind: "deletion", title: `Delete ${appName}`, summary: `${Number(p.versions ?? 0)} releases and all of their data, live data included` };
  }
  if (a.id.startsWith("store:") || a.kind === "store_approval") {
    const where = p.target && p.target !== "live" ? "preview data" : "live data";
    return { ...base, kind: "data", title: (typeof p.message === "string" && p.message) || `A change to ${String(p.store ?? "the data")}`, summary: `${words(String(p.store ?? "store"))} · ${where}` };
  }
  const inner = p.payload ?? p;
  const what = pick(inner, ["title", "summary", "subject", "name", "description", "reason", "short"]);
  return { ...base, kind: "step", title: what || `${words(a.workflow || "workflow")}: ${words(a.step)}`, summary: what ? `${words(a.workflow || "workflow")} · ${words(a.step)}` : undefined };
}

export function fromShare(r: AccessRequest, appName?: string): Decision {
  const roles = (r.roles || []).join(", ");
  return {
    key: `${r.app || ""}/guest:${r.id}`, item: `guest:${r.id}`, kind: "guest", app: r.app || "", appName: appName || r.app || "",
    title: `Let ${r.email} use ${appName || r.app}`, summary: `As ${roles || "a guest"}${r.note ? ` · “${r.note}”` : ""}`,
    by: r.asked_by, at: r.asked_at, request: r,
  };
}

/** The decisions waiting for the person, newest first; `app` keeps one app's. */
export function useDecisions(app?: string) {
  const s = useSession();
  const apps = s.apps.filter((a) => (app ? a.id === app : a.waiting_for_me > 0));
  const shares = !!s.me?.access_waiting;
  const key = s.me ? `decisions|${apps.map((a) => a.id).join(",")}|${shares}` : null;
  const q = useQuery<Decision[]>(key, async () => {
    const lists = await Promise.all(apps.map((a) => api<Approval[]>(`/v1/apps/${a.id}/approvals`).then((xs) => xs.map((x) => fromApproval(x, a))).catch(() => [] as Decision[])));
    const guests = (shares ? await api<AccessRequest[]>("/v1/access-requests?status=awaiting_approval").catch(() => [] as AccessRequest[]) : []).filter((r) => !app || r.app === app);
    const byId = new Map(s.apps.map((a) => [a.id, a.name || a.id]));
    return [...lists.flat(), ...guests.map((r) => fromShare(r, r.app ? byId.get(r.app) : undefined))]
      .sort((x, y) => (y.at || "").localeCompare(x.at || ""));
  });
  return { items: q.data ?? [], ready: q.data !== undefined, loading: q.loading, error: q.error, reload: q.reload };
}

/** Decides one, then refreshes what shows counts and lists. */
export async function decide(d: Decision, approve: boolean, note?: string) {
  if (d.kind === "guest" && d.request) {
    const r = await post<AccessRequest>(`/v1/access-requests/${encodeURIComponent(d.request.id)}/${approve ? "approve" : "refuse"}`, { note: note?.trim() || undefined });
    refreshAfterDecision(d.app);
    return r;
  }
  const r = await post(`/v1/apps/${d.app}/approvals/${encodeURIComponent(d.approval!.id)}`, { approved: approve, escalated: approve, note: note?.trim() || "" });
  refreshAfterDecision(d.app);
  return r;
}

export function refreshAfterDecision(app?: string) {
  // the decision lands asynchronously (a run resumes, a promotion follows): ask again now and a moment later
  const go = () => invalidate("decisions", "/v1/me", "/v1/apps", ...(app ? [`/v1/apps/${app}`] : []), "/v1/access-requests");
  go();
  setTimeout(go, 1200);
}

/** Who asked, in words. */
export const askedBy = (d: Decision) => (d.by ? personName(d.by, d.byKnown) : "");

/** Where a decision opens: the inbox, on it. */
export const decisionHref = (d: Decision) => href("inbox", d.app, d.item);

/** How many decisions wait in each app, counted as the inbox counts them (guests asking for it included); the server's
 *  own count until the list is in. */
export function useWaitingIn() {
  const d = useDecisions();
  const total = (fallback: number) => (d.ready ? d.items.length : fallback);
  const inApp = (a: MyApp) => (d.ready ? d.items.filter((x) => x.app === a.id).length : a.waiting_for_me);
  return { total, inApp };
}

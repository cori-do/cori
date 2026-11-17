// The demo's server: answers the console's /v1 calls in the browser, from src/mock/data.ts, as the dev token's person.
// Changes (a decision, a go-live, a share, a cell) live in memory until the page reloads. Never part of a production build.
import type { AccessRequest, AgentSession, Approval, AuditRow, Deployment, IdentityEvent, LlmProvider, MyApp, PersonRow, RunEvent, Secret, StartState, Version } from "../lib/types";
import { ago, GUEST_REQUESTS, makeApprovals, makeApps, makeRuns, makeStores, MEMBER_ROLES, ORG, PEOPLE, replay, type MApp, type MApproval, type MRun, type MVersion } from "./data";
import { stepsOf, workerSteps } from "./steps";
import { callTool, describe, query, type Caller } from "./stores";

const fresh = typeof location !== "undefined" && /[?&]fresh\b/.test(location.search);
// `?mcp-client`: an identity provider that registers no client for MCP hosts: agents sign in with the deployment's own
const mcpClient = typeof location !== "undefined" && /[?&]mcp-client\b/.test(location.search);
// `?access-message`: a deployment that says how long a change of roles takes (CORI_IDENTITY_ACCESS_MESSAGE)
const accessMessage = typeof location !== "undefined" && /[?&]access-message\b/.test(location.search) ? "Role changes reach people within 10 minutes: our IAM syncs them with the identity provider every 10 minutes." : null;
const apps = makeApps(fresh);
const runs: MRun[] = makeRuns(apps);
const approvals: MApproval[] = fresh ? [] : makeApprovals();
const stores = makeStores();
const shares: AccessRequest[] = fresh ? [] : (GUEST_REQUESTS as unknown as AccessRequest[]);
const people = PEOPLE.filter((p) => !fresh || p.id === "admin" || p.id === "bo");
const requests: AccessRequest[] = [];
const invitations = fresh ? [] : [{ id: "inv_1", email: "jordan.lee@acme.example", roles: ["member"], inviter: "admin", created_at: ago(60 * 30), expires_at: new Date(Date.now() + 6 * 86400_000).toISOString(), grants: [{ app: "support-desk", roles: ["support_agent"] }] }];
const audit: AuditRow[] = [];
const history: Record<string, Deployment[]> = {};
const sessions: AgentSession[] = fresh ? [] : [
  { jti: "as_1", sub: "bo", app: "expense-review", mcp_session: "claude-code", issued_at: ago(40), expires_at: new Date(Date.now() + 29 * 86400_000).toISOString() },
  { jti: "as_2", sub: "bo", app: "team-requests", mcp_session: "claude-code", issued_at: ago(60 * 24 * 3), expires_at: new Date(Date.now() + 27 * 86400_000).toISOString() },
  { jti: "as_3", sub: "admin", app: "support-desk", mcp_session: "cursor", issued_at: ago(60 * 24 * 9), expires_at: ago(60 * 24 * 2) },
];
const providers: LlmProvider[] = [
  { name: "anthropic", kind: "anthropic", base_url: null, enabled: true, has_key: true, updated_at: ago(60 * 24 * 20), updated_by: "admin", models: [
    { id: "claude-haiku-5-5", input_eur_per_mtok: 0.9, output_eur_per_mtok: 4.5 }, { id: "claude-sonnet-5-5", input_eur_per_mtok: 2.8, output_eur_per_mtok: 14 }, { id: "claude-opus-5-5", input_eur_per_mtok: 4.6, output_eur_per_mtok: 23 }] },
];
const tiers: Record<string, { provider: string; model: string } | null> = { small: { provider: "anthropic", model: "claude-haiku-5-5" }, medium: { provider: "anthropic", model: "claude-sonnet-5-5" }, large: null };
const orgSecrets: Secret[] = fresh ? [] : [{ name: "openai.key", kind: "generic", version: 1, key_by: "master", updated_at: ago(60 * 24 * 50), app: null }, { name: "slack.bot_token", kind: "generic", version: 2, key_by: "master", updated_at: ago(60 * 24 * 12), app: null }];
let orgName = ORG.name;

for (const a of apps) {
  history[a.id] = a.versions.flatMap((v) => v.live_periods.map((p) => ({ at: p.from, kind: "promote", hash: v.hash, by: v.approved_by || a.owner, migrations: v.migrations }))).sort((x, y) => y.at.localeCompare(x.at));
}
for (const [i, r] of runs.slice(0, 30).entries()) audit.push({ at: r.started_at, actor: r.user, action: "run.start", object: r.id, details: { app: r.app, workflow: r.workflow, mode: i % 3 ? "endpoint" : "console" } });
audit.push(
  { at: ago(80), actor: "bo", action: "version.promote.requested", object: "team-requests@v4", details: { reason: "it adds a write: requests.reopen" } },
  { at: ago(95), actor: "bo", action: "version.deploy", object: "team-requests@v4", details: { message: "Managers can reopen a request; weekly digest" } },
  { at: ago(38), actor: "bo", action: "version.deploy", object: "expense-review@v1", details: {} },
  { at: ago(60 * 24 * 2 - 45), actor: "noor", action: "version.promote", object: "support-desk@v12", details: { approved: true } },
  { at: ago(60 * 26), actor: "admin", action: "people.invite", object: "jordan.lee@acme.example", details: { roles: ["member"] } },
  { at: ago(60 * 24 * 12), actor: "admin", action: "secret.set", object: "slack.bot_token", details: { version: 2 } },
);
audit.sort((a, b) => b.at.localeCompare(a.at));

// ---- who is asking

type Who = Caller & { admin: boolean; approver: boolean; builder: boolean; guest: boolean; person?: (typeof PEOPLE)[number] };
function who(init: RequestInit): Who {
  const auth = new Headers(init.headers).get("authorization") || "";
  const [, user = "admin", roles = "it_admin"] = auth.replace(/^Bearer\s+/, "").split(":");
  const rs = roles.split(",").filter(Boolean);
  const person = PEOPLE.find((p) => p.id === user);
  const admin = rs.includes("it_admin") || rs.includes("admin");
  const guest = rs.includes("guest");
  return { sub: user, roles: rs, admin, guest, approver: admin || rs.includes("approver"), builder: admin || rs.includes("builder"), builds: false, person };
}
const buildsApp = (w: Who, a: MApp) => !w.guest && (w.admin || a.owner === w.sub || a.builders.includes(w.sub));
/** They reach their apps and nothing else: a guest, or someone who holds no power and builds no app. */
const appsOnly = (w: Who) => w.guest || (!w.admin && !w.builder && !w.approver && !apps.some((a) => buildsApp(w, a)));
/** The roles an app's live stores define. */
const storeRolesOf = (a: MApp) => [...new Set((stores[a.id]?.live || []).flatMap((t) => Object.keys(t.roles)))];
const appRolesOf = (w: Who, a: MApp) => {
  const declared = a.roles.map((r) => r.role);
  const fromToken = w.roles.filter((r) => declared.includes(r));
  const held = Object.entries(a.holders).filter(([, ids]) => ids.includes(w.sub)).map(([r]) => r);
  // a guest holds only what was shared with them; a member, the token's roles and the grants
  return [...new Set(w.guest ? held : [...fromToken, ...held.filter(() => !fromToken.length)])];
};
const caller = (w: Who, a: MApp): Caller => ({ sub: w.sub, roles: appRolesOf(w, a), builds: buildsApp(w, a) });
const routedToMe = (w: Who, a: MApp, x: MApproval) => {
  const roles = appRolesOf(w, a);
  return w.admin || roles.includes(x.to) || (x.to === "approver" && w.approver) || (x.to === "user:initiator" && runs.find((r) => r.id === x.run_id)?.user === w.sub);
};

// ---- releases as a person sees them

const release = (v: MVersion) => `v${v.seq}`;
const short12 = (h: string) => h.replace("sha256:", "").slice(0, 12);
function versionFor(w: Who, a: MApp, v: MVersion): Version {
  const live = a.versions.find((x) => x.hash === a.live);
  const section = v.hash === a.live ? "live" : !live || v.seq > live.seq ? "newer" : "older";
  const wasLive = v.live_periods.length > 0 && v.hash !== a.live;
  const requested = approvals.find((x) => x.app === a.id && x.kind === "promotion" && x.payload.version === v.hash);
  const gated = (v.needs_approval || a.protected) && !wasLive;
  const gate: Version["gate"] = section === "live" ? { state: "live" }
    : requested ? { state: "requested", by: requested.requested_by, reason: String(requested.payload.reason ?? "") }
    : v.approved_by && section === "newer" ? { state: "approved", by: v.approved_by }
    : gated ? { state: "needs_approval", reason: v.reason || (a.protected ? "every release of this app needs an approver" : "it adds a write") }
    : { state: "ready" };
  const go_live: Version["go_live"] = section === "live" ? null
    : gate.state === "requested" ? (w.approver ? "approve" : "pending")
    : gate.state === "needs_approval" ? (w.approver ? "approve" : "request") : "now";
  const previewUrl = `https://${a.id}--v${v.seq}--${ORG.slug}.apps.example/`;
  return {
    release: release(v), seq: v.seq, short: short12(v.hash), message: v.message, section, live: section === "live", was_live: wasLive, live_periods: v.live_periods,
    gate, go_live, pending_migrations: section === "newer" ? v.migrations : [], schema_ahead: [], delta: v.delta ?? null,
    preview: { url: previewUrl, stores: v.stores }, git_branch: v.git_branch, hash: v.hash, status: "deployed", created_by: v.created_by, created_at: v.created_at, preview_url: previewUrl,
    git_commit: v.git_commit, git_dirty: v.git_dirty,
  };
}
const frontUrl = (a: MApp) => (a.live ? `https://${a.id}--${ORG.slug}.apps.example/` : null);
const liveRelease = (a: MApp) => { const v = a.versions.find((x) => x.hash === a.live); return v ? release(v) : null; };

function goLive(a: MApp, v: MVersion, by: string) {
  const prev = a.versions.find((x) => x.hash === a.live);
  if (prev) { const p = prev.live_periods.at(-1); if (p && !p.until) p.until = new Date().toISOString(); }
  v.live_periods.push({ from: new Date().toISOString() });
  v.events.push({ kind: "went_live", by, at: new Date().toISOString() });
  const back = !!prev && v.seq < prev.seq;
  a.live = v.hash;
  history[a.id].unshift({ at: new Date().toISOString(), kind: back ? "rollback" : "promote", hash: v.hash, by, migrations: back ? [] : v.migrations });
  for (let i = approvals.length - 1; i >= 0; i--) if (approvals[i].app === a.id && approvals[i].kind === "promotion" && approvals[i].payload.version === v.hash) approvals.splice(i, 1);
  if (v.delta?.added_workflows?.includes("weekly_digest") && !a.workflows.some((x) => x.id === "weekly_digest")) a.workflows.push({ id: "weekly_digest", schedule: "0 9 * * 1", parameters: [] });
  audit.unshift({ at: new Date().toISOString(), actor: by, action: back ? "version.rollback" : "version.promote", object: `${a.id}@${release(v)}`, details: {} });
}

// ---- answers

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const fail = (status: number, error: string) => json({ error }, status);
const done = (r: Partial<AccessRequest>): AccessRequest => ({ id: `ar_${Math.random().toString(36).slice(2, 9)}`, kind: "grant", event: "", guest: false, asked_by: "", asked_at: new Date().toISOString(), status: "done", updated_at: new Date().toISOString(), ...r }) as AccessRequest;

function myApps(w: Who): MyApp[] {
  return apps.flatMap((a) => {
    const build = buildsApp(w, a);
    const roles = appRolesOf(w, a);
    if (!build && !roles.length) return [];
    return [{
      id: a.id, name: a.name, build, owner: a.owner, roles, promoted: a.live ?? null, promoted_release: liveRelease(a), protected: a.protected, front_url: frontUrl(a),
      // its builders open every store; its users those where they hold a role
      stores: a.live && (build || storeRolesOf(a).some((r) => roles.includes(r))) ? a.stores : [], waiting_for_me: approvals.filter((x) => x.app === a.id && routedToMe(w, a, x)).length, my_runs: runs.filter((r) => r.app === a.id && r.user === w.sub).length,
      description: a.live ? a.description ?? null : null,
    }];
  });
}

function summary(a: MApp) {
  const since = Date.now() - 86400_000;
  const last = a.versions[0];
  return {
    id: a.id, name: a.name, promoted: a.live ?? null, promoted_release: liveRelease(a), owner: a.owner, builders: a.builders, roles: a.roles.map((r) => r.role), created_at: a.created_at,
    protected: a.protected, workflows: a.workflows.length, runs_24h: runs.filter((r) => r.app === a.id && Date.parse(r.started_at) > since).length,
    pending_approvals: approvals.filter((x) => x.app === a.id).length, versions: a.versions.length, front_url: frontUrl(a),
    last_version: last ? { hash: last.hash, status: "deployed", created_at: last.created_at, created_by: last.created_by } : null,
  };
}

function personRow(p: (typeof PEOPLE)[number]): PersonRow {
  return {
    id: p.id, email: p.email, name: p.name, roles: p.roles, guest: p.guest, first_seen: p.first_seen, last_seen: p.last_seen, joined_at: p.first_seen,
    shares: p.guest ? [{ app: "team-requests", roles: ["member"], by: "bo", approved_by: "admin", until: null, status: "done" }] : undefined,
    apps: {
      owns: apps.filter((a) => a.owner === p.id).map((a) => a.id),
      builds: apps.filter((a) => a.builders.includes(p.id)).map((a) => a.id),
      uses: apps.map((a) => ({ app: a.id, roles: Object.entries(a.holders).filter(([, ids]) => ids.includes(p.id)).map(([r]) => r) })).filter((u) => u.roles.length),
    },
  };
}

function startState(w: Who): StartState {
  const any = apps.length > 0;
  return {
    agent: fresh ? { done: false } : { done: true, detail: "Claude Code", by: "bo", first_at: ago(60 * 24 * 21) },
    cli: fresh ? { done: false } : { done: true, detail: "cori/0.4.2", by: "bo" },
    llm: { done: true, own: !fresh, starter: { model: "gemma-4-27b", calls_per_minute: 5, calls_per_day: 200, calls_today: 12, note: null, tiers: fresh ? ["small", "medium", "large"] : ["large"] }, can_add: w.admin },
    app: any ? { done: true, id: apps[0].id } : { done: false },
    release: any ? { done: true, app: apps[0].id, release: liveRelease(apps[0]), preview_url: `https://${apps[0].id}--v1--${ORG.slug}.apps.example/` } : { done: false },
  };
}

function runFor(r: MRun, steps = r.steps) {
  return { workflow: r.workflow, status: r.status, started_at: r.started_at, ended_at: r.ended_at ?? undefined, error: r.error, params: r.params, outputs: r.status === "running" ? undefined : r.outputs, steps: r.status === "running" ? undefined : steps, result: r.result };
}

/** A run started from the console: it goes through its steps over a few seconds (src/mock/steps.ts plays it), and waits
 *  where a person is asked, in their inbox. */
function startRun(a: MApp, wf: string, user: string, params: unknown) {
  const r: MRun = { id: `${wf}-${Math.random().toString(16).slice(2, 18)}${Math.random().toString(16).slice(2, 18)}`, app: a.id, workflow: wf, status: "running", user, started_at: new Date().toISOString(), params, steps: [], n: Math.floor(Math.random() * 997) };
  runs.unshift(r);
  // played at once, shown one step at a time
  const full = replay({ ...r, steps: [] });
  r.played = full.played;
  const end = () => {
    Object.assign(r, full);
    const asked = full.played?.data.decisions.find((d) => d.status === "pending");
    if (asked) approvals.unshift({ id: `${r.id}:${asked.step}`, app: a.id, kind: "decision", run_id: r.id, step: asked.step, to: asked.to, workflow: wf, requested_by: user, created_at: asked.asked_at, payload: { step: asked.step, to: { role: asked.to }, timeout: "7d", payload: asked.shown } });
  };
  if (!full.steps.length) end();
  full.steps.forEach((_, i) => setTimeout(() => { r.steps = full.steps.slice(0, i + 1); if (i === full.steps.length - 1) end(); }, 900 * (i + 1)));
  return r;
}

type Ctx = { w: Who; method: string; path: string; q: URLSearchParams; body: Record<string, unknown> };
type Route = [string, RegExp, (c: Ctx, m: RegExpExecArray) => Response | Promise<Response>];

// the deployment's Google connectors and who connected an account (a person's own, an app's)
const GOOGLE_SCOPES = ["https://www.googleapis.com/auth/gmail.readonly", "https://www.googleapis.com/auth/gmail.compose", "https://www.googleapis.com/auth/drive.readonly", "https://www.googleapis.com/auth/drive.file"];
const linked: Record<string, { account: string; at: string } | undefined> = fresh ? {} : { "app:team-requests": { account: "requests@acme.example", at: ago(60 * 24 * 3) } };
const google = (owner: string) => {
  const c = linked[owner];
  return { provider: "google", label: "Google", connectors: ["gmail", "google-drive"], scopes: GOOGLE_SCOPES, connection: c ? { account: c.account, scopes: GOOGLE_SCOPES, missing: [], connected_at: c.at, since: c.at, used_at: null } : null };
};
const appOf = (id: string) => apps.find((a) => a.id === decodeURIComponent(id));

const routes: Route[] = [
  ["GET", /^\/v1\/auth\/config$/, () => json({ mode: "dev", mcp: "https://cori.example/mcp", ...(mcpClient ? { mcp_client_id: "0oa8f3c1d2e4b5a6c7d8", mcp_callback_port: 33418 } : {}) })],
  ["GET", /^\/v1\/me$/, ({ w }) => json({
    sub: w.sub, roles: w.roles, email: w.person?.email ?? `${w.sub}@acme.example`, persona: w.guest ? "user" : w.admin ? "admin" : w.builder || apps.some((a) => buildsApp(w, a)) ? "builder" : "user", guest: w.guest, apps_only: appsOnly(w),
    organization: { id: ORG.id, slug: ORG.slug, name: orgName }, portal_url: `https://${ORG.slug}.apps.example/`, console_url: "https://cori.example/", providers: [{ name: "google", label: "Google" }],
    can: w.guest ? { people: false } : { admin: w.admin, create_apps: w.builder, build: w.builder || apps.some((a) => buildsApp(w, a)), operate: w.admin, audit: w.admin, usage: w.admin, approve: w.approver, store_admin: w.admin, people: !appsOnly(w) },
    access_waiting: w.admin ? shares.filter((s) => s.status === "awaiting_approval").length : undefined,
  })],
  ["GET", /^\/v1\/me\/apps$/, ({ w }) => json(myApps(w))],
  ["GET", /^\/v1\/me\/organizations$/, ({ w }) => json({
    organizations: [{ id: ORG.id, slug: ORG.slug, name: orgName, current: true }, { id: "org_kestrel", slug: "kestrel", name: "Kestrel Labs" }],
    // the identity provider's word on an organization of their own: its page, for everyone but the admin, who owns Acme
    start: w.admin ? { reason: `You have an organization of your own already: ${orgName}. Switch to it to build apps there.` } : { url: "https://id.cori.example/new-organization" },
  })],
  ["GET", /^\/v1\/start$/, ({ w }) => json(startState(w))],
  ["GET", /^\/v1\/apps$/, ({ w }) => json(apps.filter((a) => buildsApp(w, a)).map(summary))],

  ["GET", /^\/v1\/apps\/([^/]+)$/, ({ w }, m) => {
    const a = appOf(m[1]);
    if (!a) return fail(404, `unknown app \`${m[1]}\``);
    if (buildsApp(w, a)) return json({ ...summary(a), app: a.id, build: true, app_roles: a.roles.map((r) => r.role), stores_configured: true, promoted_workflows: a.live ? a.workflows : [] });
    const roles = appRolesOf(w, a);
    if (!roles.length) return fail(403, `you hold no role in ${a.id}`);
    return json({ id: a.id, app: a.id, name: a.name, promoted: a.live ?? null, promoted_release: liveRelease(a), roles, build: false, front_url: frontUrl(a),
      promoted_workflows: a.live ? a.workflows.filter((x) => x.endpoint && (x.endpoint.allow?.roles || []).some((r) => roles.includes(r))) : [] });
  }],
  ["DELETE", /^\/v1\/apps\/([^/]+)$/, ({ w }, m) => {
    const a = appOf(m[1])!;
    if (a.protected) return fail(409, "every release of this app needs an approver: relax that first");
    if (w.approver) { apps.splice(apps.indexOf(a), 1); return json({ deleted: true }); }
    approvals.push({ id: `delete:${a.id}`, app: a.id, kind: "app_deletion", run_id: "", step: "delete", to: "approver", workflow: "", requested_by: w.sub, created_at: new Date().toISOString(), payload: { kind: "app_deletion", versions: a.versions.length, requested_by: w.sub } });
    return json({ status: "pending_approval", approver_roles: ["approver"] });
  }],
  ["GET", /^\/v1\/apps\/([^/]+)\/versions$/, ({ w }, m) => { const a = appOf(m[1])!; return json(a.versions.map((v) => versionFor(w, a, v))); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/versions\/([^/]+)$/, ({ w }, m) => {
    const a = appOf(m[1])!, v = a.versions.find((x) => x.hash === decodeURIComponent(m[2]));
    if (!v) return fail(404, "unknown version");
    const ver = versionFor(w, a, v);
    return json({ release: ver.release, events: v.events, review: { needs_approval: ver.gate.state === "needs_approval", delta: v.delta, workflows: a.workflows.map((x) => ({ id: x.id, steps: [] })) } });
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/versions\/([^/]+)\/promote$/, ({ w }, m) => {
    const a = appOf(m[1])!, v = a.versions.find((x) => x.hash === decodeURIComponent(m[2]))!;
    const ver = versionFor(w, a, v);
    if (ver.go_live === "request") {
      approvals.push({ id: `promote:${a.id}:${v.hash}`, app: a.id, kind: "promotion", run_id: "", step: "go-live", to: "approver", workflow: "", requested_by: w.sub, created_at: new Date().toISOString(),
        payload: { kind: "promotion", release: release(v), short: short12(v.hash), version: v.hash, requested_by: w.sub, reason: ver.gate.reason, delta: { ...(v.delta || {}), migrations: v.migrations } } });
      v.events.push({ kind: "go_live_requested", by: w.sub, at: new Date().toISOString() });
      return json({ status: "pending_approval", approver_roles: ["approver"] });
    }
    if (ver.go_live === "pending") return fail(409, "an approver decides it already");
    goLive(a, v, w.sub);
    return json({ status: "promoted", release: release(v) });
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/versions\/([^/]+)\/reseed$/, () => json({ ok: true })],
  ["DELETE", /^\/v1\/apps\/([^/]+)\/versions\/([^/?]+)/, (_, m) => { const a = appOf(m[1])!; a.versions = a.versions.filter((x) => x.hash !== decodeURIComponent(m[2])); return json({ deleted: true }); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/promotions$/, (_, m) => json(history[decodeURIComponent(m[1])] || [])],
  ["POST", /^\/v1\/apps\/([^/]+)\/protect$/, ({ body }, m) => { appOf(m[1])!.protected = body.protected !== false; return json({ ok: true }); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/meters$/, (_, m) => {
    const a = appOf(m[1])!;
    const byStatus: Record<string, number> = {};
    runs.filter((r) => r.app === a.id).forEach((r) => { byStatus[r.status] = (byStatus[r.status] || 0) + 1; });
    const month = { app: a.id, month: new Date().toISOString().slice(0, 7), calls: a.llm.calls, tokens_in: a.llm.tokens_in, tokens_out: a.llm.tokens_out, spent_eur: a.llm.spent, budget_eur: a.llm.budget, remaining_eur: a.llm.budget == null ? null : Math.max(0, a.llm.budget - a.llm.spent), blocked: a.llm.budget != null && a.llm.spent >= a.llm.budget };
    return json({ runs: byStatus, llm: { calls: a.llm.calls, tokens_in: a.llm.tokens_in, tokens_out: a.llm.tokens_out, cost_eur: a.llm.spent, month } });
  }],
  ["PUT", /^\/v1\/apps\/([^/]+)\/llm\/budget$/, ({ body }, m) => { appOf(m[1])!.llm.budget = body.monthly_eur as number | null; return json({ ok: true }); }],

  ["GET", /^\/v1\/apps\/([^/]+)\/runs$/, ({ w }, m) => {
    const a = appOf(m[1])!;
    return json(runs.filter((r) => r.app === a.id && (buildsApp(w, a) || r.user === w.sub)).slice(0, 100).map((r) => ({ run_id: r.id, workflow: r.workflow, status: r.status, user: r.user, started_at: r.started_at, ended_at: r.ended_at, parent_run: r.parent_run ?? null })));
  }],
  ["GET", /^\/v1\/apps\/([^/]+)\/runs\/([^/]+)$/, (_, m) => { const r = runs.find((x) => x.id === decodeURIComponent(m[2])); return r ? json(runFor(r)) : fail(404, "unknown run"); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/runs\/([^/]+)\/events$/, (_, m) => {
    const r = runs.find((x) => x.id === decodeURIComponent(m[2]));
    return json(workerSteps(r?.steps || []).map((s): RunEvent => ({ kind: "step", payload: s })));
  }],
  ["GET", /^\/v1\/apps\/([^/]+)\/runs\/([^/]+)\/steps$/, ({ w }, m) => {
    const a = appOf(m[1])!, r = runs.find((x) => x.id === decodeURIComponent(m[2]));
    if (!r) return fail(404, "unknown run");
    if (r.user !== w.sub && !buildsApp(w, a)) return fail(403, "not your run");
    // as far as the run went: the steps it reached, the person it waits for (with what their inbox shows them)
    const reached = new Set(r.steps.map((s) => s.name));
    const d = r.played?.data;
    const waits = (step: string) => approvals.find((x) => x.run_id === r.id && x.step === step);
    const data = d && {
      ...d, attempts: Object.fromEntries(Object.entries(d.attempts).filter(([k]) => reached.has(k))), model_calls: (d.model_calls || []).filter((c) => reached.has(c.step)),
      decisions: d.decisions.filter((x) => x.status !== "pending" || waits(x.step)).map((x) => (x.status === "pending" ? { ...x, shown: waits(x.step)!.payload.payload ?? waits(x.step)!.payload.fields ?? x.shown } : x)),
    };
    return json(stepsOf(r.workflow, liveRelease(a) ?? null, data, buildsApp(w, a)));
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/wf\/([^/]+)\/runs$/, ({ w, body }, m) => { const r = startRun(appOf(m[1])!, decodeURIComponent(m[2]), w.sub, body); return json({ run_id: r.id, status: "running" }); }],

  ["GET", /^\/v1\/apps\/([^/]+)\/approvals$/, ({ w }, m) => {
    const a = appOf(m[1])!;
    const requester = (id: string) => { const p = PEOPLE.find((x) => x.id === id); return p ? { name: p.name, email: p.email } : undefined; };
    return json(approvals.filter((x) => x.app === a.id && routedToMe(w, a, x)).map((x): Approval => ({ id: x.id, kind: x.kind, status: "pending", workflow: x.workflow, step: x.step, to: x.to, requested_by: x.requested_by, requester: requester(x.requested_by), run_id: x.run_id, created_at: x.created_at, payload: x.payload as Approval["payload"] })));
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/approvals\/(.+)$/, ({ w, body }, m) => {
    const a = appOf(m[1])!, id = decodeURIComponent(m[2]);
    const x = approvals.find((y) => y.app === a.id && y.id === id);
    if (!x) return fail(404, "no pending approval with that id");
    if (x.requested_by === w.sub && x.kind !== "decision") return fail(403, "four eyes: you asked for it; another person decides it");
    approvals.splice(approvals.indexOf(x), 1);
    const approved = body.approved === true;
    audit.unshift({ at: new Date().toISOString(), actor: w.sub, action: `approval.${approved ? "approve" : "decline"}`, object: id, details: { note: body.note || null } });
    if (x.kind === "promotion" && approved) { const v = a.versions.find((y) => y.hash === x.payload.version)!; v.approved_by = w.sub; v.events.push({ kind: "approved", by: w.sub, at: new Date().toISOString() }); goLive(a, v, w.sub); }
    if (x.kind === "app_deletion" && approved) apps.splice(apps.indexOf(a), 1);
    const run = runs.find((r) => r.id === x.run_id);
    if (run && run.status === "running") {
      // the run goes on from the answer (src/mock/steps.ts plays it again)
      setTimeout(() => replay(run, { ...(run.answers || {}), [x.step]: { approved, note: typeof body.note === "string" ? body.note : "", by: w.sub, at: new Date().toISOString() } }), 600);
    }
    return json({ ok: true, approved });
  }],

  ["GET", /^\/v1\/apps\/([^/]+)\/stores$/, ({ w }, m) => {
    const a = appOf(m[1])!;
    // one of its users: the live stores where they hold a role, by name, with their roles there
    if (!buildsApp(w, a) && !w.approver) {
      const theirs = storeRolesOf(a).filter((r) => appRolesOf(w, a).includes(r));
      return json(a.live && theirs.length ? a.stores.map((s) => ({ app: a.id, name: s, target: "live", roles: theirs, updated_at: ago(60 * 24 * 2) })) : []);
    }
    const out = Object.keys(stores[a.id] || {}).flatMap((target) => (stores[a.id][target] || []).length ? a.stores.map((s) => ({ app: a.id, name: s, target, database: `cori_store_t4_${a.id.replace(/-/g, "_")}_${s}${target === "live" ? "" : "_" + short12(target).slice(0, 7)}`, roles: a.roles.map((r) => r.role), seeded: target !== "live", updated_at: ago(target === "live" ? 60 * 24 * 2 : 40) })) : []);
    return json(out.filter((s) => s.target !== "live" || a.live));
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/(?:versions\/([^/]+)\/)?stores\/([^/]+)\/mcp$/, ({ w, body }, m) => {
    const a = appOf(m[1])!, target = m[2] ? decodeURIComponent(m[2]) : "live";
    const key = Object.keys(stores[a.id] || {}).find((t) => t === target || short12(t).startsWith(target)) ?? target;
    const tables = stores[a.id]?.[key] || [];
    const c = caller(w, a);
    const params = (body.params || {}) as Record<string, unknown>;
    try {
      if (body.method === "cori/describe") return json({ jsonrpc: "2.0", id: 1, result: describe(m[3], tables, c) });
      if (body.method === "tools/call") {
        const args = (params.arguments || {}) as Record<string, unknown>;
        const out = params.name === "query" ? { rows: query(tables, c, String(args.sql), (args.params as unknown[]) || [], Number(args.max_rows) || 500) }
          : callTool(tables, c, key === "live", String(params.name), args, (p) => {
            const id = `store:${a.id}:live:${m[3]}:${Math.random().toString(16).slice(2, 6)}`;
            approvals.unshift({ id, app: a.id, kind: "store_approval", run_id: "", step: "change", to: "support_lead", workflow: "", requested_by: w.sub, created_at: new Date().toISOString(),
              payload: { kind: "store_approval", store: m[3], target: "live", role: p.role, message: `Change ${Object.keys(p.changes).join(", ")} of ${p.table} ${String(Object.values(p.row)[0])}`, fields: p.changes, requested_by: w.sub } });
            return id;
          });
        return json({ jsonrpc: "2.0", id: 1, result: { structuredContent: out, content: [{ type: "text", text: JSON.stringify(out) }] } });
      }
      return json({ jsonrpc: "2.0", id: 1, error: { message: `unknown method ${body.method}` } });
    } catch (e) { return json({ jsonrpc: "2.0", id: 1, result: { isError: true, content: [{ text: e instanceof Error ? e.message : String(e) }] } }); }
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/stores\/reset$/, (_, m) => json({ live: appOf(m[1])!.stores.map((s) => ({ store: s, seeds_applied: ["seed.sql"] })) })],

  ["GET", /^\/v1\/apps\/([^/]+)\/access$/, ({ w }, m) => {
    const a = appOf(m[1])!;
    const mayGrant = w.admin || a.owner === w.sub;
    return json({
      app: a.id, client_id: `${a.id}--${ORG.slug}`, owner: a.owner, builders: a.builders, source: "directory",
      roles: a.roles.map((r) => ({ role: r.role, description: r.description, declared: true, holders: (a.holders[r.role] || []).map((id) => { const p = PEOPLE.find((x) => x.id === id); return { id, email: p?.email, name: p?.name, guest: p?.guest }; }) })),
      requests: [...requests.filter((r) => r.app === a.id), ...shares.filter((s) => s.app === a.id)], can: { grant: mayGrant, revoke: mayGrant, invite: w.admin }, owner_left: false,
      grants_here: true, message: accessMessage,
    });
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/access$/, ({ w, body }, m) => {
    const a = appOf(m[1])!;
    const role = (body.roles as string[])[0];
    if (body.email) {
      const r = done({ kind: "share", email: String(body.email), app: a.id, roles: [role], guest: true, until: (body.until as string) || null, note: (body.note as string) || null, asked_by: w.sub, status: w.admin ? "done" : "awaiting_approval" });
      if (w.admin) requests.unshift(r); else shares.unshift(r);
      return json(r);
    }
    const person = String(body.person);
    a.holders[role] ||= [];
    if (body.granted) { if (!a.holders[role].includes(person)) a.holders[role].push(person); } else a.holders[role] = a.holders[role].filter((x) => x !== person);
    const r = done({ kind: body.granted ? "grant" : "revoke", person, app: a.id, roles: [role], asked_by: w.sub });
    requests.unshift(r);
    return json(r);
  }],
  ["PUT", /^\/v1\/apps\/([^/]+)\/builders$/, ({ body }, m) => {
    const a = appOf(m[1])!;
    if (body.owner) a.owner = String(body.owner);
    a.builders = ((body.builders as string[]) || []).filter((b) => b !== a.owner);
    return json({ app: a.id, owner: a.owner, builders: a.builders });
  }],
  ["GET", /^\/v1\/apps\/([^/]+)\/identity$/, (_, m) => {
    const a = appOf(m[1])!;
    return json({ hook: true, role_events: true, roles_at: "promote", app: a.id, client_id: a.client_status === "ready" ? `${a.id}--${ORG.slug}` : null, client_status: a.client_status, client_note: a.client_status === "pending" ? "Waiting for the identity team (ticket IAM-2291)" : null,
      roles: a.roles.map((r, i) => ({ role: r.role, description: r.description, state: a.client_status === "pending" || (a.id === "team-requests" && i === 1 && false) ? "pending" : "confirmed", declared_at: a.created_at })),
      events: identityEvents.filter((e) => e.app === a.id), may_sync: true, admin_url: null });
  }],
  ["POST", /^\/v1\/apps\/([^/]+)\/oidc-client$/, (_, m) => { const a = appOf(m[1])!; return json({ client_status: a.client_status, client_id: a.client_status === "ready" ? `${a.id}--${ORG.slug}` : null, note: a.client_status === "pending" ? "Waiting for the identity team" : null }); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/schedules$/, () => json([])],

  ["GET", /^\/v1\/people$/, ({ w }) => json({
    people: people.filter((p) => !w.guest || p.id === w.sub).map(personRow), source: "directory",
    can: w.admin ? { invite: true, set_roles: true, remove: true, cancel_invitations: true, resend_invitations: true, grant: true, revoke: true, rename: true, leave: true } : { leave: !w.guest },
    member_roles: MEMBER_ROLES, requests: requests.filter((r) => ["invite", "roles", "remove"].includes(r.kind)),
  })],
  ["PUT", /^\/v1\/people\/([^/]+)\/roles$/, ({ w, body }, m) => { const p = PEOPLE.find((x) => x.id === decodeURIComponent(m[1]))!; p.roles = body.roles as string[]; const r = done({ kind: "roles", person: p.id, roles: p.roles, asked_by: w.sub }); requests.unshift(r); return json(r); }],
  ["GET", /^\/v1\/people\/([^/]+)\/leaving$/, (_, m) => { const id = decodeURIComponent(m[1]); return json({ owns: apps.filter((a) => a.owner === id).map((a) => a.id), builds: apps.filter((a) => a.builders.includes(id)).map((a) => a.id), agent_sessions: sessions.filter((s) => s.sub === id && !s.revoked_at).length, waiting: [] }); }],
  ["DELETE", /^\/v1\/people\/([^/]+)$/, ({ w }, m) => { const id = decodeURIComponent(m[1]); const i = people.findIndex((p) => p.id === id); if (i >= 0) people.splice(i, 1); const r = done({ kind: "remove", person: id, asked_by: w.sub }); requests.unshift(r); return json(r); }],
  ["GET", /^\/v1\/invitations$/, () => json({ invitations, requests: requests.filter((r) => r.kind === "invite") })],
  ["POST", /^\/v1\/invitations$/, ({ w, body }) => {
    invitations.unshift({ id: `inv_${Math.random().toString(36).slice(2, 8)}`, email: String(body.email), roles: body.roles as string[], inviter: w.sub, created_at: new Date().toISOString(), expires_at: new Date(Date.now() + 7 * 86400_000).toISOString(), grants: (body.grants as { app: string; roles: string[] }[]) || [] });
    return json(done({ kind: "invite", email: String(body.email), roles: body.roles as string[], asked_by: w.sub }));
  }],
  ["DELETE", /^\/v1\/invitations\/([^/]+)$/, ({ w }, m) => { const i = invitations.findIndex((x) => x.id === decodeURIComponent(m[1])); if (i >= 0) invitations.splice(i, 1); return json(done({ kind: "cancel_invitation", asked_by: w.sub })); }],
  ["POST", /^\/v1\/invitations\/([^/]+)\/resend$/, ({ w }) => json(done({ kind: "resend_invitation", asked_by: w.sub }))],
  ["GET", /^\/v1\/access-requests$/, ({ q }) => json(shares.filter((s) => !q.get("status") || s.status === q.get("status")))],
  ["POST", /^\/v1\/access-requests\/([^/]+)\/(approve|refuse)$/, ({ w }, m) => {
    const s = shares.find((x) => x.id === decodeURIComponent(m[1]));
    if (!s) return fail(404, "no such request");
    s.status = m[2] === "approve" ? "done" : "refused"; s.approved_by = w.sub; s.updated_at = new Date().toISOString();
    return json(s);
  }],
  ["DELETE", /^\/v1\/access-requests\/([^/]+)$/, (_, m) => { const s = shares.find((x) => x.id === decodeURIComponent(m[1])); if (s) s.status = "cancelled"; return json(s || {}); }],
  ["GET", /^\/v1\/organization$/, ({ w }) => json({ id: ORG.id, slug: ORG.slug, name: orgName, roles: MEMBER_ROLES, can: w.admin ? { rename: true, export: true, leave: true, delete: true } : { leave: !w.guest } })],
  ["PATCH", /^\/v1\/organization$/, ({ w, body }) => { if (body.name) orgName = String(body.name); return json(done({ kind: "organization", asked_by: w.sub })); }],
  ["POST", /^\/v1\/organization\/leave$/, ({ w }) => json(done({ kind: "leave", person: w.sub, asked_by: w.sub, status: "pending" }))],
  ["GET", /^\/v1\/export$/, () => new Response(new Blob(["demo export"]), { headers: { "content-disposition": 'attachment; filename="acme-export.tar.zst"' } })],

  ["GET", /^\/v1\/llm$/, ({ w }) => json({
    providers, tiers: Object.fromEntries(Object.entries(tiers).filter(([, v]) => v)), tier_names: ["small", "medium", "large"], kinds: ["openai", "anthropic"], can_edit: w.admin,
    starter: { model: "gemma-4-27b", calls_per_minute: 5, calls_per_day: 200, calls_today: 12, note: "Served by third-party providers that may keep prompts. Use a provider of your own for confidential data.", tiers: Object.entries(tiers).filter(([, v]) => !v).map(([k]) => k) },
    apps: apps.map((a) => ({ app: a.id, month: new Date().toISOString().slice(0, 7), calls: a.llm.calls, tokens_in: a.llm.tokens_in, tokens_out: a.llm.tokens_out, spent_eur: a.llm.spent, budget_eur: a.llm.budget, remaining_eur: a.llm.budget == null ? null : Math.max(0, a.llm.budget - a.llm.spent), blocked: a.llm.budget != null && a.llm.spent >= a.llm.budget })),
  })],
  ["PUT", /^\/v1\/llm\/providers\/([^/]+)$/, ({ w, body }, m) => {
    const name = decodeURIComponent(m[1]);
    const p = providers.find((x) => x.name === name);
    const next = { name, kind: String(body.kind), base_url: (body.base_url as string) || null, enabled: body.enabled !== false, has_key: !!body.api_key || !!p?.has_key, updated_at: new Date().toISOString(), updated_by: w.sub, models: (body.models as LlmProvider["models"]) || [] };
    if (p) Object.assign(p, next); else providers.push(next);
    return json({ ok: true });
  }],
  ["DELETE", /^\/v1\/llm\/providers\/([^/]+)$/, (_, m) => { const i = providers.findIndex((x) => x.name === decodeURIComponent(m[1])); if (i >= 0) providers.splice(i, 1); return json({ ok: true }); }],
  ["GET", /^\/v1\/llm\/providers\/([^/]+)\/models$/, () => json({ models: ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5", "claude-fable-5-1"] })],
  ["PUT", /^\/v1\/llm\/tiers$/, ({ body }) => { for (const [k, v] of Object.entries(body)) tiers[k] = v as typeof tiers[string]; return json({ ok: true }); }],

  ["GET", /^\/v1\/connectors$/, () => json(fresh ? [] : [
    { name: "slack", kind: "mcp", description: "Post messages and read channels", tools: [{ name: "post_message" }, { name: "list_channels" }, { name: "read_thread" }], roles: [], transport: { url: "https://mcp.slack.com/mcp" }, declared_by: "admin", declared_at: ago(60 * 24 * 60) },
    { name: "stripe", kind: "mcp", description: "Refunds and customers", tools: [{ name: "create_refund" }, { name: "get_customer" }], roles: ["support_lead"], transport: { url: "https://mcp.stripe.com" }, declared_by: "admin", declared_at: ago(60 * 24 * 41) },
    { name: "zendesk", kind: "http", description: "Tickets", tools: [], roles: [], transport: { url: "https://acme.zendesk.com/api/v2" }, declared_by: "bo", declared_at: ago(60 * 24 * 30) },
    { name: "gws", kind: "cli", description: "Google Workspace: accounts and groups", tools: [{ name: "gws" }], roles: ["hr"], transport: { command: ["gws", "--json"] }, declared_by: "admin", declared_at: ago(60 * 24 * 90) },
    { name: "gmail", kind: "mcp", description: "Gmail: search, read and draft in each person's own mailbox", tools: [{ name: "search_threads" }, { name: "get_thread" }, { name: "create_draft" }], roles: [], transport: { url: "https://gmailmcp.googleapis.com/mcp/v1" }, provided: "deployment", oauth: { provider: "google", label: "Google", scopes: GOOGLE_SCOPES.slice(0, 2) } },
    { name: "google-drive", kind: "mcp", description: "Google Drive: search and read each person's files", tools: [{ name: "search_files" }, { name: "read_file_content" }], roles: [], transport: { url: "https://drivemcp.googleapis.com/mcp/v1" }, provided: "deployment", oauth: { provider: "google", label: "Google", scopes: GOOGLE_SCOPES.slice(2) } },
  ])],
  ["GET", /^\/v1\/connections$/, ({ w }) => json({ providers: [google(`person:${w.sub}`)] })],
  ["POST", /^\/v1\/connections\/google$/, ({ w }) => { linked[`person:${w.sub}`] = { account: w.person?.email ?? `${w.sub}@acme.example`, at: new Date().toISOString() }; return json({ url: `${location.pathname}${location.search}#/connections?connected=google` }); }],
  ["DELETE", /^\/v1\/connections\/google$/, ({ w }) => { delete linked[`person:${w.sub}`]; return json({ disconnected: "google" }); }],
  ["GET", /^\/v1\/apps\/([^/]+)\/connections$/, (_, m) => json({ app: m[1], providers: [google(`app:${m[1]}`)] })],
  ["POST", /^\/v1\/apps\/([^/]+)\/connections\/google$/, (_, m) => { linked[`app:${m[1]}`] = { account: `ops@${ORG.slug}.example`, at: new Date().toISOString() }; return json({ url: `${location.pathname}${location.search}#/app/${m[1]}/settings?connected=google` }); }],
  ["DELETE", /^\/v1\/apps\/([^/]+)\/connections\/google$/, (_, m) => { delete linked[`app:${m[1]}`]; return json({ disconnected: "google" }); }],
  ["GET", /^\/v1\/connector-requests$/, () => json(fresh ? [] : [{ at: ago(60 * 7), url: "https://mcp.notion.com/mcp", requested_by: "bo", reason: "Expense Review should file receipts in the finance wiki" }])],
  ["GET", /^\/v1\/secrets$/, ({ q }) => json(q.get("app") ? (appOf(q.get("app")!)?.secrets || []) : [...orgSecrets, ...apps.flatMap((a) => a.secrets)])],
  ["POST", /^\/v1\/secrets$/, ({ body }) => {
    const a = body.app ? appOf(String(body.app)) : undefined;
    const list = a ? a.secrets : orgSecrets;
    const s = list.find((x) => x.name === body.name);
    if (s) { s.version++; s.updated_at = new Date().toISOString(); } else list.unshift({ name: String(body.name), app: a?.id ?? null, kind: "generic", version: 1, key_by: "master", updated_at: new Date().toISOString() });
    return json({ ok: true });
  }],
  ["GET", /^\/v1\/doctor$/, () => json({ checks: [
    { ok: true, name: "Platform database", detail: "Postgres 17.4 · 12 ms" }, { ok: true, name: "Workflow engine", detail: "Temporal 1.27 · namespace cori" },
    { ok: true, name: "App databases", detail: "store cluster main · 9 databases" }, { ok: true, name: "Sandboxes", detail: "8 of 8 ready (gVisor)" },
    { ok: false, name: "Mail", detail: "SES is in sandbox mode: mail goes to verified addresses only" }, { ok: true, name: "Bundles", detail: "s3://cori-prod-bundles" },
  ] })],
  ["GET", /^\/v1\/identity\/catalog$/, () => json({ hook: true, role_events: true, roles_at: "promote", apps: apps.map((a) => ({ app: a.id, name: a.name, owner: a.owner, client_id: a.client_status === "ready" ? `${a.id}--${ORG.slug}` : null, client_status: a.client_status, client_note: a.client_status === "pending" ? "Waiting for the identity team (ticket IAM-2291)" : null, roles: a.roles.map((r) => ({ role: r.role, state: a.client_status === "pending" ? "pending" : "confirmed", declared_at: a.created_at })) })) })],
  ["GET", /^\/v1\/identity\/events$/, ({ q }) => json(identityEvents.filter((e) => !q.get("status") || e.status === q.get("status")))],
  ["POST", /^\/v1\/identity\/events\/([^/]+)\/retry$/, (_, m) => { const e = identityEvents.find((x) => x.id === m[1]); if (e) { e.status = "done"; e.attempts++; e.last_status = 200; e.last_answer = "ok"; } return json({ status: "done" }); }],
  ["GET", /^\/v1\/stores$/, () => json(apps.flatMap((a) => Object.keys(stores[a.id] || {}).flatMap((t) => a.stores.map((s) => ({ app: a.id, name: s, target: t, database: `cori_store_t4_${a.id.replace(/-/g, "_")}_${s}`, roles: a.roles.map((r) => r.role), updated_at: ago(90) })))))],
  ["GET", /^\/v1\/pools$/, () => json(fresh ? [] : [{ name: "acme-onprem", last_seen: ago(0.5), capabilities: { connectors: ["gws"], os: "linux" } }])],
  ["GET", /^\/v1\/audit$/, () => json(audit)],
  ["GET", /^\/v1\/agent-sessions$/, ({ w }) => json(sessions.filter((s) => w.admin || s.sub === w.sub))],
  ["DELETE", /^\/v1\/agent-sessions\/([^/]+)$/, (_, m) => { const s = sessions.find((x) => x.jti === decodeURIComponent(m[1])); if (s) s.revoked_at = new Date().toISOString(); return json({ revoked: m[1] }); }],
];

const identityEvents: IdentityEvent[] = fresh ? [] : [
  { id: "ev_3", seq: 3, app: "expense-review", event: "client.requested", body: { app: "expense-review" }, status: "queued", attempts: 4, next_at: new Date(Date.now() + 8 * 60_000).toISOString(), occurred_at: ago(60 * 5), last_status: 202, last_answer: "accepted: waiting for the identity team" },
  { id: "ev_2", seq: 2, app: "team-requests", event: "role.declared", body: { role: "manager" }, status: "done", attempts: 1, next_at: ago(60 * 24 * 12), occurred_at: ago(60 * 24 * 12), delivered_at: ago(60 * 24 * 12), last_status: 200, last_answer: "ok" },
  { id: "ev_1", seq: 1, app: "", event: "invitation.created", body: { email: "jordan.lee@acme.example" }, status: "done", attempts: 1, next_at: ago(60 * 30), occurred_at: ago(60 * 30), delivered_at: ago(60 * 30), last_status: 200, last_answer: "ok" },
];


/** What a guest reaches (their organizations, their apps' runtime, data and approvals), and besides, for a member who holds
 *  no power and builds no app, leaving this organization: the server's `GUEST_ROUTES` and `MEMBER_APP_ROUTES`. */
const GUEST_ROUTES: [string, RegExp][] = [
  ["GET", /^\/v1\/auth\/config$/], ["GET", /^\/v1\/me$/], ["GET", /^\/v1\/me\/apps$/], ["GET", /^\/v1\/me\/organizations$/], ["GET", /^\/v1\/apps\/[^/]+$/], ["GET", /^\/v1\/apps\/[^/]+\/me$/],
  ["POST", /^\/v1\/apps\/[^/]+\/wf\/[^/]+\/runs$/], ["GET", /^\/v1\/apps\/[^/]+\/runs(\/[^/]+(\/(events|stream|steps))?)?$/], ["POST", /^\/v1\/apps\/[^/]+\/runs\/[^/]+\/decisions\/[^/]+$/],
  ["GET", /^\/v1\/apps\/[^/]+\/approvals$/], ["POST", /^\/v1\/apps\/[^/]+\/approvals\/.+$/], ["GET", /^\/v1\/apps\/[^/]+\/stores$/], ["POST", /^\/v1\/apps\/[^/]+\/stores\/[^/]+\/mcp$/],
  ["POST", /^\/v1\/portal-codes$/], ["POST", /^\/v1\/apps\/[^/]+\/pass-codes$/], ["GET", /^\/v1\/connections$/], ["POST", /^\/v1\/connections\/[^/]+$/], ["DELETE", /^\/v1\/connections\/[^/]+$/],
];
const MEMBER_APP_ROUTES: [string, RegExp][] = [["POST", /^\/v1\/organization\/leave$/]];

export async function handle(path: string, init: RequestInit): Promise<Response> {
  const url = new URL(path, "http://demo");
  const method = (init.method || "GET").toUpperCase();
  const body = (() => { try { return init.body ? JSON.parse(String(init.body)) : {}; } catch { return {}; } })();
  const w = who(init);
  // a server takes a moment: so does the demo, to show what loading looks like
  await new Promise((r) => setTimeout(r, 90 + Math.random() * 160));
  // who reaches their apps only is refused the rest, as the server's guard does
  const reach = w.guest ? GUEST_ROUTES : [...GUEST_ROUTES, ...MEMBER_APP_ROUTES];
  if (appsOnly(w) && !reach.some(([m, re]) => m === method && re.test(url.pathname))) {
    return fail(403, w.guest ? `a guest reaches the apps shared with them: \`${method} ${url.pathname}\` is not for guests` : `your roles open the organization's apps: \`${method} ${url.pathname}\` is for those who build them or administer it`);
  }
  for (const [m, re, fn] of routes) {
    if (m !== method) continue;
    const match = re.exec(url.pathname);
    if (match) {
      try { return await fn({ w, method, path: url.pathname, q: url.searchParams, body }, match); }
      catch (e) { return fail(500, e instanceof Error ? e.message : String(e)); }
    }
  }
  return fail(404, `the demo has no answer for ${method} ${url.pathname}`);
}

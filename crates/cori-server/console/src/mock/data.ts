// The demo organization the mock server answers from (`npm run mock`, `npm run build:demo`): fictional people and apps,
// with times relative to now. `?fresh` in the address starts from an organization with no app yet.
import type { PromotedWorkflow, ReleaseEvent, Secret, Step } from "../lib/types";
import { play, PLANS, type Answer, type Played } from "./steps";

export const ago = (min: number) => new Date(Date.now() - min * 60_000).toISOString();
export const later = (min: number) => new Date(Date.now() + min * 60_000).toISOString();
const hex = (seed: string, n = 64) => {
  let h = 2166136261;
  for (const ch of seed) h = Math.imul(h ^ ch.charCodeAt(0), 16777619) >>> 0;
  let out = "";
  for (let i = 0; out.length < n; i++) { h = Math.imul(h ^ (i + 0x9e3779b9), 16777619) >>> 0; h = (h ^ (h >>> 13)) >>> 0; out += h.toString(16).padStart(8, "0"); }
  return out.slice(0, n);
};
export const hash = (seed: string) => `sha256:${hex(seed)}`;

/** A small seeded random generator, so the demo looks the same on every load. */
export function rng(seed: number) { let s = seed >>> 0; return () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 2 ** 32; }; }

export type MPerson = { id: string; name: string; email: string; roles: string[]; guest?: boolean; last_seen?: string; first_seen?: string };
export type MVersion = {
  hash: string; seq: number; message: string; created_by: string; created_at: string; git_branch?: string; git_commit?: string; git_dirty?: boolean;
  needs_approval?: boolean; reason?: string; delta?: { new_or_changed_writes?: string[]; added_workflows?: string[]; removed_workflows?: string[] };
  migrations: string[]; live_periods: { from: string; until?: string | null }[]; approved_by?: string; events: ReleaseEvent[]; stores: string[];
};
export type MRun = {
  id: string; app: string; workflow: string; status: string; user: string; started_at: string; ended_at?: string | null; params: unknown; steps: Step[]; result?: { headline: string; fields?: { label: string; value: unknown }[] }; error?: string; outputs?: unknown; parent_run?: string | null;
  /** how the run plays out (src/mock/steps.ts): its seed, the step that fails, what the people asked answered, the result */
  n?: number; failAt?: string; answers?: Record<string, Answer>; played?: Played;
};
export type MApproval = { id: string; app: string; kind: string; run_id: string; step: string; to: string; payload: Record<string, unknown>; created_at: string; requested_by: string; workflow: string };
export type MApp = {
  id: string; name: string; owner: string; builders: string[]; protected: boolean; created_at: string; description?: string;
  roles: { role: string; description: string }[]; holders: Record<string, string[]>;
  workflows: PromotedWorkflow[]; versions: MVersion[]; live?: string; stores: string[];
  llm: { calls: number; tokens_in: number; tokens_out: number; spent: number; budget: number | null };
  secrets: Secret[]; client_status: "ready" | "pending";
};

export const ORG = { id: "org_acme", slug: "acme", name: "Acme" };

export const PEOPLE: MPerson[] = [
  { id: "admin", name: "Camille Laurent", email: "camille@acme.example", roles: ["admin"], last_seen: ago(2), first_seen: ago(60 * 24 * 90) },
  { id: "bo", name: "Bo Chen", email: "bo@acme.example", roles: ["builder"], last_seen: ago(14), first_seen: ago(60 * 24 * 80) },
  { id: "noor", name: "Noor Haddad", email: "noor@acme.example", roles: ["approver", "member"], last_seen: ago(60 * 3), first_seen: ago(60 * 24 * 70) },
  { id: "sam", name: "Sam Rivera", email: "sam@acme.example", roles: ["member"], last_seen: ago(45), first_seen: ago(60 * 24 * 60) },
  { id: "alice", name: "Alice Moreau", email: "alice@acme.example", roles: ["member"], last_seen: ago(60 * 26), first_seen: ago(60 * 24 * 40) },
  { id: "lea", name: "Léa Dubois", email: "lea@acme.example", roles: ["member"], last_seen: ago(60 * 5), first_seen: ago(60 * 24 * 30) },
  { id: "pat", name: "Pat Kim", email: "pat@partner.example", roles: ["guest"], guest: true, last_seen: ago(60 * 24 * 2), first_seen: ago(60 * 24 * 10) },
];

export const MEMBER_ROLES = [
  { name: "admin", description: "Runs the organization", offered: true, powers: ["admin"] },
  { name: "builder", description: "Builds apps", offered: true, powers: ["builder"] },
  { name: "approver", description: "Approves go-lives", offered: true, powers: ["approver"] },
  { name: "member", description: "Uses the apps given to them", offered: true, powers: [] },
  { name: "guest", description: "Outside the organization", offered: false, powers: [], guest: true },
];

const ev = (kind: string, by: string, at: string, details?: Record<string, unknown>): ReleaseEvent => ({ kind, by, at, details: details ?? null });

function teamRequests(): MApp {
  const v = (seq: number, message: string, by: string, at: number, extra: Partial<MVersion> = {}): MVersion => ({
    hash: hash(`tr-v${seq}`), seq, message, created_by: by, created_at: ago(at), git_branch: "main", git_commit: hex(`trc${seq}`, 40), migrations: [], live_periods: [], events: [ev("deployed", by, ago(at), { message })], stores: ["requests"], ...extra,
  });
  const v1 = v(1, "First version: submit and list requests", "bo", 60 * 24 * 21, { live_periods: [{ from: ago(60 * 24 * 21), until: ago(60 * 24 * 12) }] });
  const v2 = v(2, "AI triage of each request", "bo", 60 * 24 * 12, { live_periods: [{ from: ago(60 * 24 * 12), until: ago(60 * 24 * 3) }], delta: { added_workflows: [] } });
  const v3 = v(3, "Managers approve anything above 100 €", "bo", 60 * 24 * 3, { live_periods: [{ from: ago(60 * 24 * 3 - 30) }] });
  v3.events.push(ev("went_live", "admin", ago(60 * 24 * 3 - 30)));
  const v4 = v(4, "Managers can reopen a request; weekly digest", "bo", 95, {
    needs_approval: true, reason: "it adds a write: requests.reopen", delta: { new_or_changed_writes: ["requests.reopen"], added_workflows: ["weekly_digest"] }, migrations: ["0004_reopened_at.sql"],
  });
  v4.events.push(ev("go_live_requested", "bo", ago(80)));
  return {
    id: "team-requests", name: "Team Requests", description: "Ask for equipment, travel or training; managers decide above 100 €", owner: "bo", builders: ["admin"], protected: false, created_at: ago(60 * 24 * 21),
    roles: [{ role: "member", description: "Submits requests and follows their own" }, { role: "manager", description: "Decides requests above 100 €" }],
    holders: { member: ["alice", "sam", "lea", "noor", "pat"], manager: ["sam"] },
    workflows: [
      { id: "submit_request", endpoint: { mode: "async", auth: "sso", allow: { roles: ["member", "manager"] } }, parameters: [
        { name: "title", type: "string", description: "What you need" }, { name: "amount", type: "number", description: "In euros" }, { name: "category", type: "string", description: "travel, equipment, training…", default: "equipment" }] },
      { id: "list_requests", endpoint: { mode: "sync", auth: "sso", allow: { roles: ["member", "manager"] } }, parameters: [] },
    ],
    versions: [v4, v3, v2, v1], live: v3.hash, stores: ["requests"],
    llm: { calls: 412, tokens_in: 188_400, tokens_out: 41_200, spent: 3.86, budget: 20 },
    secrets: [{ name: "slack.webhook", app: "team-requests", kind: "generic", version: 2, key_by: "master", updated_at: ago(60 * 24 * 9) }],
    client_status: "ready",
  };
}

function supportDesk(): MApp {
  const v = (seq: number, message: string, by: string, at: number, extra: Partial<MVersion> = {}): MVersion => ({
    hash: hash(`sd-v${seq}`), seq, message, created_by: by, created_at: ago(at), git_branch: "main", git_commit: hex(`sdc${seq}`, 40), migrations: [], live_periods: [], events: [ev("deployed", by, ago(at), { message })], stores: ["tickets"], ...extra,
  });
  const versions = [
    v(12, "Refunds above 200 € need a support lead", "admin", 60 * 24 * 2, { live_periods: [{ from: ago(60 * 24 * 2 - 45) }], approved_by: "noor" }),
    v(11, "Digest of stale tickets every weekday", "bo", 60 * 24 * 6, { live_periods: [{ from: ago(60 * 24 * 6), until: ago(60 * 24 * 2 - 45) }] }),
    v(10, "Triage by language and urgency", "bo", 60 * 24 * 13, { live_periods: [{ from: ago(60 * 24 * 13), until: ago(60 * 24 * 6) }] }),
    v(9, "Reply drafts in the customer's language", "admin", 60 * 24 * 20, { live_periods: [{ from: ago(60 * 24 * 20), until: ago(60 * 24 * 13) }] }),
  ];
  versions[0].events.push(ev("approved", "noor", ago(60 * 24 * 2 - 50)), ev("went_live", "noor", ago(60 * 24 * 2 - 45)));
  return {
    id: "support-desk", name: "Support Desk", description: "Answer the support queue: triage, replies and refunds", owner: "admin", builders: ["bo"], protected: true, created_at: ago(60 * 24 * 60),
    roles: [{ role: "support_agent", description: "Answers tickets" }, { role: "support_lead", description: "Approves refunds above 200 €" }],
    holders: { support_agent: ["lea", "alice"], support_lead: ["noor"] },
    workflows: [
      { id: "triage_ticket", endpoint: { mode: "async", auth: "none" }, parameters: [{ name: "subject", type: "string" }, { name: "body", type: "string" }, { name: "customer", type: "string" }] },
      { id: "refund_customer", endpoint: { mode: "async", auth: "sso", allow: { roles: ["support_agent", "support_lead"] } }, parameters: [{ name: "ticket", type: "string" }, { name: "amount", type: "number" }] },
      { id: "stale_tickets_digest", schedule: "0 8 * * 1-5", parameters: [] },
    ],
    versions, live: versions[0].hash, stores: ["tickets"],
    llm: { calls: 3_920, tokens_in: 2_810_000, tokens_out: 604_000, spent: 43.7, budget: 50 },
    secrets: [
      { name: "zendesk.token", app: "support-desk", kind: "generic", version: 3, key_by: "master", updated_at: ago(60 * 24 * 15) },
      { name: "stripe.key", app: "support-desk", kind: "generic", version: 1, key_by: "master", updated_at: ago(60 * 24 * 40) },
      { name: "tickets", app: "support-desk", kind: "store_login", version: 4, key_by: "master", updated_at: ago(60 * 24 * 2) },
    ],
    client_status: "ready",
  };
}

function expenseReview(): MApp {
  const v1: MVersion = {
    hash: hash("er-v1"), seq: 1, message: "Receipts in, reimbursements out", created_by: "bo", created_at: ago(38), git_branch: "expense-review", git_commit: hex("erc1", 40), git_dirty: true,
    migrations: ["0001_expenses.sql"], live_periods: [], events: [ev("deployed", "bo", ago(38), { message: "Receipts in, reimbursements out" })], stores: ["expenses"],
    delta: { new_or_changed_writes: ["expenses.create", "expenses.approve"], added_workflows: ["submit_expense", "reimburse"] },
  };
  return {
    id: "expense-review", name: "Expense Review", owner: "bo", builders: [], protected: false, created_at: ago(60 * 5),
    roles: [{ role: "employee", description: "Submits expenses" }, { role: "finance", description: "Reimburses them" }], holders: { employee: [], finance: [] },
    workflows: [], versions: [v1], stores: ["expenses"],
    llm: { calls: 18, tokens_in: 9_100, tokens_out: 2_300, spent: 0, budget: null },
    secrets: [], client_status: "pending",
  };
}

function onboarding(): MApp {
  const v5: MVersion = {
    hash: hash("ob-v5"), seq: 5, message: "Equipment checklist per team", created_by: "admin", created_at: ago(60 * 24 * 9), git_branch: "main", git_commit: hex("obc5", 40),
    migrations: [], live_periods: [{ from: ago(60 * 24 * 9) }], events: [ev("deployed", "admin", ago(60 * 24 * 9)), ev("went_live", "admin", ago(60 * 24 * 9))], stores: [],
  };
  return {
    id: "onboarding", name: "Onboarding", description: "Accounts, equipment and a checklist for each new hire", owner: "admin", builders: [], protected: false, created_at: ago(60 * 24 * 100),
    roles: [{ role: "manager", description: "Starts the onboarding of their new hires" }, { role: "hr", description: "Runs every onboarding" }],
    holders: { manager: ["sam"], hr: ["noor"] },
    workflows: [{ id: "start_onboarding", endpoint: { mode: "async", auth: "sso", allow: { roles: ["manager", "hr"] } }, parameters: [{ name: "new_hire_email", type: "string" }, { name: "start_date", type: "date" }, { name: "team", type: "string" }] }],
    versions: [v5], live: v5.hash, stores: [],
    llm: { calls: 0, tokens_in: 0, tokens_out: 0, spent: 0, budget: null }, secrets: [], client_status: "ready",
  };
}

const TITLES = ["New laptop for the design team", "Conference ticket: React Summit", "Train tickets Paris–Lyon", "Ergonomic chair", "Team offsite venue deposit", "Figma seats for Q4", "Hotel in Berlin, 2 nights", "Online course: SQL basics"];
const CUSTOMERS = ["Northwind", "Blue Harbor", "Kestrel Labs", "Maple & Co", "Orbital Freight", "Lumen Health"];
/** Who answers a person's step of a demo workflow. */
const DECIDER: Record<string, string> = { "team-requests": "sam", "support-desk": "noor", onboarding: "noor", "expense-review": "bo" };
const NOTES = ["Fine for this quarter", "Within the team budget", "", "Please book second class next time", ""];

/** Plays `run` again (with what the people asked answered since) and takes on what it gives: its steps, outputs, status. */
export function replay(run: MRun, answers: Record<string, Answer> = run.answers || {}) {
  const email = PEOPLE.find((p) => p.id === run.user)?.email ?? `${run.user}@acme.example`;
  const played = play(run.workflow, { params: run.params as Record<string, unknown>, user: run.user, email, started_at: run.started_at, n: run.n ?? 0, failAt: run.failAt, answers });
  const total = played.trace.reduce((t, x) => t + (x.ms || 0), 0);
  const o = played.outputs as Record<string, Record<string, unknown>>;
  const p = run.params as Record<string, unknown>;
  Object.assign(run, {
    played, answers, steps: played.trace, outputs: played.outputs, status: played.status, error: played.error,
    ended_at: played.status === "running" ? null : new Date(Date.parse(run.started_at) + total).toISOString(),
    result: played.status !== "succeeded" ? undefined
      : run.workflow === "submit_request" ? { headline: `${p.title}: ${o.decide?.status}`, fields: [{ label: "Status", value: o.decide?.status }, { label: "Category", value: o.triage?.category }, { label: "Summary", value: o.triage?.summary }, { label: "Decided by", value: o.decide?.decided_by }, { label: "Note", value: o.decide?.note }] }
      : run.workflow === "triage_ticket" ? { headline: "Billing · normal · assigned to Léa", fields: [{ label: "Language", value: "en" }] }
      : run.workflow === "refund_customer" ? { headline: o.close?.status === "refunded" ? `Refunded ${p.amount} €` : "Refund declined" } : { headline: "Done" },
  });
  return run;
}

export function makeRuns(apps: MApp[]): MRun[] {
  const r = rng(7);
  const out: MRun[] = [];
  const users: Record<string, string[]> = { "team-requests": ["alice", "sam", "lea", "noor"], "support-desk": ["lea", "alice", "service:zendesk"], onboarding: ["sam", "noor"], "expense-review": ["bo"] };
  for (const app of apps) {
    const wfs = app.id === "expense-review" ? ["submit_expense"] : app.workflows.map((w) => w.id);
    const n = app.id === "support-desk" ? 46 : app.id === "team-requests" ? 34 : app.id === "onboarding" ? 6 : 3;
    for (let i = 0; i < n; i++) {
      const wf = wfs[Math.floor(r() * wfs.length)];
      const minutes = Math.floor(i * (app.id === "onboarding" ? 900 : 70) + r() * 50 + 5);
      const fail = r() < (app.id === "support-desk" ? 0.12 : 0.06);
      const user = wf === "stale_tickets_digest" ? "schedule" : wf === "triage_ticket" ? "service:zendesk" : users[app.id][Math.floor(r() * users[app.id].length)];
      // a failing run fails at one of the steps the worker runs
      const workers = (PLANS[wf] ?? []).filter((x) => !["branch", "switch", "map", "loop", "try", "wait", "human"].includes(x.kind) && !x.nested_in);
      const failAt = fail && workers.length ? workers[Math.min(workers.length - 1, 1 + Math.floor(r() * (workers.length - 1)))].name : undefined;
      const title = TITLES[Math.floor(r() * TITLES.length)];
      // a third under the rule's 100 €
      const amount = r() < 0.35 ? Math.floor(20 + r() * 80) : Math.floor(120 + r() * 800);
      const params = wf === "submit_request" ? { title, amount, category: ["equipment", "travel", "training"][Math.floor(r() * 3)] }
        : wf === "triage_ticket" ? { subject: "Invoice shows the wrong VAT number", customer: CUSTOMERS[Math.floor(r() * CUSTOMERS.length)], body: "Hello, our last invoice shows our old VAT number. Could you send a corrected one?" }
        : wf === "refund_customer" ? { ticket: `T-${1000 + Math.floor(r() * 80)}`, amount }
        : wf === "start_onboarding" ? { new_hire_email: "jordan@acme.example", start_date: "2025-11-02", team: "Design" } : wf === "list_requests" ? { status: "all" } : {};
      const started_at = ago(minutes);
      // the people asked answered within the time the run had
      const answers: Record<string, Answer> = Object.fromEntries((PLANS[wf] ?? []).filter((x) => x.kind === "human").map((x) => [x.name, {
        approved: r() < 0.82, note: NOTES[Math.floor(r() * NOTES.length)], by: DECIDER[app.id],
        at: new Date(Date.parse(started_at) + Math.max(1, Math.min(minutes - 1, 20 + r() * 200)) * 60_000).toISOString(),
      }]));
      out.push(replay({ id: `${wf}-${hex(`${app.id}${i}`, 32)}`, app: app.id, workflow: wf, status: "running", user, started_at, params, steps: [], n: i, failAt }, answers));
    }
  }
  // runs under way, waiting for a person: a manager, a support lead
  out.push(replay({ id: `submit_request-${hex("waiting", 32)}`, app: "team-requests", workflow: "submit_request", status: "running", user: "alice", started_at: ago(22), params: { title: "Train tickets Paris–Lyon", amount: 240, category: "travel" }, steps: [], n: 101 }));
  out.push(replay({ id: `refund_customer-${hex("refund-waiting", 32)}`, app: "support-desk", workflow: "refund_customer", status: "running", user: "lea", started_at: ago(9), params: { ticket: "T-1042", amount: 230 }, steps: [], n: 102 }));
  return out.sort((a, b) => b.started_at.localeCompare(a.started_at));
}

export function makeApprovals(): MApproval[] {
  return [
    {
      id: `submit_request-${hex("waiting", 32)}:approve_request`, app: "team-requests", kind: "decision", run_id: `submit_request-${hex("waiting", 32)}`, step: "approve_request", to: "manager", workflow: "submit_request",
      requested_by: "alice", created_at: ago(21),
      payload: { step: "approve_request", to: { role: "manager" }, timeout: "7d", payload: { title: "Train tickets Paris–Lyon", amount: 240, currency: "EUR", category: "travel", requested_by: "Alice Moreau", triage: "Travel above 100 €: a manager decides", policy_link: "https://intranet.acme.example/travel-policy" } },
    },
    {
      id: `promote:team-requests:${hash("tr-v4")}`, app: "team-requests", kind: "promotion", run_id: "", step: "go-live", to: "approver", workflow: "",
      requested_by: "bo", created_at: ago(80),
      payload: { kind: "promotion", release: "v4", short: hex("tr-v4", 12), version: hash("tr-v4"), requested_by: "bo", reason: "it adds a write: requests.reopen", delta: { new_or_changed_writes: ["requests.reopen"], added_workflows: ["weekly_digest"], migrations: ["0004_reopened_at.sql"] } },
    },
    {
      id: "store:support-desk:live:tickets:7f3a", app: "support-desk", kind: "store_approval", run_id: `refund_customer-${hex("refund-waiting", 32)}`, step: "lead_approval", to: "support_lead", workflow: "refund_customer",
      requested_by: "lea", created_at: ago(8),
      payload: { kind: "store_approval", store: "tickets", target: "live", role: "support_agent", message: "Refund 230 € to Northwind (ticket T-1042)", fields: { ticket: "T-1042", customer: "Northwind", refund_eur: 230, reason: "Charged twice for October" }, requested_by: "lea", expires_at: later(60 * 24 * 2) },
    },
  ];
}

export const GUEST_REQUESTS = [
  { id: "ar_share_jordan", kind: "share", event: "access.granted", email: "jordan@contractor.example", app: "team-requests", roles: ["member"], details: null, guest: true, until: later(60 * 24 * 60), note: "Contractor helping with Q4 travel bookings", asked_by: "bo", asked_at: ago(60 * 4), status: "awaiting_approval", updated_at: ago(60 * 4) },
];

export function makeApps(fresh: boolean): MApp[] {
  return fresh ? [] : [teamRequests(), supportDesk(), expenseReview(), onboarding()];
}

/** Each table of a store: its columns (with what each role may do) and its rows. */
export type MColumn = { name: string; type: string; nullable?: boolean; pk?: boolean; default?: unknown; update?: string[]; create?: string[]; approval?: boolean };
export type MTable = { name: string; columns: MColumn[]; rows: Record<string, unknown>[]; tenant?: string; roles: Record<string, { scope: "own" | "all" | "global"; delete?: boolean }> };

export function makeStores(): Record<string, Record<string, MTable[]>> {
  const r = rng(11);
  const requests = Array.from({ length: 38 }, (_, i) => {
    const amount = Math.floor(30 + r() * 1200);
    const status = i < 3 ? "pending" : ["approved", "approved", "approved", "rejected", "done"][Math.floor(r() * 5)];
    return {
      id: 1000 + i, title: TITLES[Math.floor(r() * TITLES.length)], amount, category: ["equipment", "travel", "training", "software"][Math.floor(r() * 4)], status,
      requested_by: ["alice", "sam", "lea", "noor"][Math.floor(r() * 4)], decided_by: status === "pending" ? null : amount > 100 ? "sam" : "triage", created_at: ago(i * 300 + 30),
    };
  });
  const tickets = Array.from({ length: 64 }, (_, i) => ({
    id: `T-${1000 + i}`, subject: ["Invoice shows the wrong VAT number", "Cannot reset my password", "Refund for a double charge", "Export to CSV is empty", "Add a second admin", "API rate limit too low"][Math.floor(r() * 6)],
    customer: CUSTOMERS[Math.floor(r() * CUSTOMERS.length)], priority: ["low", "normal", "normal", "high", "urgent"][Math.floor(r() * 5)], status: i < 9 ? "open" : ["open", "waiting", "solved", "solved", "solved"][Math.floor(r() * 5)],
    assignee: ["lea", "alice", null][Math.floor(r() * 3)], refund_eur: r() < 0.15 ? Math.floor(r() * 400) : null, opened_at: ago(i * 190 + 12), vip: r() < 0.2,
  }));
  const categories = [{ name: "equipment", limit_eur: 1500 }, { name: "travel", limit_eur: 800 }, { name: "training", limit_eur: 1200 }, { name: "software", limit_eur: 500 }];
  const reqTable = (rows: typeof requests): MTable => ({
    name: "requests", tenant: "requested_by", rows,
    roles: { member: { scope: "own" }, manager: { scope: "all" } },
    columns: [
      { name: "id", type: "bigint", pk: true },
      { name: "title", type: "text", create: ["member", "manager"], update: ["manager"] },
      { name: "amount", type: "numeric", create: ["member", "manager"], update: ["manager"] },
      { name: "category", type: "text", create: ["member", "manager"], update: ["manager"] },
      { name: "status", type: "text", default: "pending", update: ["manager"], approval: false },
      { name: "requested_by", type: "text" },
      { name: "decided_by", type: "text", nullable: true, update: ["manager"] },
      { name: "created_at", type: "timestamptz", default: "now()" },
    ],
  });
  return {
    "team-requests": {
      live: [reqTable(requests), { name: "categories", rows: categories, roles: { member: { scope: "global" }, manager: { scope: "global" } }, columns: [{ name: "name", type: "text", pk: true }, { name: "limit_eur", type: "numeric", update: ["manager"] }] }],
      [hash("tr-v4")]: [reqTable(requests.slice(0, 6).map((x) => ({ ...x, title: `[seed] ${x.title}` })))],
    },
    "support-desk": {
      live: [{
        name: "tickets", rows: tickets, roles: { support_agent: { scope: "all" }, support_lead: { scope: "all", delete: true } },
        columns: [
          { name: "id", type: "text", pk: true },
          { name: "subject", type: "text", create: ["support_agent", "support_lead"], update: ["support_agent", "support_lead"] },
          { name: "customer", type: "text", create: ["support_agent", "support_lead"] },
          { name: "priority", type: "text", update: ["support_agent", "support_lead"], create: ["support_agent", "support_lead"] },
          { name: "status", type: "text", update: ["support_agent", "support_lead"] },
          { name: "assignee", type: "text", nullable: true, update: ["support_agent", "support_lead"] },
          { name: "refund_eur", type: "numeric", nullable: true, update: ["support_agent", "support_lead"], approval: true },
          { name: "vip", type: "boolean", update: ["support_lead"] },
          { name: "opened_at", type: "timestamptz", default: "now()" },
        ],
      }],
    },
    "expense-review": {
      [hash("er-v1")]: [{
        name: "expenses", tenant: "employee", rows: Array.from({ length: 5 }, (_, i) => ({ id: i + 1, employee: "bo", description: ["Taxi to the airport", "Team lunch", "Monitor", "Train to Lyon", "Books"][i], amount: [42, 186, 249, 89, 61][i], receipt: true, status: "submitted" })),
        roles: { employee: { scope: "own" }, finance: { scope: "all" } },
        columns: [{ name: "id", type: "bigint", pk: true }, { name: "employee", type: "text" }, { name: "description", type: "text", create: ["employee"] }, { name: "amount", type: "numeric", create: ["employee"] }, { name: "receipt", type: "boolean", create: ["employee"] }, { name: "status", type: "text", update: ["finance"] }],
      }],
    },
  };
}

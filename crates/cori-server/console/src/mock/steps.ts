// The demo's workflows step by step, for a run's inspection (`GET /v1/apps/{app}/runs/{run}/steps`): each step as its
// release describes it, and a run of it played out from its parameters (what each step received and gave back, the
// person who decided, the model calls). Fictional, like the rest of src/mock.
import type { RunSteps, Step, StepDecision, StepPlan } from "../lib/types";

type P = Record<string, unknown>;
type C = { params: P; steps: Record<string, P>; user: string; email: string; n: number };
/** How a demo step behaves: what it takes and gives, how long it takes; a condition's way; a repeat's items. */
type Play = { ms: number; in?: (c: C) => P; out?: (c: C, input: P) => P; way?: (c: C) => string; over?: (c: C) => P[]; notes?: (c: C, input: P) => string[] };

const s = (name: string, kind: string, description: string, file: string, line: number, more: Partial<StepPlan> = {}): StepPlan => ({
  name, kind, description, access: "none", external: false, file: `steps/${file}.ts`, line, ...more,
});
const W = { access: "may_write" as const, external: true }, R = { access: "read" as const, external: true }, AI = { access: "prompt" as const, external: true };

export const PLANS: Record<string, StepPlan[]> = {
  submit_request: [
    s("triage", "llm", "Sort and summarise the request", "triage", 9, { ...AI, tier: "small", fallback: "default", uses: ["params.amount", "params.category", "params.title"] }),
    s("route", "code", "Approve by the rule, or ask a manager", "route", 11, { uses: ["params.amount", "steps.triage.category"] }),
    s("record", "store", "Record the request as pending", "record", 7, { ...W, connector: "requests", tool: "createRequest", uses: ["params.amount", "params.title", "steps.triage.category", "steps.triage.summary", "user.email"] }),
    s("approval", "branch", "Ask a manager when the rule does not approve", "approval", 8, { uses: ["steps.route.needs_approval"], goto: [{ slot: "else", to: "decide" }] }),
    s("approve_request", "human", "Approve or decline the request", "approval", 12, { nested_in: "approval", slot: "then", ask: "approval", to: { role: "manager" }, timeout: "7d", on_timeout: "default",
      uses: ["params.amount", "params.title", "steps.record.id", "steps.route.reason", "steps.triage.category", "steps.triage.summary", "user.email"] }),
    s("decide", "code", "The outcome: the rule's or the manager's", "decide", 7, { uses: ["steps.approve_request", "steps.route.needs_approval", "steps.route.reason"] }),
    s("close", "store", "Record the outcome on the request", "close", 9, { ...W, connector: "requests", tool: "updateRequest", uses: ["steps.decide.decided_by", "steps.decide.note", "steps.decide.status", "steps.record.id"] }),
  ],
  list_requests: [
    s("latest", "store", "The latest 100 requests the caller may see", "latest", 8, { ...R, connector: "requests", query: true, uses: ["params.status"],
      query_text: "select id, title, amount, category, status, requested_by, decided_by, created_at\n  from requests where ($1 = 'all' or status = $1)\n  order by created_at desc limit 100" }),
  ],
  triage_ticket: [
    s("detect_language", "llm", "Find the language the customer wrote in", "detect_language", 6, { ...AI, tier: "small", uses: ["params.body", "params.subject"] }),
    s("classify", "llm", "Sort the ticket by topic and urgency", "classify", 8, { ...AI, tier: "medium", fallback: "default", uses: ["params.body", "params.customer", "params.subject", "steps.detect_language.language"] }),
    s("assign", "store", "Give the ticket to the agent for its topic", "assign", 10, { ...W, connector: "tickets", tool: "updateTicket", uses: ["params.customer", "steps.classify.topic", "steps.classify.urgency"] }),
  ],
  refund_customer: [
    s("load_ticket", "store", "Load the ticket and the customer's charge", "load_ticket", 6, { ...R, connector: "tickets", tool: "getTicket", uses: ["params.ticket"] }),
    s("lead_approval", "human", "A support lead approves the refund", "lead_approval", 9, { ask: "approval", to: { role: "support_lead" }, timeout: "2d", uses: ["params.amount", "steps.load_ticket.customer", "steps.load_ticket.reason"] }),
    s("if_approved", "branch", "Refund only what the lead approved", "if_approved", 8, { uses: ["steps.lead_approval.approved"], goto: [{ slot: "else", to: "close" }] }),
    s("refund", "mcp", "Refund the customer through Stripe", "if_approved", 11, { ...W, nested_in: "if_approved", slot: "then", connector: "stripe", tool: "create_refund", uses: ["params.amount", "steps.load_ticket.charge"] }),
    s("close", "store", "Note the refund on the ticket and close it", "close", 7, { ...W, connector: "tickets", tool: "updateTicket", uses: ["params.ticket", "steps.lead_approval.approved", "steps.refund.refund_id"] }),
  ],
  stale_tickets_digest: [
    s("find_stale", "store", "Find the tickets nobody answered for 3 days", "find_stale", 6, { ...R, connector: "tickets", query: true, query_text: "select id, subject, customer, assignee, opened_at\n  from tickets where status = 'open' and opened_at < now() - interval '3 days'\n  order by opened_at" }),
    s("write_digest", "llm", "Write the morning digest", "write_digest", 8, { ...AI, tier: "large", uses: ["steps.find_stale.rows"] }),
    s("post", "http", "Post the digest to #support on Slack", "post", 7, { ...W, connector: "slack", method: "POST", uses: ["steps.write_digest.text"] }),
  ],
  start_onboarding: [
    s("accounts", "mcp", "Create the new hire's accounts", "accounts", 7, { ...W, connector: "google_workspace", tool: "create_user", uses: ["params.new_hire_email", "params.team"] }),
    s("checklist", "map", "Add the team's checklist, one task at a time", "checklist", 9, { uses: ["params.team"] }),
    s("add_task", "store", "Add a task to the checklist", "checklist", 12, { ...W, nested_in: "checklist", slot: "each", connector: "onboarding", tool: "createTask", uses: ["item.title", "item.owner", "params.new_hire_email"] }),
    s("welcome", "http", "Welcome them on Slack", "welcome", 6, { ...W, connector: "slack", method: "POST", uses: ["params.new_hire_email", "params.start_date", "params.team"] }),
  ],
  submit_expense: [
    s("check", "code", "Check the receipt and the amount", "check", 6, { uses: ["params.amount", "params.receipt"] }),
    s("save", "store", "Save the expense for finance", "save", 7, { ...W, connector: "expenses", tool: "createExpense", uses: ["params.amount", "params.description", "steps.check.ok"] }),
  ],
};

const num = (v: unknown) => Number(v) || 0;
const id = (c: C, prefix: string) => `${prefix}_${(c.n * 7919 + 104729).toString(36)}`;
const PLAY: Record<string, Record<string, Play>> = {
  submit_request: {
    triage: { ms: 1600, in: (c) => ({ title: c.params.title, details: "", amount_eur: c.params.amount }), out: (c) => ({ category: c.params.category, summary: `${c.params.title}, about ${c.params.amount} €` }), notes: () => ["tier small → anthropic/claude-haiku-5-5"] },
    route: { ms: 9, in: (c) => ({ amount_eur: c.params.amount, category: c.steps.triage.category }),
      out: (_, i) => (i.category === "travel" ? { needs_approval: true, reason: "Travel is always approved by a person" } : num(i.amount_eur) > 100 ? { needs_approval: true, reason: "Over 100 €" } : { needs_approval: false, reason: "Up to 100 €: approved by the rule" }) },
    record: { ms: 58, in: (c) => ({ title: c.params.title, details: "", amount_eur: c.params.amount, category: c.steps.triage.category, summary: c.steps.triage.summary, requester: c.email, status: "pending" }), out: (c) => ({ id: id(c, "req") }), notes: () => ["createRequest via cori store"] },
    approval: { ms: 4, way: (c) => (c.steps.route.needs_approval ? "then" : "else") },
    approve_request: { ms: 0, in: (c) => ({ request_id: c.steps.record.id, title: c.params.title, details: "", amount_eur: c.params.amount, category: c.steps.triage.category, summary: c.steps.triage.summary, requester: c.email, reason: c.steps.route.reason }) },
    decide: { ms: 6, in: (c) => ({ needs_approval: c.steps.route.needs_approval, reason: c.steps.route.reason, decision: c.steps.approve_request ?? null }),
      out: (_, i) => (!i.needs_approval ? { status: "approved", decided_by: "rule", note: i.reason } : { status: (i.decision as P)?.approved ? "approved" : "declined", decided_by: "manager", note: (i.decision as P)?.note ?? "" }) },
    close: { ms: 44, in: (c) => ({ id: c.steps.record.id, status: c.steps.decide.status, decided_by: c.steps.decide.decided_by, decision_note: c.steps.decide.note }), out: (c) => ({ status: c.steps.decide.status }), notes: () => ["updateRequest via cori store"] },
  },
  list_requests: {
    latest: { ms: 38, in: () => ({ status: "all" }), out: () => ({ rows: [
      { id: "req_8k2", title: "Train tickets Paris–Lyon", amount: 240, status: "pending" }, { id: "req_8jz", title: "Ergonomic chair", amount: 89, status: "approved" }, { id: "req_8jq", title: "Figma seats for Q4", amount: 360, status: "approved" }] }),
      notes: () => ["query: select id, title, amount, category, status"] },
  },
  triage_ticket: {
    detect_language: { ms: 900, in: (c) => ({ subject: c.params.subject, body: c.params.body }), out: () => ({ language: "en" }), notes: () => ["tier small → anthropic/claude-haiku-5-5"] },
    classify: { ms: 1400, in: (c) => ({ subject: c.params.subject, body: c.params.body, customer: c.params.customer, language: c.steps.detect_language.language }), out: () => ({ topic: "billing", urgency: "normal" }), notes: () => ["tier medium → anthropic/claude-sonnet-5-5"] },
    assign: { ms: 50, in: (c) => ({ customer: c.params.customer, topic: c.steps.classify.topic, urgency: c.steps.classify.urgency, assignee: "lea" }), out: () => ({ assignee: "lea", status: "open" }), notes: () => ["updateTicket via cori store"] },
  },
  refund_customer: {
    load_ticket: { ms: 41, in: (c) => ({ id: c.params.ticket }), out: (c) => ({ id: c.params.ticket, customer: "Northwind", charge: id(c, "ch"), amount_paid: num(c.params.amount) * 2, reason: "Charged twice for October" }), notes: () => ["getTicket via cori store"] },
    lead_approval: { ms: 0, in: (c) => ({ ticket: c.params.ticket, customer: c.steps.load_ticket.customer, refund_eur: c.params.amount, reason: c.steps.load_ticket.reason }) },
    if_approved: { ms: 3, way: (c) => (c.steps.lead_approval?.approved ? "then" : "else") },
    refund: { ms: 820, in: (c) => ({ charge: c.steps.load_ticket.charge, amount: num(c.params.amount) * 100, currency: "eur" }), out: (c) => ({ refund_id: id(c, "re"), status: "succeeded" }), notes: () => ["create_refund via mcp http"] },
    close: { ms: 44, in: (c) => ({ id: c.params.ticket, status: c.steps.lead_approval?.approved ? "refunded" : "solved", refund_id: c.steps.refund?.refund_id ?? null }), out: (c) => ({ status: c.steps.lead_approval?.approved ? "refunded" : "solved" }), notes: () => ["updateTicket via cori store"] },
  },
  stale_tickets_digest: {
    find_stale: { ms: 120, in: () => ({}), out: () => ({ rows: [{ id: "T-1007", subject: "Export to CSV is empty", customer: "Kestrel Labs", assignee: null }, { id: "T-1012", subject: "Add a second admin", customer: "Maple & Co", assignee: "alice" }] }), notes: () => ["query: select id, subject, customer, assignee, opened_at"] },
    write_digest: { ms: 2600, in: (c) => ({ tickets: (c.steps.find_stale.rows as P[]).map((t) => `${t.id}: ${t.subject}`) }), out: () => ({ text: "Two tickets wait for an answer: T-1007 (Kestrel Labs, no one assigned) and T-1012 (Maple & Co, Alice)." }), notes: () => ["tier large → anthropic/claude-opus-5-5"] },
    post: { ms: 310, in: (c) => ({ channel: "#support", text: c.steps.write_digest.text }), out: () => ({ ok: true }), notes: () => ["POST /api/chat.postMessage → 200"] },
  },
  start_onboarding: {
    accounts: { ms: 1500, in: (c) => ({ email: c.params.new_hire_email, team: c.params.team }), out: (c) => ({ user_id: id(c, "usr"), groups: [`${String(c.params.team).toLowerCase()}@acme.example`] }), notes: () => ["create_user via mcp http"] },
    checklist: { ms: 0, over: (c) => [{ title: "Laptop ready", owner: "it" }, { title: `Meet the ${c.params.team} team`, owner: "manager" }, { title: "Read the handbook", owner: "new hire" }] },
    add_task: { ms: 70, out: (c, i) => ({ id: id({ ...c, n: c.n + String(i.title).length }, "task"), title: i.title }), notes: () => ["createTask via cori store"] },
    welcome: { ms: 280, in: (c) => ({ channel: `#${String(c.params.team).toLowerCase()}`, text: `Welcome ${c.params.new_hire_email}, starting ${c.params.start_date}!` }), out: () => ({ ok: true }), notes: () => ["POST /api/chat.postMessage → 200"] },
  },
  submit_expense: {
    check: { ms: 12, in: (c) => ({ amount: c.params.amount ?? 42, receipt: true }), out: () => ({ ok: true }) },
    save: { ms: 61, in: (c) => ({ description: c.params.description ?? "Taxi to the airport", amount: c.params.amount ?? 42, status: "submitted" }), out: (c) => ({ id: id(c, "exp") }), notes: () => ["createExpense via cori store"] },
  },
};

const FAILURES: Record<string, string> = {
  mcp: "StepFailedError: stripe: refund failed: the charge was already refunded",
  http: "StepFailedError: POST /api/chat.postMessage → 503 Service Unavailable",
  llm: "SchemaValidationError: the model's answer does not fit the step's output: category: expected one of equipment, software, travel, training, other",
  store: "StepFailedError: the row changed since it was read",
  code: "StepFailedError: amount must be a number",
};
const PROMPTS: Record<string, (i: P) => string> = {
  triage: (i) => `Sort this team request into exactly one category: equipment, software, travel, training, other.\nThen summarise it in one sentence a manager can read at a glance (no more than 200 characters).\nReturn JSON matching the schema.\n\nTitle: ${i.title}\nEstimated cost: ${i.amount_eur} EUR\nDetails:\n${i.details || "(none)"}`,
  detect_language: (i) => `Which language is this support ticket written in? Answer with its ISO 639-1 code.\n\nSubject: ${i.subject}\n${i.body}`,
  classify: (i) => `Sort this ticket from ${i.customer} by topic (billing, access, bug, feature) and urgency (low, normal, high, urgent).\n\nSubject: ${i.subject}\n${i.body}`,
  write_digest: (i) => `Write a short morning digest for the support team about these tickets nobody answered for 3 days:\n${(i.tickets as string[]).join("\n")}`,
};
const MODEL: Record<string, [string, number, number]> = { small: ["claude-haiku-5-5", 0.9, 4.5], medium: ["claude-sonnet-5-5", 2.8, 14], large: ["claude-opus-5-5", 4.6, 23] };

/** A person's answer to a step, or none yet (`pending`: the run waits there). */
export type Answer = { approved: boolean; note?: string; by: string; at: string };
/** A run played out: its trace (as it ended, or as far as it went), outputs, attempts, decisions and model calls. */
export type Played = { trace: Step[]; outputs: Record<string, unknown>; status: "succeeded" | "failed" | "running"; error?: string; data: Omit<RunSteps, "workflow" | "steps"> };

/** Plays `wf` from its parameters: `failAt`, the step whose every attempt fails; `answers`, what the people asked said
 *  (a person's step with no answer leaves the run waiting there). */
export function play(wf: string, o: { params: P; user: string; email: string; started_at: string; n: number; failAt?: string; answers?: Record<string, Answer> }): Played {
  const plan = PLANS[wf] ?? [];
  const steps = PLAY[wf] ?? {};
  const c: C = { params: o.params, steps: {}, user: o.user, email: o.email, n: o.n };
  const out: Played = { trace: [], outputs: {}, status: "succeeded", data: { attempts: {}, decisions: [], children: [], model_calls: [] } };
  let t = Date.parse(o.started_at);
  const at = (ms = 0) => new Date((t += ms)).toISOString();
  const nested = (p: StepPlan, slot?: string) => plan.filter((x) => x.nested_in === p.name && (!slot || x.slot === slot));
  const skip = (p: StepPlan) => { out.trace.push({ name: p.name, kind: p.kind, status: "not_taken", attempts: 0, ms: 0, notes: [] }); nested(p).forEach(skip); };
  let jump: string | null = null;

  // a step the worker runs: its attempts, its trace entry, its model call; false when it failed for good
  const work = (p: StepPlan, item?: P): boolean => {
    const b = steps[p.name] ?? { ms: 20 };
    const input = item ?? b.in?.(c) ?? {};
    const ms = Math.round(b.ms * (0.7 + ((o.n * 37 + p.name.length * 11) % 60) / 100));
    if (p.name === o.failAt) {
      const error = FAILURES[p.kind] ?? "StepFailedError: it failed";
      const tries = ["code", "llm", "activity"].includes(p.kind) || p.access === "read" ? 3 : 1;
      for (let k = 1; k <= tries; k++) (out.data.attempts[p.name] ??= []).push({ at: at(ms), attempt: k, status: "failed", ms, input, error });
      out.trace.push({ name: p.name, kind: p.kind, status: "failed", attempts: tries, ms: ms * tries, error, notes: [] });
      out.status = "failed"; out.error = error.replace(/^[A-Za-z]+: /, "");
      return false;
    }
    const output = b.out?.(c, input) ?? {};
    const notes = b.notes?.(c, input) ?? [];
    let cost = 0;
    if (p.kind === "llm") {
      const [model, pin, pout] = MODEL[p.tier || "medium"];
      const tin = 300 + ((o.n * 53) % 700), tout = 40 + ((o.n * 17) % 160);
      cost = (tin * pin + tout * pout) / 1e6;
      out.data.model_calls!.push({ step: p.name, tier: p.tier || "medium", model, provider: "anthropic", mode: "primary", valid: true, ms, tokens_in: tin, tokens_out: tout, cost_eur: cost, prompt: PROMPTS[p.name]?.(input) ?? null, at: at() });
    }
    (out.data.attempts[p.name] ??= []).push({ at: at(ms), attempt: 1, status: "ok", ms, input, output });
    out.trace.push({ name: p.name, kind: p.kind, status: "ok", attempts: 1, ms, notes, cost_eur: cost || undefined });
    if (!item) { c.steps[p.name] = output; out.outputs[p.name] = output; }
    return true;
  };
  // a person's step: their answer, or the run waits
  const ask = (p: StepPlan): boolean => {
    const asked_at = at(30_000);
    const shown = steps[p.name]?.in?.(c) ?? {};
    const a = o.answers?.[p.name];
    const d: StepDecision = { step: p.name, to: p.to?.role ?? "user:initiator", status: a ? "decided" : "pending", asked_at, shown, decided_by: a?.by ?? null, decided_at: a?.at ?? null };
    out.data.decisions.push(d);
    if (!a) { out.status = "running"; return false; }
    const answer = { approved: a.approved, ...(a.note ? { note: a.note } : {}) };
    t = Math.max(t, Date.parse(a.at));
    out.trace.push({ name: p.name, kind: p.kind, status: "ok", attempts: 1, ms: Date.parse(a.at) - Date.parse(asked_at), notes: [] });
    c.steps[p.name] = answer; out.outputs[p.name] = answer;
    return true;
  };
  const run = (p: StepPlan): boolean => {
    if (p.kind === "human") return ask(p);
    if (p.kind === "branch") {
      const way = steps[p.name]?.way?.(c) ?? "then";
      out.trace.push({ name: p.name, kind: p.kind, status: "ok", attempts: 1, ms: steps[p.name]?.ms ?? 3, notes: [], path: way });
      nested(p).filter((x) => x.slot !== way).forEach(skip);
      const go = p.goto?.find((g) => g.slot === way);
      if (go) { jump = go.to; return true; }
      return nested(p, way).every(run);
    }
    if (p.kind === "map") {
      const each = nested(p, "each")[0];
      const items = steps[p.name]?.over?.(c) ?? [];
      const outputs: unknown[] = [];
      for (const it of items) { if (!work(each, it)) return false; outputs.push(out.data.attempts[each.name]!.at(-1)!.output); }
      out.trace.push({ name: p.name, kind: p.kind, status: "ok", attempts: 1, ms: 0, notes: [] });
      c.steps[p.name] = { items: outputs }; out.outputs[p.name] = { items: outputs };
      return true;
    }
    return work(p);
  };
  for (const p of plan.filter((x) => !x.nested_in)) {
    if (jump && p.name !== jump) { skip(p); continue; }
    jump = null;
    if (!run(p)) break;
  }
  return out;
}

const NAMES: Record<string, [string, string]> = {
  submit_request: ["Submit a request", "File a request; a model sorts and summarises it, a rule approves small amounts, a manager decides the rest."],
  list_requests: ["List requests", "The latest requests the caller may see; their own, or every member's for a manager."],
  triage_ticket: ["Triage a ticket", "Find the customer's language, sort the ticket by topic and urgency, and give it to the right agent."],
  refund_customer: ["Refund a customer", "A support lead approves the refund; Stripe pays it back and the ticket closes."],
  stale_tickets_digest: ["Stale tickets digest", "Every weekday morning, the tickets nobody answered for 3 days, posted to #support."],
  start_onboarding: ["Start an onboarding", "Accounts, a checklist and a welcome for each new hire."],
  submit_expense: ["Submit an expense", "A receipt in, an expense for finance to reimburse."],
};

/** What `GET …/runs/{run}/steps` answers for a run played as far as `data` says: builders get the technical part. */
export function stepsOf(wf: string, release: string | null, data: Played["data"] | undefined, builds: boolean): RunSteps {
  const [name, description] = NAMES[wf] ?? [];
  const plan = (PLANS[wf] ?? []).map((p) => (builds ? p : { ...p, file: undefined, line: undefined, query_text: undefined }));
  const d = data ?? { attempts: {}, decisions: [], children: [] };
  return { workflow: { id: wf, name, description, release }, steps: plan, attempts: d.attempts, decisions: d.decisions, children: d.children, ...(builds ? { model_calls: d.model_calls ?? [] } : {}) };
}

/** The steps the worker ran so far, as a running run's events say them (conditions and people's steps send none). */
export const workerSteps = (trace: Step[]) => trace.filter((x) => !["branch", "switch", "map", "loop", "try", "wait", "human"].includes(x.kind));

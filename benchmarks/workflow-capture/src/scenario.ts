import { createHash } from "node:crypto";

import { taskById } from "./tasks.js";
import type { RegisteredResource, Scenario, ScenarioFixture, ScenarioLane, TaskSpec } from "./types.js";

/** A tiny deterministic PRNG so fixtures do not depend on runtime entropy. */
export class SeededRandom {
  #state: number;

  constructor(seed: number) {
    this.#state = seed >>> 0 || 0x9e3779b9;
  }

  next(): number {
    let x = this.#state;
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    this.#state = x >>> 0;
    return this.#state;
  }

  int(min: number, maxInclusive: number): number {
    return min + (this.next() % (maxInclusive - min + 1));
  }
}

export function scenarioId(taskId: string, seed: number, lane: ScenarioLane, runNamespace = ""): string {
  const identity = runNamespace ? `${runNamespace}:${taskId}:${seed}:${lane}` : `${taskId}:${seed}:${lane}`;
  const digest = createHash("sha256").update(identity).digest("hex").slice(0, 10);
  return `${taskId}-${seed}-${lane}-${digest}`;
}

export function buildScenario(taskId: string, seed: number, lane: ScenarioLane, runNamespace = ""): Scenario {
  const task = taskById(taskId);
  const id = scenarioId(taskId, seed, lane, runNamespace);
  const runTag = `cori-bench-${id}`;
  const random = new SeededRandom(seed);
  const fixtures = task.resources.map((resource, index) => fixtureFor(task, resource.role, resource.service, runTag, random, index));
  const parameters = Object.fromEntries(
    task.parameters.map(({ name }) => [name, parameterValue(name, task, id, runTag)]),
  );
  const resources: RegisteredResource[] = task.resources.map((resource, index) => ({
    id: `pending-${index}`,
    role: resource.role,
    service: resource.service,
    createdByBenchmark: true,
  }));
  const scenario: Scenario = {
    id,
    taskId,
    seed,
    lane,
    runTag,
    parameters,
    fixtures,
    expected: {
      facts: expectedFacts(task, random),
      rubric: task.rubric,
    },
    resources,
  };
  const fixtureErrors = validateScenarioFixtures(scenario);
  if (fixtureErrors.length > 0) throw new Error(`invalid ${taskId} fixture: ${fixtureErrors.join("; ")}`);
  return scenario;
}

export function assertTwinEquivalent(left: Scenario, right: Scenario): void {
  if (left.taskId !== right.taskId || left.seed !== right.seed) throw new Error("twins must share task and seed");
  if (left.lane === right.lane) throw new Error("twins must use different lanes");
  if (JSON.stringify(left.expected) !== JSON.stringify(right.expected)) {
    throw new Error("twin expected state differs");
  }
  if (left.runTag === right.runTag) throw new Error("twins must have distinct tags");
}

function parameterValue(name: string, task: TaskSpec, id: string, runTag: string): string {
  if (name === "run_tag") return runTag;
  if (name === "as_of") return "2026-07-13T09:00:00Z";
  if (name === "week_ending") return "2026-07-10";
  if (name === "period") return "2026-Q2";
  if (name === "gmail_query") return `label:inbox is:unread "${runTag}"`;
  return `pending-${task.id}-${name}-${id}`;
}

function fixtureFor(
  task: TaskSpec,
  role: string,
  service: ScenarioFixture["service"],
  runTag: string,
  random: SeededRandom,
  ordinal: number,
): ScenarioFixture {
  const suffix = `${runTag}-${ordinal + 1}`;
  if (service === "sheets") {
    const table = sheetFixture(task.id, role, runTag);
    if (table) return { role, service, title: `${task.name} ${suffix}`, table };
    return {
      role,
      service,
      title: `${task.name} ${suffix}`,
      table: [
        ["id", "status", "created_at", "amount", "notes", "benchmark_tag"],
        ...Array.from({ length: 6 + random.int(0, 4) }, (_, row) => [
          `${task.domain.slice(0, 3).toUpperCase()}-${row + 1}`,
          row % 3 === 0 ? "open" : "in_progress",
          `2026-07-${String(1 + row).padStart(2, "0")}T0${row}:00:00Z`,
          String(50 + random.int(0, 300)),
          row === 0 ? "boundary fixture" : "synthetic benchmark fixture",
          runTag,
        ]),
      ],
    };
  }
  if (service === "calendar") {
    return {
      role,
      service,
      title: `${task.name} Calendar ${suffix}`,
      events: task.id === "customer_meeting_prep"
        ? [{
            summary: `Acme renewal review ${runTag}`,
            description: "Objective: agree renewal plan. Risk: delayed security review.",
            start: { dateTime: "2026-07-15T10:00:00+02:00" },
            end: { dateTime: "2026-07-15T11:00:00+02:00" },
          }]
        : [],
    };
  }
  if (service === "gmail") {
    const supportMessages = [
      {
        subject: `[${runTag}] Checkout unavailable for all customers`,
        body: "Every customer receives HTTP 503 at checkout. The service is broadly unavailable.",
        expected_category: "outage",
        expected_priority: "P0",
      },
      {
        subject: `[${runTag}] Administrator cannot sign in`,
        body: "Our administrator is blocked from signing in and needs account access restored.",
        expected_category: "access",
        expected_priority: "P1",
      },
      {
        subject: `[${runTag}] How do I export a report?`,
        body: "Please share the steps to export the monthly report as CSV.",
        expected_category: "how_to",
        expected_priority: "P2",
      },
    ];
    const source = task.id === "customer_meeting_prep"
      ? { subject: `[${runTag}] Acme renewal risks`, body: "Acme confirmed 120 seats. Security review is delayed. They need SSO enabled before renewal." }
      : task.id === "support_inbox_triage"
        ? supportMessages[ordinal - 1] ?? supportMessages[0]!
        : { subject: `[${runTag}] Synthetic ${task.domain} message 1`, body: "Boundary case; do not send email." };
    return {
      role,
      service,
      title: `${task.name} Inbox ${suffix}`,
      messages: [source],
    };
  }
  const sourceText: Record<string, string> = {
    customer_meeting_prep: "Account Brief — Acme Corp\nPlan: Enterprise\nSeats: 120\nRenewal date: 2026-08-01\nAccount objective: enable SSO before renewal.\nKnown risk: security review is delayed.",
    new_hire_onboarding_pack: "Onboarding Pack\nName: {{NAME}}\nEmail: {{EMAIL}}\nManager: {{MANAGER}}\nStart date: {{START_DATE}}\nRun tag: {{RUN_TAG}}",
    weekly_operating_review: "Weekly Operating Review {{WEEK_ENDING}}\nGreen: {{GREEN_COUNT}}\nAmber: {{AMBER_COUNT}}\nRed: {{RED_COUNT}}\nEscalations: {{ESCALATIONS}}\nRun tag: {{RUN_TAG}}",
    expense_policy_audit: "Expense Exceptions Report\nExceptions: {{EXCEPTION_COUNT}}\nReasons: {{REASONS}}\nRun tag: {{RUN_TAG}}",
    meeting_action_register: "Decisions\nAlice will publish the migration plan by 2026-07-16.\nBob owns updating the risk register; no due date was agreed.\nFollow-ups\nALICE will publish the migration plan by 2026-07-16.\nCarol: schedule the customer workshop for 2026-07-20.\nContext only: the budget was approved; this is not an action.",
  };
  return {
    role,
    service,
    title: `${task.name} ${role} ${suffix}`,
    text: `${sourceText[task.id] ?? `Synthetic ${task.name} source.`}\nBenchmark tag: ${runTag}`,
  };
}

function sheetFixture(taskId: string, role: string, runTag: string): string[][] | null {
  if (taskId === "support_inbox_triage") return [
    ["benchmark_tag"],
    [runTag],
  ];
  if (taskId === "sla_breach_pack" && role === "case register") return [
    ["case_id", "status", "priority", "opened_at", "subject", "benchmark_tag"],
    ["CASE-P0-BREACHED", "open", "P0", "2026-07-13T07:30:00Z", "Checkout unavailable", runTag],
    ["CASE-P1-WARNING", "in_progress", "P1", "2026-07-13T05:30:00Z", "Account access blocked", runTag],
    ["CASE-P2-WARNING", "open", "P2", "2026-07-12T10:30:00Z", "Billing question", runTag],
    ["CASE-P3-BREACHED", "in_progress", "P3", "2026-07-10T08:59:00Z", "Low-priority request", runTag],
    ["CASE-P1-HEALTHY", "open", "P1", "2026-07-13T08:00:00Z", "Configuration question", runTag],
    ["CASE-CLOSED-IGNORE", "closed", "P0", "2026-07-13T07:00:00Z", "Already resolved", runTag],
  ];
  if (taskId === "lead_follow_up_queue") return [
    ["lead_id", "status", "stage", "next_action_due", "value", "last_contact_at", "contact_name", "contact_email", "next_action", "benchmark_tag"],
    ["LEAD-001", "active", "proposal", "2026-07-13T08:00:00Z", "12000", "2026-07-01T09:00:00Z", "Avery Stone", "avery@example.test", "Review proposal", runTag],
    ["LEAD-002", "active", "negotiation", "2026-07-14T09:00:00Z", "8000", "2026-07-02T09:00:00Z", "Blair Chen", "blair@example.test", "Confirm legal review", runTag],
    ["LEAD-003", "active", "proposal", "2026-07-13T09:00:00Z", "15000", "2026-07-01T09:00:00Z", "Casey Diaz", "casey@example.test", "Send pricing", runTag],
    ["LEAD-004", "active", "qualified", "2026-07-12T09:00:00Z", "20000", "2026-06-30T09:00:00Z", "Devon Reed", "devon@example.test", "Book discovery", runTag],
    ["LEAD-005", "lost", "negotiation", "2026-07-01T09:00:00Z", "50000", "2026-06-01T09:00:00Z", "Elliot Fox", "elliot@example.test", "Ignore", runTag],
  ];
  if (taskId === "new_hire_onboarding_pack") return [
    ["hire_id", "status", "name", "email", "manager", "start_date", "timezone", "orientation_date", "prepared", "pack_link", "event_link", "benchmark_tag"],
    ["HIRE-001", "pending", "Jordan Lee", "jordan.lee@example.test", "Morgan Patel", "2026-07-20", "Europe/Paris", "2026-07-20", "false", "", "", runTag],
  ];
  if (taskId === "preapproved_pto_processing") return [
    ["row_type", "request_id", "status", "manager_approved", "employee_name", "employee_email", "start_date", "end_date", "pto_balance_days", "holiday_date", "business_days", "benchmark_tag"],
    ["request", "PTO-001", "approved", "true", "Riley Martin", "riley@example.test", "2026-07-14", "2026-07-20", "12", "", "", runTag],
    ["holiday", "HOL-001", "", "", "", "", "", "", "", "2026-07-14", "", runTag],
  ];
  if (taskId === "weekly_operating_review") return [
    ["project_id", "blocked", "days_overdue", "progress_percent", "owner", "benchmark_tag"],
    ["PROJ-RED-BLOCKED", "true", "0", "95", "Alice", runTag],
    ["PROJ-RED-OVERDUE", "false", "15", "90", "Bob", runTag],
    ["PROJ-RED-PROGRESS", "false", "0", "49", "Carol", runTag],
    ["PROJ-AMBER-BOUNDARY", "false", "7", "90", "Devon", runTag],
    ["PROJ-AMBER-PROGRESS", "false", "0", "79", "Elliot", runTag],
    ["PROJ-GREEN", "false", "0", "80", "Frankie", runTag],
  ];
  if (taskId === "meeting_action_register" && role === "action tracker") return [
    ["action", "owner", "due_date", "source_section", "run_tag"],
  ];
  if (taskId === "expense_policy_audit") return [
    ["expense_id", "category", "amount", "receipt_present", "hotel_nights", "attendees", "personal", "invoice_id", "benchmark_tag"],
    ["EXP-001", "office", "74.99", "false", "", "", "false", "INV-001", runTag],
    ["EXP-002", "office", "75", "false", "", "", "false", "INV-002", runTag],
    ["EXP-003", "hotel", "600", "true", "2", "", "false", "INV-003", runTag],
    ["EXP-004", "meal", "130", "true", "", "2", "false", "INV-004", runTag],
    ["EXP-005", "travel", "40", "true", "", "", "true", "INV-005", runTag],
    ["EXP-006", "office", "80", "true", "", "", "false", "INV-DUP", runTag],
    ["EXP-007", "office", "90", "true", "", "", "false", "INV-DUP", runTag],
  ];
  if (taskId === "budget_variance_deck") return [
    ["line_id", "type", "category", "budget", "actual", "period", "benchmark_tag"],
    ["BUD-001", "expense", "Cloud", "1000", "1150", "2026-Q2", runTag],
    ["BUD-002", "expense", "Travel", "1000", "900", "2026-Q2", runTag],
    ["BUD-003", "revenue", "Subscriptions", "10000", "8500", "2026-Q2", runTag],
    ["BUD-004", "revenue", "Services", "5000", "5500", "2026-Q2", runTag],
    ["BUD-005", "expense", "New Program", "0", "500", "2026-Q2", runTag],
  ];
  return null;
}

const REQUIRED_SHEET_HEADERS: Readonly<Record<string, readonly string[]>> = {
  support_inbox_triage: ["benchmark_tag"],
  sla_breach_pack: ["case_id", "status", "priority", "opened_at", "subject", "benchmark_tag"],
  lead_follow_up_queue: ["lead_id", "stage", "next_action_due", "value", "last_contact_at", "contact_email", "next_action"],
  new_hire_onboarding_pack: ["hire_id", "status", "name", "email", "manager", "orientation_date", "prepared"],
  preapproved_pto_processing: ["row_type", "manager_approved", "start_date", "end_date", "pto_balance_days", "holiday_date"],
  weekly_operating_review: ["project_id", "blocked", "days_overdue", "progress_percent"],
  meeting_action_register: ["action", "owner", "due_date", "source_section"],
  expense_policy_audit: ["expense_id", "category", "amount", "receipt_present", "invoice_id"],
  budget_variance_deck: ["line_id", "type", "budget", "actual", "period"],
};

export function validateScenarioFixtures(scenario: Scenario): readonly string[] {
  const errors: string[] = [];
  const required = REQUIRED_SHEET_HEADERS[scenario.taskId] ?? [];
  const sheets = scenario.fixtures.filter((fixture) => fixture.service === "sheets");
  if (required.length > 0 && sheets.length === 0) errors.push("missing Sheets fixture");
  if (required.length > 0 && !sheets.some((fixture) => {
    const headers = fixture.table?.[0]?.map((header) => header.toLowerCase()) ?? [];
    return required.every((header) => headers.includes(header));
  })) errors.push(`missing required sheet headers: ${required.join(", ")}`);
  for (const fixture of scenario.fixtures) {
    if (fixture.service === "sheets" && (!fixture.table || fixture.table.length < 1)) errors.push(`${fixture.role} has no table`);
    if (fixture.service === "docs" && !fixture.text?.includes(scenario.runTag)) errors.push(`${fixture.role} does not contain run tag`);
    if (fixture.service === "gmail" && !fixture.messages?.length) errors.push(`${fixture.role} has no message`);
  }
  if (scenario.taskId === "customer_meeting_prep") {
    const event = scenario.fixtures.find((fixture) => fixture.service === "calendar")?.events?.[0];
    if (!event || JSON.stringify(event).includes(scenario.runTag) === false) errors.push("customer meeting does not contain run tag");
  }
  return errors;
}

function expectedFacts(task: TaskSpec, random: SeededRandom): readonly string[] {
  const variant = random.int(0, 2);
  const common = ["all output resources include the scenario run tag", "no sent Gmail messages", "no Calendar attendee notifications"];
  const boundaries: Record<string, readonly string[]> = {
    support_inbox_triage: ["P0 applies to a broad outage", "queue sorting is deterministic"],
    sla_breach_pack: ["strict breach boundary is included", "two-hour warning boundary is included"],
    lead_follow_up_queue: ["ties sort by oldest contact then lead ID"],
    customer_meeting_prep: ["next matching meeting is within seven days"],
    new_hire_onboarding_pack: ["orientation starts at 09:00 local time"],
    preapproved_pto_processing: ["exclusive calendar end date", "holiday is excluded"],
    weekly_operating_review: ["seven-day overdue boundary is amber"],
    meeting_action_register: ["missing due date becomes TBD", "duplicate action is removed"],
    expense_policy_audit: ["receipt threshold includes 75", "duplicate invoice is flagged"],
    budget_variance_deck: ["zero budget uses N/A", "expense and revenue signs differ"],
  };
  return [...common, ...(boundaries[task.id] ?? []), `variant-${variant}`];
}

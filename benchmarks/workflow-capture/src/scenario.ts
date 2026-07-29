import { createHash } from "node:crypto";

import {
  addDays,
  generateContract,
  generateIncident,
  generateInvoices,
  generateLeads,
  isoDate,
  leadBand,
  leadScore,
  type Rng,
  wholeDaysBetween,
} from "./fixtures/prose.js";
import { SUPPORT_MESSAGES, SUPPORT_SENDERS } from "./fixtures/support.js";
import { taskById, TASKS } from "./tasks.js";
import type {
  GroundTruthRecord,
  RegisteredResource,
  Scenario,
  ScenarioFixture,
  ScenarioLane,
  TaskSpec,
} from "./types.js";

/** A tiny deterministic PRNG so fixtures do not depend on runtime entropy. */
export class SeededRandom implements Rng {
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
    if (maxInclusive <= min) return min;
    return min + (this.next() % (maxInclusive - min + 1));
  }

  pick<T>(values: readonly T[]): T {
    if (values.length === 0) throw new Error("cannot pick from an empty set");
    return values[this.int(0, values.length - 1)]!;
  }

  bool(): boolean {
    return this.next() % 2 === 0;
  }

  shuffle<T>(values: readonly T[]): T[] {
    const copy = [...values];
    for (let index = copy.length - 1; index > 0; index -= 1) {
      const target = this.int(0, index);
      [copy[index], copy[target]] = [copy[target]!, copy[index]!];
    }
    return copy;
  }

  sample<T>(values: readonly T[], count: number): T[] {
    return this.shuffle(values).slice(0, Math.min(count, values.length));
  }
}

export const BENCHMARK_AS_OF = "2026-07-13T09:00:00Z";

export function scenarioId(taskId: string, seed: number, lane: ScenarioLane, runNamespace = ""): string {
  const identity = runNamespace ? `${runNamespace}:${taskId}:${seed}:${lane}` : `${taskId}:${seed}:${lane}`;
  const digest = createHash("sha256").update(identity).digest("hex").slice(0, 10);
  return `${taskId}-${seed}-${lane}-${digest}`;
}

interface FixtureBuild {
  fixtures: ScenarioFixture[];
  groundTruth: GroundTruthRecord[];
  aggregates: Record<string, string>;
  facts: string[];
}

interface BuildContext {
  task: TaskSpec;
  runTag: string;
  asOf: Date;
  random: SeededRandom;
}

export function buildScenario(taskId: string, seed: number, lane: ScenarioLane, runNamespace = ""): Scenario {
  const task = taskById(taskId);
  const id = scenarioId(taskId, seed, lane, runNamespace);
  const runTag = `cori-bench-${id}`;
  // Fixture content is a pure function of the seed, so the direct and replay
  // twins of one seed receive the same problem while carrying different tags.
  const random = new SeededRandom(seed);
  const context: BuildContext = { task, runTag, asOf: new Date(BENCHMARK_AS_OF), random };
  const build = BUILDERS[taskId]?.(context) ?? genericBuild(context);
  const parameters = Object.fromEntries(
    task.parameters.map(({ name }) => [name, parameterValue(name, task, id, runTag)]),
  );
  const resources: RegisteredResource[] = build.fixtures.flatMap((fixture, index) =>
    Array.from({ length: fixtureResourceCount(fixture) }, (_, ordinal) => ({
      id: `pending-${index}-${ordinal}`,
      role: fixture.role,
      service: fixture.service,
      createdByBenchmark: true,
      fixtureIndex: index,
    }))
  );
  const scenario: Scenario = {
    id,
    taskId,
    seed,
    lane,
    runTag,
    parameters,
    fixtures: build.fixtures,
    expected: {
      facts: build.facts,
      rubric: task.rubric,
      groundTruth: build.groundTruth,
      aggregates: build.aggregates,
    },
    resources,
  };
  const fixtureErrors = validateScenarioFixtures(scenario);
  if (fixtureErrors.length > 0) throw new Error(`invalid ${taskId} fixture: ${fixtureErrors.join("; ")}`);
  return scenario;
}

export function fixtureResourceCount(fixture: ScenarioFixture): number {
  if (fixture.service === "gmail") return Math.max(1, fixture.messages?.length ?? 1);
  if (fixture.service === "docs") return Math.max(1, fixture.documents?.length ?? 1);
  return 1;
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
  if (name === "as_of") return BENCHMARK_AS_OF;
  if (name === "week_ending") return "2026-07-10";
  if (name === "period") return "2026-Q2";
  if (name === "gmail_query") return `label:inbox "${runTag}"`;
  if (name === "invoice_folder_query") {
    return `name contains 'Invoice ' and name contains '${runTag}' and trashed = false`;
  }
  return `pending-${task.id}-${name}-${id}`;
}

/* ------------------------------------------------------------- builders */

const BUILDERS: Readonly<Record<string, (context: BuildContext) => FixtureBuild>> = {
  support_inbox_triage: buildSupportInboxTriage,
  inbound_lead_qualification: buildInboundLeadQualification,
  vendor_invoice_intake: buildVendorInvoiceIntake,
  incident_postmortem_pack: buildIncidentPostmortemPack,
  contract_obligation_register: buildContractObligationRegister,
  sla_breach_pack: buildSlaBreachPack,
  expense_policy_audit: buildExpensePolicyAudit,
  budget_variance_deck: buildBudgetVarianceDeck,
  preapproved_pto_processing: buildPreapprovedPtoProcessing,
  weekly_operating_review: buildWeeklyOperatingReview,
};

function buildSupportInboxTriage({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const categories = ["outage", "access", "billing", "bug", "how_to"] as const;
  // Draw at least one message per category so the daily load always exercises
  // the whole policy, then fill the rest of a varying volume at random.
  const chosen = categories.map((category) =>
    random.pick(SUPPORT_MESSAGES.filter((message) => message.category === category))
  );
  const remaining = SUPPORT_MESSAGES.filter((message) => !chosen.includes(message));
  const live = random.shuffle([...chosen, ...random.sample(remaining, random.int(4, 9))]);
  const alreadyTriaged = random.sample(
    SUPPORT_MESSAGES.filter((message) => !live.includes(message)),
    random.int(2, 4),
  );
  const senders = random.sample(SUPPORT_SENDERS, live.length + alreadyTriaged.length);
  const entries = [
    ...live.map((template) => ({ template, pretriaged: false })),
    ...alreadyTriaged.map((template) => ({ template, pretriaged: true })),
  ];
  const ordered = random.shuffle(entries);
  const messages = ordered.map((entry, index) => ({
    subject: `[${runTag}] ${entry.template.subject}`,
    body: `${entry.template.body}\n\n-- \nReference ${runTag}`,
    from: senders[index] ?? SUPPORT_SENDERS[index % SUPPORT_SENDERS.length]!,
    // Spread arrivals across the night before as_of so received_at ordering is
    // a real tie-break rather than an artefact of identical timestamps.
    date: new Date(asOf.getTime() - random.int(30, 1_700) * 60_000).toISOString(),
    pretriaged: entry.pretriaged,
    unread: !entry.pretriaged,
  }));
  const groundTruth = ordered.map((entry, index) => ({
    key: `message:${index}`,
    fields: {
      category: entry.template.category,
      priority: entry.template.priority,
      skip: entry.pretriaged ? "true" : "false",
    },
  }));
  const triaged = groundTruth.filter((record) => record.fields.skip === "false");
  const aggregates: Record<string, string> = {
    triaged_count: String(triaged.length),
    skipped_count: String(groundTruth.length - triaged.length),
  };
  for (const category of categories) {
    aggregates[`category_${category}`] = String(
      triaged.filter((record) => record.fields.category === category).length,
    );
  }
  for (const priority of ["P0", "P1", "P2"]) {
    aggregates[`priority_${priority}`] = String(
      triaged.filter((record) => record.fields.priority === priority).length,
    );
  }
  return {
    fixtures: [
      {
        role: "support queue",
        service: "sheets",
        title: `${task.name} Queue ${runTag}`,
        table: [["benchmark_tag"], [runTag]],
      },
      {
        role: "support inbox",
        service: "gmail",
        title: `${task.name} Inbox ${runTag}`,
        messages,
      },
    ],
    groundTruth,
    aggregates,
    facts: [
      "category and priority are independent judgements",
      "messages already labelled triaged are left untouched",
      "queue ordering breaks ties on received_at then message_id",
    ],
  };
}

function buildInboundLeadQualification({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const leads = generateLeads(random, asOf, random.int(8, 12));
  const messages = leads.map((lead) => ({
    subject: `[${runTag}] ${lead.subject}`,
    body: `${lead.body}\n\n-- \nReference ${runTag}`,
    from: lead.sender,
    date: new Date(asOf.getTime() - random.int(60, 1_400) * 60_000).toISOString(),
    unread: true,
  }));
  const groundTruth = leads.map((lead, index) => {
    const score = leadScore(lead.seatCount, lead.timelineDays, lead.securityReview);
    return {
      key: `message:${index}`,
      fields: {
        company: lead.company,
        sender: lead.sender,
        seat_count: String(lead.seatCount),
        timeline_days: String(lead.timelineDays),
        security_review: String(lead.securityReview),
        score: String(score),
        band: leadBand(score),
      },
    };
  });
  const ranked = [...groundTruth].sort((left, right) =>
    Number(right.fields.score) - Number(left.fields.score) ||
    Number(right.fields.seat_count) - Number(left.fields.seat_count) ||
    left.key.localeCompare(right.key)
  );
  return {
    fixtures: [
      {
        role: "lead register",
        service: "sheets",
        title: `${task.name} Register ${runTag}`,
        table: [["benchmark_tag"], [runTag]],
      },
      {
        role: "inbound lead inbox",
        service: "gmail",
        title: `${task.name} Inbox ${runTag}`,
        messages,
      },
    ],
    groundTruth,
    aggregates: {
      lead_count: String(leads.length),
      top_sender: ranked[0]?.fields.sender ?? "",
      top_seat_count: ranked[0]?.fields.seat_count ?? "",
      top_timeline_days: ranked[0]?.fields.timeline_days ?? "",
      hot_count: String(groundTruth.filter((record) => record.fields.band === "hot").length),
    },
    facts: [
      "seat counts are stated in words, ranges, or sums of teams",
      "timelines are stated as fiscal or relative expressions",
      "each message contains numbers that are not the seat count",
    ],
  };
}

function buildVendorInvoiceIntake({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const invoices = generateInvoices(random, asOf, random.int(6, 9), runTag);
  const asOfDate = isoDate(asOf);
  const groundTruth = invoices.map((invoice, index) => ({
    key: `document:${index}`,
    fields: {
      vendor: invoice.vendor,
      invoice_number: invoice.invoiceNumber,
      currency: invoice.currency,
      net: invoice.net,
      tax: invoice.tax,
      gross: invoice.gross,
      due_date: invoice.dueDate,
      status: invoice.blocked
        ? "blocked"
        : invoice.dueDate < asOfDate
        ? "overdue"
        : "payable",
    },
  }));
  const counts = { blocked: 0, overdue: 0, payable: 0 };
  for (const record of groundTruth) counts[record.fields.status as keyof typeof counts] += 1;
  return {
    fixtures: [
      {
        role: "invoice register",
        service: "sheets",
        title: `${task.name} Register ${runTag}`,
        table: [["benchmark_tag"], [runTag]],
      },
      {
        role: "vendor invoices",
        service: "docs",
        title: `${task.name} Invoices ${runTag}`,
        documents: invoices.map((invoice) => ({
          title: `Invoice ${invoice.title}`,
          text: invoice.text,
        })),
      },
    ],
    groundTruth,
    aggregates: {
      blocked_count: String(counts.blocked),
      overdue_count: String(counts.overdue),
      payable_count: String(counts.payable),
      blocked_vendors: groundTruth
        .filter((record) => record.fields.status === "blocked")
        .map((record) => record.fields.invoice_number)
        .join(","),
    },
    facts: [
      "field labels, ordering, currency, and date format differ per vendor",
      "exactly one invoice does not balance and must not be corrected",
    ],
  };
}

function buildIncidentPostmortemPack({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const incident = generateIncident(random, runTag);
  const startedAt = new Date(asOf.getTime() - random.int(600, 2_400) * 60_000);
  const detectMinutes = random.int(3, 45);
  const mitigateMinutes = detectMinutes + random.int(10, 120);
  const resolveMinutes = mitigateMinutes + random.int(15, 240);
  const at = (minutes: number) => new Date(startedAt.getTime() + minutes * 60_000).toISOString();
  const groundTruth: GroundTruthRecord[] = [
    ...incident.confirmed.map((entry) => ({
      key: `factor:${entry.component}`,
      fields: { factor_id: entry.component, confirmed_by: entry.person, present: "true" },
    })),
    ...incident.ruledOut.map((component) => ({
      key: `factor:${component}`,
      fields: { factor_id: component, confirmed_by: "", present: "false" },
    })),
    {
      key: "timing:time_to_detect",
      fields: { metric: "time_to_detect", minutes: String(detectMinutes) },
    },
    {
      key: "timing:time_to_mitigate",
      fields: { metric: "time_to_mitigate", minutes: String(mitigateMinutes) },
    },
    {
      key: "timing:time_to_resolve",
      fields: { metric: "time_to_resolve", minutes: String(resolveMinutes) },
    },
  ];
  return {
    fixtures: [
      {
        role: "incident channel transcript",
        service: "docs",
        title: `${task.name} Transcript ${runTag}`,
        text: incident.text,
      },
      {
        role: "incident metrics",
        service: "sheets",
        title: `${task.name} Metrics ${runTag}`,
        table: [
          ["metric", "timestamp", "benchmark_tag"],
          ["started_at", startedAt.toISOString(), runTag],
          ["detected_at", at(detectMinutes), runTag],
          ["mitigated_at", at(mitigateMinutes), runTag],
          ["resolved_at", at(resolveMinutes), runTag],
        ],
      },
      {
        role: "postmortem findings",
        service: "sheets",
        title: `${task.name} Findings ${runTag}`,
        table: [["benchmark_tag"], [runTag]],
      },
    ],
    groundTruth,
    aggregates: {
      confirmed_count: String(incident.confirmed.length),
      time_to_detect: String(detectMinutes),
      time_to_mitigate: String(mitigateMinutes),
      time_to_resolve: String(resolveMinutes),
    },
    facts: [
      "the transcript is not in chronological order",
      "hypotheses the team ruled out must not appear as findings",
    ],
  };
}

function buildContractObligationRegister({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const contract = generateContract(random, asOf, runTag);
  const termEnd = new Date(`${contract.termEnd}T00:00:00Z`);
  const asOfDate = isoDate(asOf);
  const groundTruth = contract.obligations.map((obligation) => {
    const actBy = isoDate(addDays(termEnd, -obligation.noticeDays));
    return {
      key: `clause:${obligation.clause}`,
      fields: {
        clause: obligation.clause,
        party: obligation.party,
        notice_days: String(obligation.noticeDays),
        act_by: actBy,
        action_required: actBy <= asOfDate ? "true" : "false",
      },
    };
  });
  return {
    fixtures: [
      {
        role: "customer contract",
        service: "docs",
        title: `${task.name} Contract ${runTag}`,
        text: contract.text,
      },
      {
        role: "obligation register",
        service: "sheets",
        title: `${task.name} Register ${runTag}`,
        table: [["benchmark_tag"], [runTag]],
      },
    ],
    groundTruth,
    aggregates: {
      term_end: contract.termEnd,
      obligation_count: String(contract.obligations.length),
      action_required_count: String(
        groundTruth.filter((record) => record.fields.action_required === "true").length,
      ),
    },
    facts: [
      "at least one notice period is stated only by reference to another clause",
      "act_by is the term end minus the resolved notice period",
    ],
  };
}

function buildSlaBreachPack({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const targets: Readonly<Record<string, number>> = { P0: 1, P1: 4, P2: 24, P3: 72 };
  const priorities = ["P0", "P1", "P2", "P3"] as const;
  const rows: string[][] = [];
  const groundTruth: GroundTruthRecord[] = [];
  const count = random.int(6, 12);
  // Force the two decision boundaries to appear every run: one deadline exactly
  // at as_of and one exactly two hours after it.
  const boundaries = random.sample([0, 120], 2);
  for (let index = 0; index < count; index += 1) {
    const priority = random.pick(priorities);
    const closed = index > 0 && random.int(0, 5) === 0;
    const status = closed ? "closed" : random.bool() ? "open" : "in_progress";
    const offsetMinutes = index < boundaries.length
      ? boundaries[index]!
      : random.int(-2_800, 900);
    const deadline = new Date(asOf.getTime() + offsetMinutes * 60_000);
    const openedAt = new Date(deadline.getTime() - targets[priority]! * 3_600_000);
    const caseId = `CASE-${String(1_000 + index)}`;
    rows.push([
      caseId,
      status,
      priority,
      openedAt.toISOString(),
      `Case ${caseId} follow-up`,
      runTag,
    ]);
    if (closed) continue;
    groundTruth.push({
      key: `case:${caseId}`,
      fields: {
        case_id: caseId,
        status,
        priority,
        opened_at: openedAt.toISOString(),
        sla_deadline: deadline.toISOString(),
        breached: String(deadline.getTime() < asOf.getTime()),
        due_within_two_hours: String(
          deadline.getTime() >= asOf.getTime() &&
            deadline.getTime() <= asOf.getTime() + 2 * 3_600_000,
        ),
      },
    });
  }
  return {
    fixtures: [
      {
        role: "case register",
        service: "sheets",
        title: `${task.name} Cases ${runTag}`,
        table: [["case_id", "status", "priority", "opened_at", "subject", "benchmark_tag"], ...rows],
      },
      {
        role: "report template",
        service: "docs",
        title: `${task.name} Template ${runTag}`,
        text: "SLA Breach Pack\nBreached: {{BREACHED_COUNT}}\nDue within two hours: {{WARNING_COUNT}}\nRun tag: {{RUN_TAG}}",
      },
    ],
    groundTruth,
    aggregates: {
      breached_count: String(groundTruth.filter((record) => record.fields.breached === "true").length),
      warning_count: String(
        groundTruth.filter((record) => record.fields.due_within_two_hours === "true").length,
      ),
      open_count: String(groundTruth.length),
    },
    facts: ["the breach boundary is strict", "the two-hour warning boundary is inclusive"],
  };
}

function buildExpensePolicyAudit({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const rows: string[][] = [];
  const groundTruth: GroundTruthRecord[] = [];
  const count = random.int(8, 14);
  const duplicateInvoice = `INV-${random.int(5_000, 5_999)}`;
  const duplicatePair = random.sample(
    Array.from({ length: count }, (_, index) => index),
    2,
  );
  const records: {
    id: string;
    category: string;
    amount: number;
    receipt: boolean;
    nights: string;
    attendees: string;
    personal: boolean;
    invoiceId: string;
  }[] = [];
  for (let index = 0; index < count; index += 1) {
    const category = random.pick(["office", "hotel", "meal", "travel"]);
    const isDuplicate = duplicatePair.includes(index);
    const amount = category === "hotel"
      ? random.int(20_000, 80_000) / 100
      : category === "meal"
      ? random.int(3_000, 30_000) / 100
      : random.int(2_000, 40_000) / 100;
    records.push({
      id: `EXP-${String(100 + index)}`,
      category,
      amount,
      receipt: random.int(0, 2) > 0,
      nights: category === "hotel" ? String(random.int(1, 4)) : "",
      attendees: category === "meal" ? String(random.int(1, 6)) : "",
      personal: random.int(0, 6) === 0,
      invoiceId: isDuplicate ? duplicateInvoice : `INV-${String(1_000 + index)}`,
    });
  }
  // Guarantee each policy rule fires at least once so a run always scores the
  // whole rubric rather than whichever rules the draw happened to hit.
  const forced = random.sample(Array.from({ length: count }, (_, index) => index), 3);
  const [receiptRow, hotelRow, mealRow] = forced as [number, number, number];
  records[receiptRow]!.receipt = false;
  records[receiptRow]!.amount = Math.max(records[receiptRow]!.amount, 75);
  records[hotelRow]!.category = "hotel";
  records[hotelRow]!.nights = "2";
  records[hotelRow]!.attendees = "";
  records[hotelRow]!.amount = 620.5;
  records[mealRow]!.category = "meal";
  records[mealRow]!.attendees = "2";
  records[mealRow]!.nights = "";
  records[mealRow]!.amount = 148.75;
  for (const record of records) {
    const reasons: string[] = [];
    if (!record.receipt && record.amount >= 75) reasons.push("missing_receipt");
    if (record.category === "hotel" && Number(record.nights) > 0 && record.amount / Number(record.nights) > 250) {
      reasons.push("hotel_rate");
    }
    if (record.category === "meal" && Number(record.attendees) > 0 && record.amount / Number(record.attendees) > 60) {
      reasons.push("meal_per_person");
    }
    if (record.personal) reasons.push("personal");
    if (records.filter((other) => other.invoiceId === record.invoiceId).length > 1) {
      reasons.push("duplicate_invoice");
    }
    rows.push([
      record.id,
      record.category,
      record.amount.toFixed(2),
      String(record.receipt),
      record.nights,
      record.attendees,
      String(record.personal),
      record.invoiceId,
      runTag,
    ]);
    groundTruth.push({
      key: `expense:${record.id}`,
      fields: {
        expense_id: record.id,
        audit: reasons.length === 0 ? "PASS" : "FAIL",
        reasons: reasons.join(";"),
      },
    });
  }
  return {
    fixtures: [
      {
        role: "expense register",
        service: "sheets",
        title: `${task.name} Expenses ${runTag}`,
        table: [
          ["expense_id", "category", "amount", "receipt_present", "hotel_nights", "attendees", "personal", "invoice_id", "benchmark_tag"],
          ...rows,
        ],
      },
      {
        role: "audit template",
        service: "docs",
        title: `${task.name} Template ${runTag}`,
        text: "Expense Exceptions Report\nExceptions: {{EXCEPTION_COUNT}}\nReasons: {{REASONS}}\nRun tag: {{RUN_TAG}}",
      },
    ],
    groundTruth,
    aggregates: {
      exception_count: String(groundTruth.filter((record) => record.fields.audit === "FAIL").length),
      duplicate_invoice: duplicateInvoice,
    },
    facts: ["the receipt threshold includes 75", "a duplicated invoice fails every row that carries it"],
  };
}

function buildBudgetVarianceDeck({ task, runTag, random }: BuildContext): FixtureBuild {
  const period = "2026-Q2";
  const categories = random.sample(
    ["Cloud", "Travel", "Subscriptions", "Services", "Contractors", "Marketing", "Support", "Training", "Hardware", "Licences"],
    random.int(5, 9),
  );
  const rows: string[][] = [];
  const groundTruth: GroundTruthRecord[] = [];
  const zeroBudgetIndex = random.int(0, categories.length - 1);
  // Guarantee one unfavourable line of each type so the flag rule is always
  // exercised in both directions.
  const forcedExpense = random.int(0, categories.length - 1);
  const forcedRevenue = (forcedExpense + 1) % categories.length;
  categories.forEach((category, index) => {
    const type = index === forcedExpense
      ? "expense"
      : index === forcedRevenue
      ? "revenue"
      : random.bool()
      ? "expense"
      : "revenue";
    const budget = index === zeroBudgetIndex ? 0 : random.int(1_000, 40_000);
    let actual: number;
    if (budget === 0) {
      actual = random.int(100, 900);
    } else if (index === forcedExpense) {
      actual = Math.round(budget * (1.15 + random.int(0, 40) / 100));
    } else if (index === forcedRevenue) {
      actual = Math.round(budget * (0.85 - random.int(0, 30) / 100));
    } else {
      actual = Math.round(budget * (0.9 + random.int(0, 25) / 100));
    }
    const varianceAmount = actual - budget;
    const variancePercent = budget === 0
      ? "N/A"
      : (Math.round((varianceAmount / budget) * 1_000) / 10).toFixed(1);
    const unfavourable = variancePercent !== "N/A" &&
      (type === "expense" ? Number(variancePercent) > 10 : Number(variancePercent) < -10);
    const lineId = `BUD-${String(100 + index)}`;
    rows.push([lineId, type, category, String(budget), String(actual), period, runTag]);
    groundTruth.push({
      key: `line:${lineId}`,
      fields: {
        line_id: lineId,
        type,
        category,
        budget: String(budget),
        actual: String(actual),
        variance_amount: String(varianceAmount),
        variance_percent: variancePercent,
        unfavourable: String(unfavourable),
      },
    });
  });
  return {
    fixtures: [
      {
        role: "budget register",
        service: "sheets",
        title: `${task.name} Budget ${runTag}`,
        table: [["line_id", "type", "category", "budget", "actual", "period", "benchmark_tag"], ...rows],
      },
    ],
    groundTruth,
    aggregates: {
      unfavourable_count: String(
        groundTruth.filter((record) => record.fields.unfavourable === "true").length,
      ),
      line_count: String(groundTruth.length),
    },
    facts: ["a zero budget yields N/A", "expense and revenue lines flag in opposite directions"],
  };
}

function buildPreapprovedPtoProcessing({ task, runTag, asOf, random }: BuildContext): FixtureBuild {
  const employees = random.sample(
    [
      ["Riley Martin", "riley@example.test"],
      ["Sam Okoro", "sam@example.test"],
      ["Noa Levi", "noa@example.test"],
      ["Ines Duarte", "ines@example.test"],
    ],
    random.int(1, 3),
  );
  const holidayCount = random.int(1, 3);
  const holidays: string[] = [];
  for (let index = 0; index < holidayCount; index += 1) {
    holidays.push(isoDate(addDays(asOf, random.int(1, 25))));
  }
  const uniqueHolidays = [...new Set(holidays)];
  const rows: string[][] = [];
  const groundTruth: GroundTruthRecord[] = [];
  employees.forEach(([name, email], index) => {
    const start = addDays(asOf, random.int(1, 20));
    const end = addDays(start, random.int(2, 12));
    const balance = random.int(10, 30);
    let businessDays = 0;
    for (let day = new Date(start); day <= end; day = addDays(day, 1)) {
      const weekday = day.getUTCDay();
      if (weekday === 0 || weekday === 6) continue;
      if (uniqueHolidays.includes(isoDate(day))) continue;
      businessDays += 1;
    }
    const requestId = `PTO-${String(100 + index)}`;
    rows.push([
      "request", requestId, "approved", "true", name!, email!,
      isoDate(start), isoDate(end), String(balance), "", "", runTag,
    ]);
    groundTruth.push({
      key: `request:${requestId}`,
      fields: {
        request_id: requestId,
        employee_email: email!,
        start_date: isoDate(start),
        end_date: isoDate(end),
        business_days: String(businessDays),
        pto_balance_days: String(balance - businessDays),
        event_start: isoDate(start),
        event_end: isoDate(addDays(end, 1)),
      },
    });
  });
  uniqueHolidays.forEach((holiday, index) => {
    rows.push(["holiday", `HOL-${String(100 + index)}`, "", "", "", "", "", "", "", holiday, "", runTag]);
  });
  return {
    fixtures: [
      {
        role: "PTO register",
        service: "sheets",
        title: `${task.name} Register ${runTag}`,
        table: [
          ["row_type", "request_id", "status", "manager_approved", "employee_name", "employee_email", "start_date", "end_date", "pto_balance_days", "holiday_date", "business_days", "benchmark_tag"],
          ...rows,
        ],
      },
      { role: "PTO calendar", service: "calendar", title: `${task.name} Calendar ${runTag}`, events: [] },
    ],
    groundTruth,
    aggregates: { request_count: String(employees.length), holiday_count: String(uniqueHolidays.length) },
    facts: ["all-day event end dates are exclusive", "company holidays are excluded from the count"],
  };
}

function buildWeeklyOperatingReview({ task, runTag, random }: BuildContext): FixtureBuild {
  const owners = ["Alice", "Bob", "Carol", "Devon", "Elliot", "Frankie", "Gwen", "Hugo"];
  const count = random.int(6, 12);
  const rows: string[][] = [];
  const groundTruth: GroundTruthRecord[] = [];
  // Pin one project onto each side of the amber boundary so the run always
  // distinguishes 7 days overdue from 6.
  const forced = random.sample(Array.from({ length: count }, (_, index) => index), 3);
  for (let index = 0; index < count; index += 1) {
    let blocked = random.int(0, 5) === 0;
    let daysOverdue = random.int(0, 20);
    let progress = random.int(30, 100);
    if (index === forced[0]) { blocked = false; daysOverdue = 7; progress = 90; }
    if (index === forced[1]) { blocked = false; daysOverdue = 6; progress = 85; }
    if (index === forced[2]) { blocked = true; daysOverdue = 0; progress = 99; }
    const rag = blocked || daysOverdue > 14 || progress < 50
      ? "red"
      : daysOverdue >= 7 || progress < 80
      ? "amber"
      : "green";
    const projectId = `PROJ-${String(100 + index)}`;
    const owner = owners[index % owners.length]!;
    rows.push([projectId, String(blocked), String(daysOverdue), String(progress), owner, runTag]);
    groundTruth.push({
      key: `project:${projectId}`,
      fields: {
        project_id: projectId,
        rag,
        escalation: String(rag === "red"),
        owner,
      },
    });
  }
  return {
    fixtures: [
      {
        role: "project register",
        service: "sheets",
        title: `${task.name} Projects ${runTag}`,
        table: [["project_id", "blocked", "days_overdue", "progress_percent", "owner", "benchmark_tag"], ...rows],
      },
      {
        role: "review template",
        service: "docs",
        title: `${task.name} Template ${runTag}`,
        text: "Weekly Operating Review {{WEEK_ENDING}}\nGreen: {{GREEN_COUNT}}\nAmber: {{AMBER_COUNT}}\nRed: {{RED_COUNT}}\nEscalations: {{ESCALATIONS}}\nRun tag: {{RUN_TAG}}",
      },
    ],
    groundTruth,
    aggregates: {
      red_count: String(groundTruth.filter((record) => record.fields.rag === "red").length),
      amber_count: String(groundTruth.filter((record) => record.fields.rag === "amber").length),
      green_count: String(groundTruth.filter((record) => record.fields.rag === "green").length),
    },
    facts: ["seven days overdue is amber", "a blocked project is red regardless of progress"],
  };
}

function genericBuild({ task, runTag }: BuildContext): FixtureBuild {
  return {
    fixtures: task.resources.map((resource) => ({
      role: resource.role,
      service: resource.service,
      title: `${task.name} ${resource.role} ${runTag}`,
      table: resource.service === "sheets" ? [["benchmark_tag"], [runTag]] : undefined,
      text: resource.service === "docs" ? `Synthetic ${task.name} source.\nTag: ${runTag}` : undefined,
    })),
    groundTruth: [],
    aggregates: {},
    facts: [],
  };
}

/* ------------------------------------------------------------ validation */

export function validateScenarioFixtures(scenario: Scenario): readonly string[] {
  const errors: string[] = [];
  const task = taskById(scenario.taskId);
  for (const fixture of scenario.fixtures) {
    if (fixture.service === "sheets" && (!fixture.table || fixture.table.length < 1)) {
      errors.push(`${fixture.role} has no table`);
    }
    if (fixture.service === "docs") {
      const documents = fixture.documents ?? (fixture.text ? [{ title: fixture.title, text: fixture.text }] : []);
      if (documents.length === 0) errors.push(`${fixture.role} has no document text`);
    }
    if (fixture.service === "gmail" && !fixture.messages?.length) {
      errors.push(`${fixture.role} has no message`);
    }
  }
  if (task.requiresRuntimeModel && scenario.expected.groundTruth.length === 0) {
    errors.push("a hybrid task must publish ground truth for grading");
  }
  if (task.rerunContract) {
    const skipped = scenario.expected.groundTruth.filter((record) => record.fields.skip === "true");
    if (skipped.length === 0) {
      errors.push("a re-run task must include state from a previous run");
    }
  }
  return errors;
}

/**
 * Reject any fixture set where one literal decides a class.
 *
 * For every token appearing in the generated text and every class label, it
 * must not be the case that the token appears in every member of that class and
 * in no member of any other class. A bank that fails this check can be scored
 * by a keyword matcher, which would let a `code` step stand in for the
 * understanding the task is meant to measure.
 */
export function assertRegexResistant(
  labelled: readonly { text: string; label: string }[],
  context: string,
): void {
  const byLabel = new Map<string, string[]>();
  for (const entry of labelled) {
    byLabel.set(entry.label, [...(byLabel.get(entry.label) ?? []), entry.text.toLowerCase()]);
  }
  if (byLabel.size < 2) return;
  const tokenSets = labelled.map((entry) => tokensOf(entry.text));
  const allTokens = new Set(tokenSets.flatMap((tokens) => [...tokens]));
  for (const [label, texts] of byLabel) {
    // The label word itself must never appear in the text it labels.
    for (const text of texts) {
      if (tokensOf(text).has(label.toLowerCase())) {
        throw new Error(`${context}: the label "${label}" appears verbatim in its own source text`);
      }
    }
    const members = labelled.flatMap((entry, index) => entry.label === label ? [tokenSets[index]!] : []);
    const outsiders = labelled.flatMap((entry, index) => entry.label === label ? [] : [tokenSets[index]!]);
    for (const token of allTokens) {
      const inAll = members.every((tokens) => tokens.has(token));
      const inNoneOutside = outsiders.every((tokens) => !tokens.has(token));
      if (inAll && inNoneOutside) {
        throw new Error(
          `${context}: the single token "${token}" separates class "${label}" from every other class; a keyword matcher would score this fixture`,
        );
      }
    }
  }
}

function tokensOf(text: string): Set<string> {
  return new Set(
    text.toLowerCase().split(/[^\p{L}\p{N}_]+/u).filter((token) => token.length >= 4),
  );
}

/** Every hybrid task's own bank, checked offline by `benchmark validate`. */
export function assertHybridBanksAreRegexResistant(): void {
  assertRegexResistant(
    SUPPORT_MESSAGES.map((message) => ({
      text: `${message.subject} ${message.body}`,
      label: message.category,
    })),
    "support category bank",
  );
  assertRegexResistant(
    SUPPORT_MESSAGES.map((message) => ({
      text: `${message.subject} ${message.body}`,
      label: message.priority,
    })),
    "support priority bank",
  );
  for (const task of TASKS.filter((candidate) => candidate.requiresRuntimeModel)) {
    for (const seed of [11, 12, 13]) {
      const scenario = buildScenario(task.id, seed, "author", "regex-resistance");
      if (scenario.expected.groundTruth.length === 0) {
        throw new Error(`${task.id} produced no ground truth at seed ${seed}`);
      }
    }
  }
}

/** Distinct seeds must produce genuinely different problems, not just tags. */
export function assertSeedsProduceDistinctFixtures(taskId: string, seeds: readonly number[]): void {
  const seen = new Map<string, number>();
  for (const seed of seeds) {
    const scenario = buildScenario(taskId, seed, "author", "seed-variation");
    const signature = JSON.stringify(scenario.expected.groundTruth);
    const previous = seen.get(signature);
    if (previous !== undefined) {
      throw new Error(
        `${taskId} produces identical ground truth for seeds ${previous} and ${seed}; held-out trials would repeat one problem`,
      );
    }
    seen.set(signature, seed);
  }
}

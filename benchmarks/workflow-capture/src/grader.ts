import type { GroundTruthRecord, Grade, Json, Scenario, WorkspaceSnapshot } from "./types.js";

/**
 * Deterministic external-state grader. It receives only Workspace snapshots:
 * never model text, tool exit status, or Cori activity output.
 *
 * Every expected value comes from `scenario.expected`, which the fixture
 * generator derived for this seed. Nothing in this file knows the answer to any
 * particular run, so changing a fixture cannot silently leave the grader
 * checking a stale constant.
 */
export function gradeExternalState(scenario: Scenario, before: WorkspaceSnapshot, after: WorkspaceSnapshot): Grade {
  const beforeText = stableText(snapshotState(before));
  const afterText = stableText(snapshotState(after));
  const violations: string[] = [];
  const sent = after.resources[`__sent_${scenario.id}`];
  if (hasValues(sent)) violations.push("a tagged Gmail message has the SENT label");
  if (afterText.includes("sendUpdates\":\"all")) violations.push("Calendar state records sendUpdates=all");
  const changed = beforeText !== afterText;
  const tagsPresent = afterText.includes(scenario.runTag);
  const draftsExist = hasValues(after.resources[`__drafts_${scenario.id}`]) || after.drafts.some(hasValues);
  const context: GradeContext = {
    scenario,
    after,
    output: taggedOutputText(scenario, after),
    drafts: stableText(after.drafts),
    calendar: stableText(after.calendarEvents),
    truth: new Map(scenario.expected.groundTruth.map((record) => [record.key, record])),
  };
  const items = scenario.expected.rubric.map((item) => {
    const matches = itemMatches(context, item.id);
    const passed = changed && tagsPresent && (item.id !== "draft" || draftsExist) && matches;
    return {
      id: item.id,
      earned: passed && violations.length === 0 ? item.points : 0,
      max: item.points,
      note: passed
        ? "verified against scenario ground truth"
        : `no Workspace evidence for ${item.id} matching this scenario's expected values`,
    };
  });
  const score = violations.length > 0 ? 0 : items.reduce((sum, item) => sum + item.earned, 0);
  return { score, passed: score >= 90 && violations.length === 0, safetyViolations: violations, items };
}

function field(record: GroundTruthRecord, name: string): string {
  return record.fields[name] ?? "";
}

interface GradeContext {
  scenario: Scenario;
  after: WorkspaceSnapshot;
  output: string;
  drafts: string;
  calendar: string;
  truth: Map<string, GroundTruthRecord>;
}

function itemMatches(context: GradeContext, itemId: string): boolean {
  const grader = GRADERS[context.scenario.taskId];
  if (!grader) return false;
  return grader(context, itemId) === true;
}

type TaskGrader = (context: GradeContext, itemId: string) => boolean;

const GRADERS: Readonly<Record<string, TaskGrader>> = {
  support_inbox_triage: gradeSupportInboxTriage,
  inbound_lead_qualification: gradeInboundLeadQualification,
  vendor_invoice_intake: gradeVendorInvoiceIntake,
  incident_postmortem_pack: gradeIncidentPostmortemPack,
  contract_obligation_register: gradeContractObligationRegister,
  sla_breach_pack: gradeSlaBreachPack,
  expense_policy_audit: gradeExpensePolicyAudit,
  budget_variance_deck: gradeBudgetVarianceDeck,
  preapproved_pto_processing: gradePreapprovedPtoProcessing,
  weekly_operating_review: gradeWeeklyOperatingReview,
};

/* -------------------------------------------------------------- support */

const SUPPORT_QUEUE_HEADERS = [
  "message_id", "received_at", "sender", "subject", "category", "priority", "status", "run_tag", "as_of",
] as const;

function gradeSupportInboxTriage(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const liveIds = liveIdsFor(scenario, "gmail");
  const queue = findExactTable(context.after, SUPPORT_QUEUE_HEADERS);
  const rows = tableObjects(queue);
  const expected = scenario.expected.groundTruth.map((record, ordinal) => ({
    record,
    messageId: liveIds[ordinal] ?? "",
    skip: field(record, "skip") === "true",
  }));
  const triaged = expected.filter((entry) => !entry.skip);
  const skipped = expected.filter((entry) => entry.skip);
  if (itemId === "classification") {
    if (rows.length !== triaged.length) return false;
    return triaged.every((entry) => {
      const row = rows.find((candidate) => candidate.message_id === entry.messageId);
      return row?.category === field(entry.record, "category") &&
        row?.priority === field(entry.record, "priority");
    });
  }
  if (itemId === "idempotence") {
    const labelNames = labelNamesById(context.after, scenario);
    const touched = skipped.some((entry) => {
      if (rows.some((row) => row.message_id === entry.messageId)) return true;
      const applied = messageLabelNames(context.after, entry.messageId, labelNames);
      return applied.some((name) =>
        name.startsWith(`${scenario.runTag}/category/`) ||
        name.startsWith(`${scenario.runTag}/priority/`)
      );
    });
    return skipped.length > 0 && !touched;
  }
  if (itemId === "queue") {
    if (rows.length !== triaged.length) return false;
    const order = { P0: 0, P1: 1, P2: 2 } as const;
    for (let index = 1; index < rows.length; index += 1) {
      const previous = rows[index - 1]!;
      const current = rows[index]!;
      const rank = (row: Record<string, string>) =>
        order[row.priority as keyof typeof order] ?? 9;
      if (rank(previous) > rank(current)) return false;
      if (rank(previous) === rank(current)) {
        const left = Date.parse(previous.received_at ?? "");
        const right = Date.parse(current.received_at ?? "");
        if (Number.isFinite(left) && Number.isFinite(right)) {
          if (left > right) return false;
          if (left === right && (previous.message_id ?? "") > (current.message_id ?? "")) return false;
        }
      }
    }
    return rows.every((row) =>
      row.status === "triaged" &&
      row.run_tag === scenario.runTag &&
      row.as_of === scenario.parameters.as_of &&
      (row.sender ?? "").includes("@")
    );
  }
  if (itemId === "gmail") {
    const labelNames = labelNamesById(context.after, scenario);
    return triaged.length > 0 && triaged.every((entry) => {
      const applied = messageLabelNames(context.after, entry.messageId, labelNames);
      return applied.includes(`${scenario.runTag}/category/${field(entry.record, "category")}`) &&
        applied.includes(`${scenario.runTag}/priority/${field(entry.record, "priority")}`) &&
        !messageLabelIds(context.after, entry.messageId).includes("UNREAD");
    });
  }
  if (itemId === "draft") {
    const counts = scenario.expected.aggregates;
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      ["outage", "access", "billing", "bug", "how_to"].every((category) =>
        draftMentionsCount(context.drafts, category, counts[`category_${category}`] ?? "")
      ) &&
      ["P0", "P1", "P2"].every((priority) =>
        draftMentionsCount(context.drafts, priority.toLowerCase(), counts[`priority_${priority}`] ?? "")
      );
  }
  return false;
}

/* ---------------------------------------------------------------- sales */

const LEAD_HEADERS = [
  "message_id", "sender", "company", "seat_count", "timeline_days", "security_review", "score", "band", "run_tag", "as_of",
] as const;

function gradeInboundLeadQualification(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const liveIds = liveIdsFor(scenario, "gmail");
  const rows = tableObjects(findExactTable(context.after, LEAD_HEADERS));
  const expected = scenario.expected.groundTruth.map((record, ordinal) => ({
    record,
    messageId: liveIds[ordinal] ?? "",
  }));
  if (rows.length !== expected.length) return false;
  const rowFor = (messageId: string) => rows.find((row) => row.message_id === messageId);
  if (itemId === "extraction") {
    return expected.every((entry) => {
      const row = rowFor(entry.messageId);
      return row !== undefined &&
        numbersEqual(row.seat_count, field(entry.record, "seat_count")) &&
        numbersEqual(row.timeline_days, field(entry.record, "timeline_days")) &&
        booleansEqual(row.security_review, field(entry.record, "security_review"));
    });
  }
  if (itemId === "scoring") {
    return expected.every((entry) => {
      const row = rowFor(entry.messageId);
      return row !== undefined &&
        numbersEqual(row.score, field(entry.record, "score")) &&
        row.band?.toLowerCase() === field(entry.record, "band");
    });
  }
  if (itemId === "ordering") {
    for (let index = 1; index < rows.length; index += 1) {
      const previous = rows[index - 1]!;
      const current = rows[index]!;
      const leftScore = Number(previous.score);
      const rightScore = Number(current.score);
      if (leftScore < rightScore) return false;
      if (leftScore === rightScore) {
        const leftSeats = Number(previous.seat_count);
        const rightSeats = Number(current.seat_count);
        if (leftSeats < rightSeats) return false;
      }
    }
    return true;
  }
  if (itemId === "sheet") {
    return expected.every((entry) => {
      const row = rowFor(entry.messageId);
      return row !== undefined &&
        row.run_tag === scenario.runTag &&
        row.as_of === scenario.parameters.as_of &&
        (row.company ?? "").length > 0 &&
        row.sender === field(entry.record, "sender");
    });
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      context.drafts.includes((aggregates.top_sender ?? "").toLowerCase()) &&
      containsNumber(context.drafts, aggregates.top_seat_count ?? "") &&
      containsNumber(context.drafts, aggregates.top_timeline_days ?? "");
  }
  return false;
}

/* -------------------------------------------------------------- finance */

const INVOICE_HEADERS = [
  "document_id", "vendor", "invoice_number", "currency", "net", "tax", "gross", "due_date", "status", "run_tag", "as_of",
] as const;

function gradeVendorInvoiceIntake(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const liveIds = liveIdsFor(scenario, "docs");
  const rows = tableObjects(findExactTable(context.after, INVOICE_HEADERS));
  const expected = scenario.expected.groundTruth.map((record, ordinal) => ({
    record,
    documentId: liveIds[ordinal] ?? "",
  }));
  if (rows.length !== expected.length) return false;
  const rowFor = (documentId: string) => rows.find((row) => row.document_id === documentId);
  if (itemId === "extraction") {
    return expected.every((entry) => {
      const row = rowFor(entry.documentId);
      return row !== undefined &&
        row.vendor === field(entry.record, "vendor") &&
        row.invoice_number === field(entry.record, "invoice_number") &&
        row.currency?.toUpperCase() === field(entry.record, "currency") &&
        numbersEqual(row.net, field(entry.record, "net")) &&
        numbersEqual(row.tax, field(entry.record, "tax")) &&
        numbersEqual(row.gross, field(entry.record, "gross")) &&
        row.due_date === field(entry.record, "due_date");
    });
  }
  if (itemId === "reconciliation") {
    const blocked = expected.filter((entry) => field(entry.record, "status") === "blocked");
    return blocked.length > 0 && blocked.every((entry) => {
      const row = rowFor(entry.documentId);
      // The figures must be recorded exactly as written, imbalance and all.
      return row?.status === "blocked" &&
        numbersEqual(row.gross, field(entry.record, "gross")) &&
        numbersEqual(row.net, field(entry.record, "net")) &&
        numbersEqual(row.tax, field(entry.record, "tax"));
    });
  }
  if (itemId === "status") {
    return expected.every((entry) => rowFor(entry.documentId)?.status === field(entry.record, "status"));
  }
  if (itemId === "sheet") {
    const rank = { blocked: 0, overdue: 1, payable: 2 } as const;
    for (let index = 1; index < rows.length; index += 1) {
      const left = rank[rows[index - 1]!.status as keyof typeof rank] ?? 9;
      const right = rank[rows[index]!.status as keyof typeof rank] ?? 9;
      if (left > right) return false;
    }
    return rows.every((row) => row.run_tag === scenario.runTag && row.as_of === scenario.parameters.as_of);
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    const blockedNumbers = (aggregates.blocked_vendors ?? "").split(",").filter(Boolean);
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      blockedNumbers.every((number) => context.drafts.includes(number.toLowerCase())) &&
      containsNumber(context.drafts, aggregates.blocked_count ?? "");
  }
  return false;
}

/* ---------------------------------------------------------- engineering */

function gradeIncidentPostmortemPack(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const factors = scenario.expected.groundTruth.filter((record) => record.key.startsWith("factor:"));
  const timings = scenario.expected.groundTruth.filter((record) => record.key.startsWith("timing:"));
  const factorRows = tableObjects(findExactTable(context.after, ["factor_id", "summary", "confirmed_by", "run_tag"]));
  const timingRows = tableObjects(findExactTable(context.after, ["metric", "minutes", "run_tag"]));
  const confirmed = factors.filter((record) => field(record, "present") === "true");
  const ruledOut = factors.filter((record) => field(record, "present") === "false");
  if (itemId === "factors") {
    return factorRows.length === confirmed.length &&
      confirmed.every((record) =>
        factorRows.some((row) =>
          row.factor_id === field(record, "factor_id") && (row.summary ?? "").length > 0
        )
      );
  }
  if (itemId === "exclusion") {
    const everything = `${stableText(factorRows)} ${context.output}`.toLowerCase();
    return ruledOut.length > 0 &&
      ruledOut.every((record) => !everything.includes(field(record, "factor_id").toLowerCase()));
  }
  if (itemId === "attribution") {
    return confirmed.every((record) =>
      factorRows.some((row) =>
        row.factor_id === field(record, "factor_id") &&
        row.confirmed_by?.toLowerCase() === field(record, "confirmed_by").toLowerCase()
      )
    );
  }
  if (itemId === "timings") {
    return timingRows.length === timings.length &&
      timings.every((record) =>
        timingRows.some((row) =>
          row.metric === field(record, "metric") && numbersEqual(row.minutes, field(record, "minutes"))
        )
      );
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      ["time_to_detect", "time_to_mitigate", "time_to_resolve"].every((metric) =>
        containsNumber(context.drafts, aggregates[metric] ?? "")
      ) &&
      containsNumber(context.drafts, aggregates.confirmed_count ?? "");
  }
  return false;
}

/* ---------------------------------------------------------------- legal */

const OBLIGATION_HEADERS = [
  "clause", "party", "obligation", "notice_days", "act_by", "action_required", "run_tag", "as_of",
] as const;

function gradeContractObligationRegister(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const rows = tableObjects(findExactTable(context.after, OBLIGATION_HEADERS));
  if (rows.length !== expected.length) return false;
  const rowFor = (clause: string) =>
    rows.find((row) => normalizeClause(row.clause ?? "") === normalizeClause(clause));
  if (itemId === "obligations") {
    return expected.every((record) => {
      const row = rowFor(field(record, "clause"));
      return row !== undefined &&
        row.party?.toLowerCase() === field(record, "party").toLowerCase() &&
        (row.obligation ?? "").length > 0;
    });
  }
  if (itemId === "references") {
    return expected.every((record) =>
      numbersEqual(rowFor(field(record, "clause"))?.notice_days, field(record, "notice_days"))
    );
  }
  if (itemId === "dates") {
    return expected.every((record) => {
      const row = rowFor(field(record, "clause"));
      return row?.act_by === field(record, "act_by") &&
        booleansEqual(row?.action_required, field(record, "action_required"));
    });
  }
  if (itemId === "sheet") {
    for (let index = 1; index < rows.length; index += 1) {
      if ((rows[index - 1]!.act_by ?? "") > (rows[index]!.act_by ?? "")) return false;
    }
    return rows.every((row) => row.run_tag === scenario.runTag && row.as_of === scenario.parameters.as_of);
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    const due = expected.filter((record) => field(record, "action_required") === "true");
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.drafts, aggregates.obligation_count ?? "") &&
      due.every((record) => context.drafts.includes(field(record, "clause").toLowerCase()));
  }
  return false;
}

function normalizeClause(value: string): string {
  return value.toLowerCase().replace(/\s+/gu, " ").trim();
}

/* ------------------------------------------------------- deterministic */

const SLA_HEADERS = [
  "case_id", "status", "priority", "opened_at", "sla_deadline", "breached", "due_within_two_hours", "run_tag",
] as const;

function gradeSlaBreachPack(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const rows = tableObjects(findExactTable(context.after, SLA_HEADERS));
  const rowFor = (caseId: string) => rows.find((row) => row.case_id === caseId);
  if (itemId === "sla") {
    return rows.length === expected.length && expected.every((record) => {
      const row = rowFor(field(record, "case_id"));
      return row !== undefined &&
        instantsEqual(row.sla_deadline, field(record, "sla_deadline")) &&
        booleansEqual(row.breached, field(record, "breached")) &&
        booleansEqual(row.due_within_two_hours, field(record, "due_within_two_hours"));
    });
  }
  if (itemId === "sheet") {
    if (rows.length !== expected.length) return false;
    for (let index = 1; index < rows.length; index += 1) {
      const left = Date.parse(rows[index - 1]!.sla_deadline ?? "");
      const right = Date.parse(rows[index]!.sla_deadline ?? "");
      if (Number.isFinite(left) && Number.isFinite(right) && left > right) return false;
    }
    return rows.every((row) => row.run_tag === scenario.runTag);
  }
  if (itemId === "doc") {
    const aggregates = scenario.expected.aggregates;
    return context.output.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.output, aggregates.breached_count ?? "") &&
      containsNumber(context.output, aggregates.warning_count ?? "") &&
      !context.output.includes("{{");
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.drafts, aggregates.breached_count ?? "");
  }
  return false;
}

function gradeExpensePolicyAudit(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const rows = tableObjects(findExactTable(context.after, ["expense_id", "audit", "reasons", "run_tag"]));
  const rowFor = (id: string) => rows.find((row) => row.expense_id === id);
  if (itemId === "findings") {
    return rows.length === expected.length && expected.every((record) => {
      const row = rowFor(field(record, "expense_id"));
      if (!row) return false;
      const reasons = (field(record, "reasons") ? field(record, "reasons").split(";") : []).sort();
      const actual = (row.reasons ? row.reasons.split(";").map((value) => value.trim()) : [])
        .filter(Boolean).sort();
      return row.audit?.toUpperCase() === field(record, "audit") &&
        reasons.length === actual.length &&
        reasons.every((reason, index) => reason === actual[index]);
    });
  }
  if (itemId === "duplicates") {
    const duplicates = expected.filter((record) => field(record, "reasons").includes("duplicate_invoice"));
    return duplicates.length > 0 &&
      duplicates.every((record) => rowFor(field(record, "expense_id"))?.reasons?.includes("duplicate_invoice") === true);
  }
  if (itemId === "report") {
    return context.output.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.output, scenario.expected.aggregates.exception_count ?? "") &&
      !context.output.includes("{{");
  }
  if (itemId === "draft") {
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.drafts, scenario.expected.aggregates.exception_count ?? "");
  }
  return false;
}

function gradeBudgetVarianceDeck(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const presentation = taggedPresentation(scenario, context.after);
  const deckText = normalizeFigures(presentation?.text ?? "");
  if (itemId === "variance") {
    return expected.every((record) => {
      const category = field(record, "category").toLowerCase();
      if (field(record, "variance_percent") === "N/A") {
        return categoryWindowMatches(deckText, category, /\bn\s*\/\s*a\b/u);
      }
      return categoryWindowMatches(
        deckText,
        category,
        figurePattern(field(record, "variance_amount")),
        figurePattern(field(record, "variance_percent")),
      );
    });
  }
  if (itemId === "flags") {
    const unfavourable = expected.filter((record) => field(record, "unfavourable") === "true");
    const favourable = expected.filter((record) => field(record, "unfavourable") === "false");
    // Anchor on the slide title, not the word: the summary slide also counts
    // unfavourable lines, and the detail slide lists every category.
    const slide = normalizeFigures(slideText(presentation, /unfavou?rable variances/u) ?? "");
    return unfavourable.length > 0 &&
      unfavourable.every((record) => slide.includes(field(record, "category").toLowerCase())) &&
      favourable.every((record) => !slide.includes(field(record, "category").toLowerCase()));
  }
  if (itemId === "deck") {
    return presentation?.slides === 3 &&
      ["executive summary", "detail"].every((title) => deckText.includes(title)) &&
      /unfavou?rable variances/u.test(deckText) &&
      deckText.includes(scenario.runTag.toLowerCase());
  }
  if (itemId === "draft") {
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      containsNumber(context.drafts, scenario.expected.aggregates.unfavourable_count ?? "");
  }
  return false;
}

function gradePreapprovedPtoProcessing(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const register = findTable(context.after, ["row_type", "request_id", "status", "pto_balance_days", "business_days"]);
  if (itemId === "days") {
    return expected.every((record) =>
      numbersEqual(tableCell(register, "request_id", field(record, "request_id"), "business_days"), field(record, "business_days"))
    );
  }
  if (itemId === "balance") {
    return expected.every((record) =>
      numbersEqual(tableCell(register, "request_id", field(record, "request_id"), "pto_balance_days"), field(record, "pto_balance_days")) &&
      tableCell(register, "request_id", field(record, "request_id"), "status")?.toLowerCase() === "scheduled"
    );
  }
  if (itemId === "calendar") {
    const events = calendarItems(context.after).filter((event) =>
      stableText(event).includes(scenario.runTag.toLowerCase())
    );
    if (events.length !== expected.length) return false;
    return expected.every((record) =>
      events.some((event) => {
        const start = objectValue(event.start);
        const end = objectValue(event.end);
        return (stringValue(event.eventType) ?? "default") === "default" &&
          stringValue(start?.date) === field(record, "event_start") &&
          stringValue(end?.date) === field(record, "event_end");
      })
    );
  }
  if (itemId === "draft") {
    return draftCount(context) === expected.length &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      expected.every((record) =>
        context.drafts.includes(field(record, "employee_email").toLowerCase()) &&
        containsNumber(context.drafts, field(record, "business_days"))
      );
  }
  return false;
}

function gradeWeeklyOperatingReview(context: GradeContext, itemId: string): boolean {
  const { scenario } = context;
  const expected = scenario.expected.groundTruth;
  const rows = tableObjects(findExactTable(context.after, ["project_id", "rag", "escalation", "owner", "run_tag"]));
  const rowFor = (id: string) => rows.find((row) => row.project_id === id);
  if (itemId === "rag") {
    return rows.length === expected.length && expected.every((record) =>
      rowFor(field(record, "project_id"))?.rag?.toLowerCase() === field(record, "rag")
    );
  }
  if (itemId === "aggregates") {
    const rank = { red: 0, amber: 1, green: 2 } as const;
    for (let index = 1; index < rows.length; index += 1) {
      const left = rank[rows[index - 1]!.rag?.toLowerCase() as keyof typeof rank] ?? 9;
      const right = rank[rows[index]!.rag?.toLowerCase() as keyof typeof rank] ?? 9;
      if (left > right) return false;
    }
    return expected.every((record) =>
      booleansEqual(rowFor(field(record, "project_id"))?.escalation, field(record, "escalation"))
    );
  }
  if (itemId === "doc") {
    const aggregates = scenario.expected.aggregates;
    return context.output.includes(scenario.runTag.toLowerCase()) &&
      ["red_count", "amber_count", "green_count"].every((key) =>
        containsNumber(context.output, aggregates[key] ?? "")
      ) &&
      expected.filter((record) => field(record, "rag") === "red")
        .every((record) => context.output.includes(field(record, "project_id").toLowerCase())) &&
      !context.output.includes("{{");
  }
  if (itemId === "draft") {
    const aggregates = scenario.expected.aggregates;
    return draftCount(context) === 1 &&
      context.drafts.includes(scenario.runTag.toLowerCase()) &&
      ["red_count", "amber_count", "green_count"].every((key) =>
        containsNumber(context.drafts, aggregates[key] ?? "")
      );
  }
  return false;
}

/* -------------------------------------------------------------- helpers */

function liveIdsFor(scenario: Scenario, service: string): readonly string[] {
  return scenario.resources
    .filter((resource) => resource.service === service)
    .map((resource) => resource.id);
}

function labelNamesById(snapshot: WorkspaceSnapshot, scenario: Scenario): Map<string, string> {
  const listing = snapshot.resources[`__labels_${scenario.id}`];
  const names = new Map<string, string>();
  if (!listing || typeof listing !== "object" || Array.isArray(listing) || !Array.isArray(listing.labels)) {
    return names;
  }
  for (const label of listing.labels) {
    if (label && typeof label === "object" && !Array.isArray(label) &&
        typeof label.id === "string" && typeof label.name === "string") {
      names.set(label.id, label.name);
    }
  }
  return names;
}

function messageLabelIds(snapshot: WorkspaceSnapshot, messageId: string): readonly string[] {
  const message = snapshot.resources[messageId];
  if (!message || typeof message !== "object" || Array.isArray(message) || !Array.isArray(message.labelIds)) {
    return [];
  }
  return message.labelIds.filter((value): value is string => typeof value === "string");
}

function messageLabelNames(
  snapshot: WorkspaceSnapshot,
  messageId: string,
  names: Map<string, string>,
): readonly string[] {
  return messageLabelIds(snapshot, messageId).map((id) => names.get(id) ?? id);
}

function draftCount(context: GradeContext): number {
  const listing = context.after.resources[`__drafts_${context.scenario.id}`];
  if (listing && typeof listing === "object" && !Array.isArray(listing) && Array.isArray(listing.drafts)) {
    return listing.drafts.length;
  }
  return context.after.drafts.length;
}

/** A count must appear near the thing it counts, not merely somewhere. */
function draftMentionsCount(text: string, label: string, count: string): boolean {
  if (!count) return false;
  return categoryWindowMatches(text, label.toLowerCase(), figurePattern(count));
}

function containsNumber(text: string, value: string): boolean {
  if (!value) return false;
  return figurePattern(value).test(normalizeFigures(text));
}

function figurePattern(value: string): RegExp {
  const normalized = normalizeFigures(value);
  const escaped = normalized.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  return new RegExp(`(?<![\\d.])${escaped}(?![\\d])`, "u");
}

function normalizeFigures(text: string): string {
  return text.normalize("NFKC")
    .replace(/[−‒–—]/gu, "-")
    .replace(/[$€£¥]/gu, "")
    .replace(/(?<=\d),(?=\d{3}(?:\D|$))/gu, "")
    .replace(/(\.\d*?)0+(?![\d])/gu, "$1")
    .replace(/\.(?![\d])/gu, "")
    .toLowerCase();
}

function numbersEqual(actual: string | undefined, expected: string): boolean {
  if (actual === undefined) return false;
  const left = Number(String(actual).replace(/[^0-9.eE+-]/gu, ""));
  const right = Number(expected);
  if (!Number.isFinite(left) || !Number.isFinite(right)) {
    return String(actual).trim().toUpperCase() === expected.trim().toUpperCase();
  }
  return Math.abs(left - right) < 0.005;
}

function booleansEqual(actual: string | undefined, expected: string): boolean {
  if (actual === undefined) return false;
  const normalize = (value: string) => {
    const lowered = value.trim().toLowerCase();
    if (["true", "yes", "y", "1"].includes(lowered)) return "true";
    if (["false", "no", "n", "0", ""].includes(lowered)) return "false";
    return lowered;
  };
  return normalize(actual) === normalize(expected);
}

function instantsEqual(actual: string | undefined, expected: string): boolean {
  if (!actual) return false;
  const left = Date.parse(actual);
  const right = Date.parse(expected);
  return Number.isFinite(left) && Number.isFinite(right) && Math.abs(left - right) < 60_000;
}

function categoryWindowMatches(text: string, anchor: string, ...expected: readonly RegExp[]): boolean {
  let start = text.indexOf(anchor);
  while (start >= 0) {
    const window = text.slice(start, start + 600);
    if (expected.every((pattern) => pattern.test(window))) return true;
    start = text.indexOf(anchor, start + anchor.length);
  }
  return false;
}

function snapshotState(snapshot: WorkspaceSnapshot) {
  return {
    resources: snapshot.resources,
    drafts: snapshot.drafts,
    calendarEvents: snapshot.calendarEvents,
  };
}

function findTable(snapshot: WorkspaceSnapshot, headers: readonly string[]): readonly string[][] | null {
  for (const value of Object.values(snapshot.resources)) {
    for (const table of sheetTables(value)) {
      const actual = table[0]?.map((cell) => cell.trim().toLowerCase()) ?? [];
      if (headers.every((header) => actual.includes(header))) return table;
    }
  }
  return null;
}

function findExactTable(snapshot: WorkspaceSnapshot, headers: readonly string[]): readonly string[][] | null {
  for (const value of Object.values(snapshot.resources)) {
    for (const table of sheetTables(value)) {
      const actual = table[0]?.map((cell) => cell.trim().toLowerCase()) ?? [];
      if (actual.length === headers.length && actual.every((header, index) => header === headers[index])) return table;
    }
  }
  return null;
}

function tableObjects(table: readonly string[][] | null): readonly Record<string, string>[] {
  if (!table?.[0]) return [];
  const headers = table[0].map((cell) => cell.trim().toLowerCase());
  return table.slice(1)
    .filter((row) => row.some((cell) => cell.trim()))
    .map((row) => Object.fromEntries(headers.map((header, index) => [header, row[index]?.trim() ?? ""])));
}

function tableCell(table: readonly string[][] | null, keyColumn: string, key: string, valueColumn: string): string | undefined {
  return tableObjects(table).find((row) => row[keyColumn] === key)?.[valueColumn];
}

function taggedOutputText(scenario: Scenario, snapshot: WorkspaceSnapshot): string {
  return stableText(Object.fromEntries(taggedOutputFiles(scenario, snapshot)));
}

interface TaggedPresentation {
  slides: number;
  text: string;
  slideTexts: readonly string[];
}

function taggedPresentation(scenario: Scenario, snapshot: WorkspaceSnapshot): TaggedPresentation | null {
  for (const [, value] of taggedOutputFiles(scenario, snapshot)) {
    if (!value || typeof value !== "object" || Array.isArray(value) || !Array.isArray(value.slides)) continue;
    return {
      slides: value.slides.length,
      text: stableText(value),
      slideTexts: value.slides.map((slide) => stableText(slide)),
    };
  }
  return null;
}

/**
 * Read one slide, not a character window. The detail slide legitimately lists
 * every line, so a window that ran past the end of the unfavourable slide would
 * report favourable categories as flagged.
 */
function slideText(presentation: TaggedPresentation | null, anchor: RegExp): string | null {
  return presentation?.slideTexts.find((text) => anchor.test(text)) ?? null;
}

function taggedOutputFiles(scenario: Scenario, snapshot: WorkspaceSnapshot): readonly [string, Json][] {
  const registered = new Set(scenario.resources.map((resource) => resource.id));
  return Object.entries(snapshot.resources).filter(([key]) => {
    if (!key.startsWith("__drive_file_")) return false;
    return !registered.has(key.slice("__drive_file_".length));
  });
}

function calendarItems(snapshot: WorkspaceSnapshot): readonly Record<string, Json>[] {
  return snapshot.calendarEvents.flatMap((calendar) => {
    if (!calendar || typeof calendar !== "object" || Array.isArray(calendar) || !Array.isArray(calendar.items)) {
      return [];
    }
    return calendar.items.flatMap((event) =>
      event && typeof event === "object" && !Array.isArray(event) ? [event] : []
    );
  });
}

function objectValue(value: Json | undefined): Record<string, Json> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value : null;
}

function stringValue(value: Json | undefined): string | null {
  return typeof value === "string" ? value : null;
}

function hasValues(value: Json | undefined): boolean {
  if (!value) return false;
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.values(value).some(hasValues);
  return true;
}

export function sheetTables(value: Json): readonly string[][][] {
  if (!value || typeof value !== "object" || Array.isArray(value) || !Array.isArray(value.sheets)) return [];
  const tables: string[][][] = [];
  for (const sheet of value.sheets) {
    if (!sheet || typeof sheet !== "object" || Array.isArray(sheet) || !Array.isArray(sheet.data)) continue;
    for (const data of sheet.data) {
      if (!data || typeof data !== "object" || Array.isArray(data) || !Array.isArray(data.rowData)) continue;
      const table: string[][] = [];
      for (const row of data.rowData) {
        if (!row || typeof row !== "object" || Array.isArray(row)) {
          table.push([]);
          continue;
        }
        const values = Array.isArray(row.values) ? row.values : [];
        table.push(values.map((cell) => {
          if (!cell || typeof cell !== "object" || Array.isArray(cell)) return "";
          if (typeof cell.formattedValue === "string") return cell.formattedValue;
          const effective = cell.effectiveValue;
          if (effective && typeof effective === "object" && !Array.isArray(effective)) {
            for (const candidate of Object.values(effective)) {
              if (typeof candidate === "string") return candidate;
              if (typeof candidate === "number") return String(candidate);
              if (typeof candidate === "boolean") return String(candidate);
            }
          }
          return "";
        }));
      }
      tables.push(table);
    }
  }
  return tables;
}

export function stableText(value: unknown): string {
  return JSON.stringify(value, (_key, nested) => {
    if (nested && typeof nested === "object" && !Array.isArray(nested)) {
      return Object.fromEntries(Object.entries(nested).sort(([left], [right]) => left.localeCompare(right)));
    }
    return nested;
  })?.toLowerCase() ?? "";
}

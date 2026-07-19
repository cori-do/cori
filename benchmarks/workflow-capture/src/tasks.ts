import type { AllowedSideEffects, RubricItem, TaskSpec, WorkspaceService } from "./types.js";

const safety: AllowedSideEffects = {
  draftsOnly: true,
  calendarSendUpdates: "none",
  resourceTypes: ["gmail", "sheets", "docs", "drive", "calendar", "slides"],
  requiredTag: true,
};

const rubric = (...items: readonly [string, string, number][]): readonly RubricItem[] =>
  items.map(([id, description, points]) => ({ id, description, points }));

const services = (...requiredServices: readonly WorkspaceService[]) => requiredServices;

export const TASKS: readonly TaskSpec[] = [
  {
    id: "support_inbox_triage",
    name: "Support Inbox Triage",
    domain: "support",
    runtimeTrack: "hybrid",
    parameters: ["queue_spreadsheet_id", "gmail_query", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("gmail", "sheets"),
    resources: [
      { parameter: "queue_spreadsheet_id", role: "support queue", service: "sheets", source: true },
      { role: "support outage message", service: "gmail", source: true },
      { role: "support access message", service: "gmail", source: true },
      { role: "support how-to message", service: "gmail", source: true },
    ],
    prompt: "The queue spreadsheet is fresh and contains only a Source tag marker. Search gmail_query and triage every returned unread message. Categories: outage for a broad service outage, unavailability, data loss, or security incident; access for blocked login, access, or permissions; billing for an invoice, payment, or charge issue; bug for a reproducible product defect not covered by the earlier categories; how_to otherwise. Priorities: P0 for a broad outage, data loss, or security incident; P1 for blocked access or an incorrect or duplicate charge; P2 otherwise. Create a Triage Queue tab, sort by priority P0 then P1 then P2, then received_at ascending, then message_id, and batch-write exactly these columns in this order: message_id, received_at, sender, subject, category, priority, status, summary, run_tag, as_of. Use status=triaged, the exact run_tag and as_of parameters, and factual source values. For every message, add exactly the labels <run_tag>/category/<category> and <run_tag>/priority/<priority>, and remove UNREAD. Create exactly one draft to support-lead@example.test containing the run tag and category and priority counts. Never send mail.",
    rubric: rubric(["classification", "Exact category and priority classification", 40], ["queue", "Sorted queue rows and facts", 25], ["gmail", "Labels and read state", 20], ["draft", "One factual internal draft and no sent mail", 15]),
    allowedSideEffects: safety,
  },
  {
    id: "sla_breach_pack",
    name: "SLA Breach Pack",
    domain: "support",
    runtimeTrack: "deterministic",
    parameters: ["case_spreadsheet_id", "report_template_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [{ parameter: "case_spreadsheet_id", role: "case register", service: "sheets", source: true }, { parameter: "report_template_id", role: "report template", service: "docs", source: true }],
    prompt: "The Source sheet has case_id, status, priority, opened_at, subject, and benchmark_tag columns. Apply P0=1h, P1=4h, P2=24h, P3=72h to every open or in-progress case; exclude closed cases. Using as_of, calculate sla_deadline. Mark breached=true only when sla_deadline is strictly before as_of; mark due_within_two_hours=true only when the deadline is at or after as_of and no more than two hours later. Batch-write an SLA Results tab with the exact columns case_id, status, priority, opened_at, sla_deadline, breached, due_within_two_hours, run_tag. Fill a copied report template with the computed totals, and create a support-lead draft.",
    rubric: rubric(["sla", "Boundary calculations", 45], ["sheet", "Result tab and totals", 20], ["doc", "Report facts and links", 20], ["draft", "Draft facts", 15]),
    allowedSideEffects: safety,
  },
  {
    id: "lead_follow_up_queue",
    name: "Lead Follow-up Queue",
    domain: "sales",
    runtimeTrack: "deterministic",
    parameters: ["lead_spreadsheet_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "gmail"),
    resources: [{ parameter: "lead_spreadsheet_id", role: "lead register", service: "sheets", source: true }],
    prompt: "The fresh spreadsheet contains only a Source sheet with lead_id, status, stage, next_action_due, value, last_contact_at, contact_name, contact_email, next_action, and benchmark_tag; Follow-up Queue does not exist yet. Rank active leads using stage points qualified=30, proposal=50, negotiation=70; add 20 when next_action_due is at or before as_of and 10 when value is at least 10000. Exclude closed/lost. Sort by score descending, then oldest last_contact_at, then lead_id. Create Follow-up Queue and batch-write lead_id, lead_score, next_action, run_tag; in the Source sheet, update the top-ranked lead's next_action to the exact value 'Send personalized follow-up'; and create exactly one draft to that lead's contact_email containing the run tag. Every author, qualification, direct, and replay fixture is fresh, so do not add already-exists or stale-row clearing logic.",
    rubric: rubric(["ranking", "Scores and deterministic tie-breaks", 50], ["sheet", "Queue and top-lead update", 30], ["draft", "Customer draft facts", 20]),
    allowedSideEffects: safety,
  },
  {
    id: "customer_meeting_prep",
    name: "Customer Meeting Prep",
    domain: "sales",
    runtimeTrack: "hybrid",
    parameters: ["calendar_id", "account_brief_id", "source_message_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("calendar", "docs", "gmail", "drive"),
    resources: [{ parameter: "calendar_id", role: "customer calendar", service: "calendar", source: true }, { parameter: "account_brief_id", role: "account brief", service: "docs", source: true }, { parameter: "source_message_id", role: "customer message", service: "gmail", source: true }],
    prompt: "Find the next calendar event containing the run tag within seven days after as_of. Combine only its details, the account brief, and the source message into a tagged prep Doc with sections Objectives, Account Facts, Risks, and exactly five Questions. Put the prep Doc URL in that event's description using sendUpdates=none, and create exactly one tagged internal briefing draft. Do not invent facts.",
    rubric: rubric(["facts", "Source-fact coverage and no inventions", 45], ["doc", "Required prep sections", 25], ["calendar", "Correct event link without notifications", 15], ["draft", "Briefing draft", 15]),
    allowedSideEffects: safety,
  },
  {
    id: "new_hire_onboarding_pack",
    name: "New-hire Onboarding Pack",
    domain: "hr",
    runtimeTrack: "deterministic",
    parameters: ["new_hire_spreadsheet_id", "template_document_id", "calendar_id", "run_tag"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "docs", "drive", "calendar", "gmail"),
    resources: [{ parameter: "new_hire_spreadsheet_id", role: "new hire register", service: "sheets", source: true }, { parameter: "template_document_id", role: "onboarding template", service: "docs", source: true }, { parameter: "calendar_id", role: "orientation calendar", service: "calendar", source: true }],
    prompt: "The New Hires sheet contains exactly one pending hire. Copy the supplied template and replace every {{NAME}}, {{EMAIL}}, {{MANAGER}}, {{START_DATE}}, and {{RUN_TAG}} placeholder. Create a 60-minute orientation in calendar_id at 09:00 in the row's timezone on orientation_date using sendUpdates=none, create exactly one welcome draft to the hire, and update that row to prepared with the pack and event links.",
    rubric: rubric(["template", "Every placeholder filled", 30], ["calendar", "Exact event fields and no notifications", 25], ["sheet", "Prepared state and links", 25], ["draft", "Welcome draft and uniqueness", 20]),
    allowedSideEffects: safety,
  },
  {
    id: "preapproved_pto_processing",
    name: "Pre-approved PTO Processing",
    domain: "hr",
    runtimeTrack: "deterministic",
    parameters: ["pto_spreadsheet_id", "calendar_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "calendar", "gmail"),
    resources: [{ parameter: "pto_spreadsheet_id", role: "PTO register", service: "sheets", source: true }, { parameter: "calendar_id", role: "PTO calendar", service: "calendar", source: true }],
    prompt: "The PTO sheet contains one manager-approved request and holiday rows. Count weekdays from start_date through end_date inclusive, excluding dates listed as holidays. Decrement pto_balance_days by that count, set status=scheduled and business_days to the count, create one ordinary all-day Calendar event representing Out of Office in calendar_id whose end date is exclusive using sendUpdates=none, and create exactly one tagged confirmation draft to employee_email. The Calendar event must use the default event type: Google Calendar's special outOfOffice event type cannot be all-day.",
    rubric: rubric(["days", "Weekday and holiday count", 35], ["balance", "Correct balance and row state", 30], ["calendar", "All-day exclusive boundaries", 20], ["draft", "Confirmation draft", 15]),
    allowedSideEffects: safety,
  },
  {
    id: "weekly_operating_review",
    name: "Weekly Operating Review",
    domain: "management",
    runtimeTrack: "deterministic",
    parameters: ["project_spreadsheet_id", "report_template_id", "run_tag", "week_ending"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [{ parameter: "project_spreadsheet_id", role: "project register", service: "sheets", source: true }, { parameter: "report_template_id", role: "review template", service: "docs", source: true }],
    prompt: "For each Projects row assign red when blocked=true, days_overdue>14, or progress_percent<50; otherwise amber when days_overdue>=7 or progress_percent<80; otherwise green. Batch-write Weekly Review rows with project_id, rag, escalations, run_tag plus KPI totals, copy and fill the supplied report template with the totals and escalation facts, and create exactly one tagged leadership draft.",
    rubric: rubric(["rag", "Every RAG assignment", 45], ["aggregates", "KPI totals and escalation rows", 25], ["doc", "Weekly review facts", 20], ["draft", "Leadership draft", 10]),
    allowedSideEffects: safety,
  },
  {
    id: "meeting_action_register",
    name: "Meeting Action Register",
    domain: "management",
    runtimeTrack: "hybrid",
    parameters: ["meeting_notes_document_id", "action_tracker_spreadsheet_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("docs", "sheets", "gmail"),
    resources: [{ parameter: "meeting_notes_document_id", role: "meeting notes", service: "docs", source: true }, { parameter: "action_tracker_spreadsheet_id", role: "action tracker", service: "sheets", source: true }],
    prompt: "Extract only explicit action, owner, due date, and source section from the meeting notes. Use TBD when no date is stated. Deduplicate case-insensitively by normalized action text plus owner, preserving the first occurrence. Batch-write Actions columns action, owner, due_date, source_section, run_tag and create exactly one tagged follow-up draft without inventing actions.",
    rubric: rubric(["extraction", "Exact action facts", 45], ["dedupe", "Deduplication and TBD handling", 25], ["sheet", "Batch tracker write", 20], ["draft", "Follow-up draft", 10]),
    allowedSideEffects: safety,
  },
  {
    id: "expense_policy_audit",
    name: "Expense Policy Audit",
    domain: "finance",
    runtimeTrack: "deterministic",
    parameters: ["expense_spreadsheet_id", "report_template_id", "run_tag", "as_of"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [{ parameter: "expense_spreadsheet_id", role: "expense register", service: "sheets", source: true }, { parameter: "report_template_id", role: "audit template", service: "docs", source: true }],
    prompt: "Audit every Expenses row. FAIL for each applicable reason: missing receipt when amount>=75; hotel rate above 250 per night; meal cost above 60 per attendee; personal=true; or an invoice_id duplicated across rows. Use the exact reason codes missing_receipt, hotel_rate, meal_per_person, personal, and duplicate_invoice, joined with semicolons when multiple apply. Batch-write Audit columns expense_id, audit, reasons, run_tag including all applicable reasons, copy and fill the exceptions report with exact totals, and create exactly one tagged finance draft.",
    rubric: rubric(["findings", "Row-level findings and reasons", 50], ["duplicates", "Duplicate invoice handling", 20], ["report", "Exception totals and document", 20], ["draft", "Finance draft", 10]),
    allowedSideEffects: safety,
  },
  {
    id: "budget_variance_deck",
    name: "Budget Variance Deck",
    domain: "finance",
    runtimeTrack: "deterministic",
    parameters: ["budget_spreadsheet_id", "run_tag", "period"].map((name) => ({ name, description: name.replaceAll("_", " ") })),
    requiredServices: services("sheets", "slides", "gmail"),
    resources: [{ parameter: "budget_spreadsheet_id", role: "budget register", service: "sheets", source: true }],
    prompt: "For period, calculate variance_amount=actual-budget and variance_percent=(actual-budget)/budget*100, using N/A when budget is zero. Flag expense rows when variance_percent>10 and revenue rows when variance_percent<-10. Create one tagged presentation with exactly three slides titled Executive Summary, Unfavorable Variances, and Detail, containing the computed figures, then create exactly one tagged finance draft.",
    rubric: rubric(["variance", "Amount, percent, sign, zero-budget handling", 45], ["flags", "Unfavorable and favorable rankings", 25], ["deck", "Three-slide structure and text", 20], ["draft", "Finance draft", 10]),
    allowedSideEffects: safety,
  },
];

export function taskById(id: string): TaskSpec {
  const task = TASKS.find((candidate) => candidate.id === id);
  if (!task) throw new Error(`unknown benchmark task: ${id}`);
  return task;
}

export function assertTaskCatalog(): void {
  if (TASKS.length !== 10) throw new Error(`expected 10 tasks, found ${TASKS.length}`);
  for (const task of TASKS) {
    const total = task.rubric.reduce((sum, item) => sum + item.points, 0);
    if (total !== 100) throw new Error(`${task.id} rubric totals ${total}, expected 100`);
    if (task.allowedSideEffects.calendarSendUpdates !== "none" || !task.allowedSideEffects.draftsOnly) {
      throw new Error(`${task.id} violates benchmark safety defaults`);
    }
    const parameterNames = new Set(task.parameters.map((parameter) => parameter.name));
    for (const resource of task.resources) {
      if (resource.parameter && !parameterNames.has(resource.parameter)) {
        throw new Error(`${task.id} resource ${resource.role} references missing parameter ${resource.parameter}`);
      }
    }
  }
}

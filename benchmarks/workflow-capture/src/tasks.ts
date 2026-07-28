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

const parameters = (...names: readonly string[]) =>
  names.map((name) => ({ name, description: name.replaceAll("_", " ") }));

/**
 * Every prompt is written the way an operations runbook is written: it states
 * the business policy and the exact output contract, and stops there. It never
 * states which input maps to which answer, and it never enumerates the trigger
 * words a classifier could match. Tasks on the `hybrid` track additionally
 * receive source data whose surface form is regenerated every run, so the
 * mapping from input to answer cannot be captured as literals at design time.
 */
export const TASKS: readonly TaskSpec[] = [
  {
    id: "support_inbox_triage",
    name: "Support Inbox Triage",
    domain: "support",
    runtimeTrack: "hybrid",
    requiresRuntimeModel: true,
    rerunContract: true,
    parameters: parameters("queue_spreadsheet_id", "gmail_query", "run_tag", "as_of"),
    requiredServices: services("gmail", "sheets"),
    resources: [
      { parameter: "queue_spreadsheet_id", role: "support queue", service: "sheets", source: true },
      { role: "support inbox", service: "gmail", source: true },
    ],
    prompt: [
      "Run the morning support triage that the support team performs before standup.",
      "",
      "Read every message returned by gmail_query. The daily volume varies and is not fixed; process whatever the search returns and never assume a count. Customers write in several languages and in their own words.",
      "",
      "Decide two things for each message, from what the customer is actually describing rather than from any particular wording:",
      "- category: `outage` when a service is unavailable, failing, or degraded; `access` when someone cannot get into an account or resource they are entitled to; `billing` when the subject is an invoice, payment, refund, or charge; `bug` when the product behaves incorrectly but remains usable; `how_to` when the customer is asking how to accomplish something.",
      "- priority: `P0` when the customer describes impact reaching many users, or any loss of data or exposure of information; `P1` when one person or one team is completely unable to work, or money has moved incorrectly; `P2` in every other case.",
      "",
      "Category and priority are independent judgements. Do not infer one from the other.",
      "",
      "This job runs every morning against the same mailbox. Messages already carrying a label whose name ends in `/triaged` were completed by an earlier run: leave them entirely untouched, and exclude them from the queue, the labels, and the counts. Re-running this workflow on an unchanged mailbox must make no further changes.",
      "",
      "Create a `Triage Queue` tab and batch-write exactly these columns in this order: message_id, received_at, sender, subject, category, priority, status, run_tag, as_of. Sort by priority P0 then P1 then P2, then received_at ascending, then message_id ascending. Use status=triaged, the exact run_tag and as_of parameter values, and factual values copied from each message for the source columns.",
      "",
      "For every message you triage, add the labels `<run_tag>/category/<category>`, `<run_tag>/priority/<priority>`, and `<run_tag>/triaged`, and remove UNREAD. Create exactly one draft to support-lead@example.test containing the run tag and the per-category and per-priority counts of the messages you triaged. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["classification", "Category and priority for every message", 40],
      ["idempotence", "Previously triaged messages left untouched and excluded", 20],
      ["queue", "Queue ordering, columns, and source facts", 20],
      ["gmail", "Labels applied and read state cleared", 10],
      ["draft", "One factual internal digest and no sent mail", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "inbound_lead_qualification",
    name: "Inbound Lead Qualification",
    domain: "sales",
    runtimeTrack: "hybrid",
    requiresRuntimeModel: true,
    parameters: parameters("lead_spreadsheet_id", "gmail_query", "run_tag", "as_of"),
    requiredServices: services("gmail", "sheets"),
    resources: [
      { parameter: "lead_spreadsheet_id", role: "lead register", service: "sheets", source: true },
      { role: "inbound lead inbox", service: "gmail", source: true },
    ],
    prompt: [
      "Qualify the inbound sales enquiries that arrived overnight, the way the SDR team does each morning.",
      "",
      "Read every message returned by gmail_query. Prospects describe their situation in prose; some write in French or German. From each message determine:",
      "- seat_count: the number of people who would use the product, as an integer. Prospects state this in many ways, including in words, as a range, or as a sum of teams. When a range is given, use its upper bound. When the message genuinely does not indicate a number, use 0.",
      "- timeline_days: whole days from as_of until the date the prospect wants to be live. Resolve relative and fiscal expressions against as_of. When no timeline is indicated, use 0.",
      "- security_review: `true` when the prospect indicates that a security, legal, procurement, or compliance review will be part of their buying process, otherwise `false`.",
      "",
      "Then apply the standing qualification policy. Start at 0 points and add: 40 when seat_count is 100 or more, 25 when seat_count is between 25 and 99 inclusive, 10 when seat_count is between 1 and 24 inclusive; 30 when timeline_days is between 1 and 45 inclusive, 15 when timeline_days is between 46 and 120 inclusive; 20 when security_review is false. Band the total as `hot` at 70 or above, `warm` at 40 to 69, and `nurture` below 40.",
      "",
      "Create a `Qualified Leads` tab and batch-write exactly these columns in this order: message_id, sender, company, seat_count, timeline_days, security_review, score, band, run_tag, as_of. Sort by score descending, then seat_count descending, then message_id ascending. Use company exactly as the prospect names their organisation.",
      "",
      "Create exactly one draft to the sender of the highest-scoring lead, containing the run tag and that prospect's stated seat count and timeline. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["extraction", "Seat count, timeline, and security signal per lead", 45],
      ["scoring", "Policy score and band from the extracted values", 20],
      ["ordering", "Deterministic queue ordering", 15],
      ["sheet", "Register columns and source facts", 10],
      ["draft", "One factual draft to the top lead", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "vendor_invoice_intake",
    name: "Vendor Invoice Intake",
    domain: "finance",
    runtimeTrack: "hybrid",
    requiresRuntimeModel: true,
    parameters: parameters("register_spreadsheet_id", "invoice_folder_query", "run_tag", "as_of"),
    requiredServices: services("drive", "docs", "sheets", "gmail"),
    resources: [
      { parameter: "register_spreadsheet_id", role: "invoice register", service: "sheets", source: true },
      { role: "vendor invoices", service: "docs", source: true },
    ],
    prompt: [
      "Process the vendor invoices that accounts payable received this week.",
      "",
      "Every registered invoice document belongs to a different vendor and each vendor formats its invoices differently: field order, labels, currency symbols, and date formats all vary between documents, and some state tax before the total while others state it after. From each document read the vendor name, the vendor's own invoice number, the ISO 4217 currency code, the net amount, the tax amount, the gross amount, and the payment due date.",
      "",
      "Record amounts as plain decimal numbers with two decimal places and no currency symbol or thousands separator. Record the due date as YYYY-MM-DD.",
      "",
      "Then apply the accounts payable control policy:",
      "- status is `blocked` when the stated net and tax amounts do not sum to the stated gross amount. Record the document exactly as written and never adjust a figure to make it balance; a human resolves these.",
      "- otherwise status is `overdue` when the due date is strictly before as_of, and `payable` in every other case.",
      "",
      "Create an `Invoice Register` tab and batch-write exactly these columns in this order: document_id, vendor, invoice_number, currency, net, tax, gross, due_date, status, run_tag, as_of. Sort by status `blocked` then `overdue` then `payable`, then due_date ascending, then invoice_number ascending.",
      "",
      "Create exactly one draft to ap-lead@example.test containing the run tag, the count of invoices in each status, and the vendor name and invoice number of every blocked invoice. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["extraction", "Vendor, number, currency, amounts, and due date per invoice", 40],
      ["reconciliation", "Blocked invoices detected and never silently corrected", 25],
      ["status", "Overdue and payable classification against as_of", 15],
      ["sheet", "Register columns and ordering", 10],
      ["draft", "Exception draft naming every blocked invoice", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "incident_postmortem_pack",
    name: "Incident Postmortem Pack",
    domain: "engineering",
    runtimeTrack: "hybrid",
    requiresRuntimeModel: true,
    parameters: parameters("transcript_document_id", "metrics_spreadsheet_id", "findings_spreadsheet_id", "run_tag", "as_of"),
    requiredServices: services("docs", "sheets", "drive", "gmail"),
    resources: [
      { parameter: "transcript_document_id", role: "incident channel transcript", service: "docs", source: true },
      { parameter: "metrics_spreadsheet_id", role: "incident metrics", service: "sheets", source: true },
      { parameter: "findings_spreadsheet_id", role: "postmortem findings", service: "sheets", source: true },
    ],
    prompt: [
      "Prepare the postmortem pack for the incident whose response channel transcript is registered below.",
      "",
      "The transcript is the raw channel history: messages are interleaved, arrive out of order, and include side conversations. During the response the team raised several possible explanations and ruled some of them out as the investigation progressed. Read the whole transcript and identify only the causes the team actually confirmed as contributing to the incident. A hypothesis that the transcript later rules out is not a contributing factor and must not appear in your output.",
      "",
      "The metrics sheet records the incident's `detected_at`, `mitigated_at`, and `resolved_at` timestamps. From them compute time_to_detect_minutes, time_to_mitigate_minutes, and time_to_resolve_minutes as whole minutes, each measured from the incident's `started_at`.",
      "",
      "Create a `Contributing Factors` tab in the findings spreadsheet and batch-write exactly these columns in this order: factor_id, summary, confirmed_by, run_tag. Write one row per confirmed contributing factor, ordered by factor_id ascending, where confirmed_by is the person the transcript shows confirming it and summary is a factual one-line description in your own words.",
      "",
      "Create a `Timings` tab and batch-write exactly these columns in this order: metric, minutes, run_tag, with one row per computed duration using the metric names time_to_detect, time_to_mitigate, and time_to_resolve.",
      "",
      "Create exactly one draft to incident-review@example.test containing the run tag, the three computed durations, and the count of confirmed contributing factors. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["factors", "Every confirmed contributing factor recorded", 30],
      ["exclusion", "Ruled-out hypotheses absent from the findings", 25],
      ["attribution", "Correct confirming person per factor", 15],
      ["timings", "Duration arithmetic from the metrics sheet", 20],
      ["draft", "Review draft facts", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "contract_obligation_register",
    name: "Contract Obligation Register",
    domain: "legal",
    runtimeTrack: "hybrid",
    requiresRuntimeModel: true,
    parameters: parameters("contract_document_id", "register_spreadsheet_id", "run_tag", "as_of"),
    requiredServices: services("docs", "sheets", "gmail"),
    resources: [
      { parameter: "contract_document_id", role: "customer contract", service: "docs", source: true },
      { parameter: "register_spreadsheet_id", role: "obligation register", service: "sheets", source: true },
    ],
    prompt: [
      "Build the obligation register for the registered customer contract, as legal operations does when a signed contract arrives.",
      "",
      "Read the contract and record every obligation that binds either party to act by a date. Obligations are written in ordinary contract style: some clauses state their own deadline, and some state it by reference to a period or a date defined in another clause. Resolve those references and record the resulting number of days.",
      "",
      "For each obligation record the clause reference exactly as the contract labels it, which party the obligation binds, a factual one-line description of what must be done, and notice_days as the whole number of days of notice or lead time the contract requires.",
      "",
      "Then compute, for each obligation, the earliest date on which the obliged party must act in order to satisfy the contract before the term ends, as term_end minus notice_days, in YYYY-MM-DD. Mark action_required as `true` when that date is on or before as_of, otherwise `false`.",
      "",
      "Create an `Obligations` tab and batch-write exactly these columns in this order: clause, party, obligation, notice_days, act_by, action_required, run_tag, as_of. Sort by act_by ascending, then clause ascending.",
      "",
      "Create exactly one draft to legal-ops@example.test containing the run tag, the total number of obligations, and the clause references of every obligation whose action_required is true. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["obligations", "Every obligation and its binding party", 35],
      ["references", "Cross-referenced notice periods resolved", 20],
      ["dates", "act_by arithmetic and action_required flags", 25],
      ["sheet", "Register columns and ordering", 10],
      ["draft", "Legal operations draft facts", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "sla_breach_pack",
    name: "SLA Breach Pack",
    domain: "support",
    runtimeTrack: "deterministic",
    parameters: parameters("case_spreadsheet_id", "report_template_id", "run_tag", "as_of"),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [
      { parameter: "case_spreadsheet_id", role: "case register", service: "sheets", source: true },
      { parameter: "report_template_id", role: "report template", service: "docs", source: true },
    ],
    prompt: [
      "Produce the SLA breach pack for the registered case load.",
      "",
      "The Source sheet has case_id, status, priority, opened_at, subject, and benchmark_tag columns. The case load varies between runs; process every row present.",
      "",
      "Apply the response targets P0=1h, P1=4h, P2=24h, P3=72h to every case whose status is open or in_progress, and exclude closed cases entirely. Compute sla_deadline as opened_at plus the target for that case's priority. Mark breached=true only when sla_deadline is strictly before as_of. Mark due_within_two_hours=true only when sla_deadline is at or after as_of and no more than two hours after it.",
      "",
      "Batch-write an `SLA Results` tab with exactly these columns in this order: case_id, status, priority, opened_at, sla_deadline, breached, due_within_two_hours, run_tag. Sort by sla_deadline ascending, then case_id ascending.",
      "",
      "Copy the supplied report template, fill it with the computed breach and warning totals, and create exactly one draft to support-lead@example.test containing the run tag and those totals. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["sla", "Deadline and boundary calculations for every case", 45],
      ["sheet", "Result tab, ordering, and totals", 20],
      ["doc", "Report facts and links", 20],
      ["draft", "Draft facts", 15],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "expense_policy_audit",
    name: "Expense Policy Audit",
    domain: "finance",
    runtimeTrack: "deterministic",
    parameters: parameters("expense_spreadsheet_id", "report_template_id", "run_tag", "as_of"),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [
      { parameter: "expense_spreadsheet_id", role: "expense register", service: "sheets", source: true },
      { parameter: "report_template_id", role: "audit template", service: "docs", source: true },
    ],
    prompt: [
      "Audit the submitted expense claims against the travel and expense policy. The claim volume varies between runs; audit every row present.",
      "",
      "A row FAILS for each of these reasons that applies, recorded with the exact reason code shown: `missing_receipt` when no receipt is present and the amount is 75 or more; `hotel_rate` when a hotel claim's amount divided by its nights exceeds 250; `meal_per_person` when a meal claim's amount divided by its attendees exceeds 60; `personal` when the claim is marked personal; `duplicate_invoice` when the same invoice_id appears on more than one row. Join multiple reasons with semicolons in the order listed here. A row with no applicable reason PASSES.",
      "",
      "Batch-write an `Audit` tab with exactly these columns in this order: expense_id, audit, reasons, run_tag, ordered by expense_id ascending.",
      "",
      "Copy and fill the exceptions report template with the exact number of failing rows and the reason codes present, and create exactly one draft to finance-lead@example.test containing the run tag and the failure count. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["findings", "Row-level pass or fail and every applicable reason", 50],
      ["duplicates", "Duplicate invoice detection across rows", 20],
      ["report", "Exception totals and document", 20],
      ["draft", "Finance draft", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "budget_variance_deck",
    name: "Budget Variance Deck",
    domain: "finance",
    runtimeTrack: "deterministic",
    parameters: parameters("budget_spreadsheet_id", "run_tag", "period"),
    requiredServices: services("sheets", "slides", "gmail"),
    resources: [
      { parameter: "budget_spreadsheet_id", role: "budget register", service: "sheets", source: true },
    ],
    prompt: [
      "Build the quarterly budget variance deck for the given period. The number of budget lines varies between runs; include every line whose period matches.",
      "",
      "For each line compute variance_amount = actual - budget, and variance_percent = (actual - budget) / budget * 100 rounded to one decimal place, using the exact text `N/A` when budget is zero. Flag an expense line as unfavourable when its variance_percent is above 10, and a revenue line as unfavourable when its variance_percent is below -10.",
      "",
      "Create one presentation whose title contains the run tag, with exactly three slides titled `Executive Summary`, `Unfavourable Variances`, and `Detail`. The summary slide states the count of unfavourable lines; the unfavourable slide names each unfavourable line's category with its variance amount and percent; the detail slide lists every line with its budget, actual, variance amount, and variance percent.",
      "",
      "Create exactly one draft to finance-lead@example.test containing the run tag and the count of unfavourable lines. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["variance", "Amount, percent, sign, and zero-budget handling", 45],
      ["flags", "Unfavourable classification by line type", 25],
      ["deck", "Three-slide structure and required figures", 20],
      ["draft", "Finance draft", 10],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "preapproved_pto_processing",
    name: "Pre-approved PTO Processing",
    domain: "hr",
    runtimeTrack: "deterministic",
    parameters: parameters("pto_spreadsheet_id", "calendar_id", "run_tag", "as_of"),
    requiredServices: services("sheets", "calendar", "gmail"),
    resources: [
      { parameter: "pto_spreadsheet_id", role: "PTO register", service: "sheets", source: true },
      { parameter: "calendar_id", role: "PTO calendar", service: "calendar", source: true },
    ],
    prompt: [
      "Process the manager-approved time-off requests in the register. The sheet contains request rows and company holiday rows; the number of each varies between runs.",
      "",
      "For every request whose status is approved, count the weekdays from start_date through end_date inclusive, excluding any date listed as a company holiday. Decrement that employee's pto_balance_days by the count, set status to `scheduled`, and set business_days to the count.",
      "",
      "For each processed request create one ordinary all-day Calendar event in calendar_id representing the employee's time off, whose end date is exclusive, using sendUpdates=none. Use the default event type: Google Calendar's outOfOffice event type cannot be all-day.",
      "",
      "Create exactly one draft per processed request to that employee's employee_email, containing the run tag, the dates, and the business-day count. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["days", "Weekday count excluding holidays for every request", 35],
      ["balance", "Balance decrement and row state", 30],
      ["calendar", "All-day events with exclusive end boundaries", 20],
      ["draft", "Confirmation drafts", 15],
    ),
    allowedSideEffects: safety,
  },
  {
    id: "weekly_operating_review",
    name: "Weekly Operating Review",
    domain: "management",
    runtimeTrack: "deterministic",
    parameters: parameters("project_spreadsheet_id", "report_template_id", "run_tag", "week_ending"),
    requiredServices: services("sheets", "docs", "drive", "gmail"),
    resources: [
      { parameter: "project_spreadsheet_id", role: "project register", service: "sheets", source: true },
      { parameter: "report_template_id", role: "review template", service: "docs", source: true },
    ],
    prompt: [
      "Produce the weekly operating review from the project register. The portfolio size varies between runs; assess every project row.",
      "",
      "Assign each project a RAG status: `red` when the project is blocked, or days_overdue is above 14, or progress_percent is below 50; otherwise `amber` when days_overdue is 7 or more, or progress_percent is below 80; otherwise `green`. A project is an escalation when its RAG status is red.",
      "",
      "Batch-write a `Weekly Review` tab with exactly these columns in this order: project_id, rag, escalation, owner, run_tag, ordered by RAG red then amber then green, then project_id ascending.",
      "",
      "Copy and fill the supplied report template with the per-status counts and the project_id of every escalation, and create exactly one draft to leadership@example.test containing the run tag and the per-status counts. Never send mail.",
    ].join("\n"),
    rubric: rubric(
      ["rag", "RAG status for every project", 45],
      ["aggregates", "Status counts and escalation rows", 25],
      ["doc", "Weekly review document facts", 20],
      ["draft", "Leadership draft", 10],
    ),
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
  const hybrid = TASKS.filter((task) => task.runtimeTrack === "hybrid");
  if (hybrid.length < 5) {
    throw new Error(`expected at least 5 hybrid tasks, found ${hybrid.length}`);
  }
  for (const task of TASKS) {
    const total = task.rubric.reduce((sum, item) => sum + item.points, 0);
    if (total !== 100) throw new Error(`${task.id} rubric totals ${total}, expected 100`);
    if (task.allowedSideEffects.calendarSendUpdates !== "none" || !task.allowedSideEffects.draftsOnly) {
      throw new Error(`${task.id} violates benchmark safety defaults`);
    }
    if ((task.runtimeTrack === "hybrid") !== (task.requiresRuntimeModel === true)) {
      throw new Error(`${task.id} must set requiresRuntimeModel exactly when it is on the hybrid track`);
    }
    const parameterNames = new Set(task.parameters.map((parameter) => parameter.name));
    for (const resource of task.resources) {
      if (resource.parameter && !parameterNames.has(resource.parameter)) {
        throw new Error(`${task.id} resource ${resource.role} references missing parameter ${resource.parameter}`);
      }
    }
  }
}

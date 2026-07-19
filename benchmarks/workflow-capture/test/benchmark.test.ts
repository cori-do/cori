import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

import {
  codexAutomationArgs,
  codexModel,
  DEFAULT_CODEX_MODEL,
  parseJsonl,
} from "../src/harness.js";
import { gradeExternalState } from "../src/grader.js";
import {
  benchmarkCalendarEnv,
  configuredBenchmarkCalendarId,
  gmailFixtureReady,
  GwsClient,
  requireBenchmarkCalendarId,
  WorkspaceScenarioDriver,
} from "../src/gws.js";
import { inspectWorkflowPolicy } from "../src/policy.js";
import { normalizedCsv, readJson, scorecard, writeJson } from "../src/artifacts.js";
import {
  aggregateCaptures,
  approvalPrompt,
  authoringReference,
  captureReady,
  failedTraceDiagnostic,
  formatWorkflowCheckFailure,
  isCoriWorkflowCliHelp,
  parseBatch,
  prepareCaptureWorkspace,
  prepareDirectWorkspace,
  renderedTaskPrompt,
  report,
  retryableCaptureFailure,
  runBenchmark,
  selectTasks,
  traceUsage,
  trialIntegrityError,
  transcriptExecutedCoriRun,
  workspaceCoriBinary,
} from "../src/runner.js";
import {
  assertTwinEquivalent,
  buildScenario,
  validateScenarioFixtures,
} from "../src/scenario.js";
import { pairedDifferenceCi95, reuseAdvantage } from "../src/statistics.js";
import { assertTaskCatalog, TASKS } from "../src/tasks.js";
import { benchmarkViewerDocument } from "../src/viewer.js";
import type {
  BenchmarkResultV1,
  Grade,
  Json,
  Scenario,
  TrialResult,
  WorkspaceSnapshot,
} from "../src/types.js";

const packageRoot = join(fileURLToPath(new URL("../..", import.meta.url)));

test("task catalog contains ten 100-point tasks", () => {
  assertTaskCatalog();
  assert.equal(TASKS.length, 10);
  assert.equal(
    TASKS.filter((task) => task.runtimeTrack === "deterministic").length,
    7,
  );
  assert.equal(
    TASKS.filter((task) => task.runtimeTrack === "hybrid").length,
    3,
  );
});

test("authoring guidance locks the flat shallow-merged state contract", async () => {
  const guidance = authoringReference(
    "gpt-5.4",
    "hybrid",
    "support_inbox_triage",
  );
  assert.match(
    guidance,
    /execution state begins with manifest parameters as top-level keys/u,
  );
  assert.match(
    guidance,
    /Successful object outputs are shallow-merged into one flat state object/u,
  );
  assert.match(
    guidance,
    /duplicate top-level key overwrites the earlier value/u,
  );
  assert.match(
    guidance,
    /required step input key must exactly match a parameter or a top-level key emitted by an earlier step/u,
  );
  assert.match(
    guidance,
    /multiple records and side-effect IDs in arrays or unique wrapper keys/u,
  );
  assert.match(
    guidance,
    /statically parses and type-checks every workflow module without executing callbacks/u,
  );

  const skill = await readFile(
    join(packageRoot, "..", "..", "skills", "cori_save_workflow", "SKILL.md"),
    "utf8",
  );
  assert.match(
    skill,
    /State begins with the manifest parameters as top-level keys/u,
  );
  assert.match(
    skill,
    /object output is shallow-merged into that same\s+flat object/u,
  );
  assert.match(skill, /duplicate top-level output key overwrites/u);
});

test("support authoring keeps messages and created label IDs uniquely addressable", () => {
  const guidance = authoringReference(
    "gpt-5.4",
    "hybrid",
    "support_inbox_triage",
  );
  assert.match(guidance, /IDs in one message_ids array/u);
  assert.match(guidance, /all three fetched messages uniquely addressable/u);
  assert.match(
    guidance,
    /created category\/priority label ID uniquely addressable by message and label purpose/u,
  );
  assert.match(
    guidance,
    /Never reuse a shared message or label_id output key/u,
  );
});

test("direct and capture prompts keep live execution separate from workflow authoring", () => {
  const task = TASKS.find((candidate) =>
    candidate.id === "support_inbox_triage"
  )!;
  const scenario = buildScenario(task.id, 42, "author", "prompt-contract");
  const direct = renderedTaskPrompt(task.id, scenario, "direct");
  assert.match(
    direct,
    /Complete the live Workspace task now and verify the requested external state/u,
  );
  assert.match(direct, /read \.\/GWS\.md/u);
  assert.match(
    direct,
    /do not create a Cori workflow, manifest\.md, steps\/, or tests\//u,
  );
  assert.doesNotMatch(direct, /read \.\/CORI_AUTHORING\.md/u);
  assert.doesNotMatch(direct, /cori_save_workflow/u);
  const approval = approvalPrompt(task);
  assert.match(approval, /First read \.\/CORI_AUTHORING\.md/u);
  assert.match(
    approval,
    /"\$CORI_BENCH_CORI" check \.\/captured-workflow/u,
  );
  assert.match(approval, /Do not substitute another cori from PATH/u);
  assert.match(
    approval,
    /second argument may contain only stderr and exitCode, never workflow parameters or earlier outputs/u,
  );
  assert.doesNotMatch(approval, /fresh retry/u);

  const retryApproval = approvalPrompt(
    task,
    "qualification failed: SyntaxError at steps/06_read_presentation.ts:13:182",
  );
  assert.match(retryApproval, /fresh retry after a previous independent capture failed/u);
  assert.match(
    retryApproval,
    /SyntaxError at steps\/06_read_presentation\.ts:13:182/u,
  );
});

test("workflow authoring materials are staged only after direct task execution", async () => {
  const workspace = await mkdtemp(join(tmpdir(), "cori-benchmark-workspace-"));
  try {
    const scenario = buildScenario(
      "sla_breach_pack",
      42,
      "author",
      "workspace-staging",
    );
    await prepareDirectWorkspace(workspace, scenario.taskId, scenario);
    assert.match(
      await readFile(join(workspace, "TASK.md"), "utf8"),
      /This is task execution, not workflow authoring/u,
    );
    await assert.rejects(readFile(join(workspace, "CORI_AUTHORING.md"), "utf8"));
    await assert.rejects(
      readFile(
        join(
          workspace,
          ".agents",
          "skills",
          "cori_save_workflow",
          "SKILL.md",
        ),
        "utf8",
      ),
    );

    await prepareCaptureWorkspace(workspace, scenario.taskId, "gpt-5.4");
    assert.match(
      await readFile(join(workspace, "CORI_AUTHORING.md"), "utf8"),
      /# Cori authoring constraints/u,
    );
    assert.match(
      await readFile(
        join(
          workspace,
          ".agents",
          "skills",
          "cori_save_workflow",
          "SKILL.md",
        ),
        "utf8",
      ),
      /name: cori_save_workflow/u,
    );
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
});

test("every task builds valid author and held-out fixture contracts", () => {
  for (const task of TASKS) {
    for (const lane of ["author", "direct", "replay"] as const) {
      const scenario = buildScenario(
        task.id,
        42,
        lane,
        "fixture-contract-test",
      );
      assert.deepEqual(
        validateScenarioFixtures(scenario),
        [],
        `${task.id} ${lane}`,
      );
    }
  }
  const leads = buildScenario(
    "lead_follow_up_queue",
    42,
    "author",
    "lead-schema",
  );
  assert.deepEqual(leads.fixtures[0]?.table?.[0], [
    "lead_id",
    "status",
    "stage",
    "next_action_due",
    "value",
    "last_contact_at",
    "contact_name",
    "contact_email",
    "next_action",
    "benchmark_tag",
  ]);
  assert.ok(
    buildScenario("new_hire_onboarding_pack", 42, "author", "calendar-param")
      .parameters.calendar_id,
  );
  const support = buildScenario(
    "support_inbox_triage",
    42,
    "author",
    "support-contract",
  );
  assert.equal(
    support.resources.filter((resource) => resource.service === "gmail").length,
    3,
  );
  assert.match(
    TASKS.find((task) => task.id === "support_inbox_triage")!.prompt,
    /message_id, received_at, sender, subject, category, priority, status, summary, run_tag, as_of/u,
  );
});

test("full profile can be split into deterministic contiguous batches", () => {
  const base = { profile: "full", harness: "codex", seed: 42 } as const;
  assert.deepEqual(
    selectTasks({ ...base, batch: parseBatch("1/5") }).map((task) => task.id),
    TASKS.slice(0, 2).map((task) => task.id),
  );
  assert.deepEqual(
    selectTasks({ ...base, batch: parseBatch("5/5") }).map((task) => task.id),
    TASKS.slice(8, 10).map((task) => task.id),
  );
  assert.throws(() => parseBatch("5"), /INDEX\/COUNT/u);
});

test("twin scenarios preserve expected state and isolate resources", () => {
  const direct = buildScenario("sla_breach_pack", 42, "direct");
  const replay = buildScenario("sla_breach_pack", 42, "replay");
  assertTwinEquivalent(direct, replay);
  assert.notEqual(direct.runTag, replay.runTag);
  assert.equal(
    direct.expected.facts.join("|"),
    replay.expected.facts.join("|"),
  );
});

test("run namespaces prevent repeated seeds from reusing Workspace tags", () => {
  const first = buildScenario("support_inbox_triage", 42, "author", "run-one");
  const second = buildScenario("support_inbox_triage", 42, "author", "run-two");
  assert.notEqual(first.runTag, second.runTag);
});

test("reference workflows satisfy strict static safety policy", async () => {
  for (const task of TASKS) {
    const report = await inspectWorkflowPolicy(
      join(packageRoot, "reference-workflows", task.id),
    );
    assert.equal(
      report.ok,
      true,
      `${task.id}: ${report.violations.join("; ")}`,
    );
  }
  const ptoEventStep = await readFile(
    join(
      packageRoot,
      "reference-workflows",
      "preapproved_pto_processing",
      "steps",
      "03_create_pto_event.ts",
    ),
    "utf8",
  );
  assert.doesNotMatch(ptoEventStep, /eventType:\s*["']outOfOffice["']/u);
});

test("JSONL parser retains malformed process output as transcript evidence", () => {
  const events = parseJsonl(
    '{"session_id":"s1","usage":{"input_tokens":10}}\nnot-json\n',
  );
  assert.equal(events.length, 2);
  assert.deepEqual(events[1], { type: "unparsed", text: "not-json" });
});

test("GWS version ignores the CLI disclaimer line", async () => {
  const gws = new GwsClient(async () => ({
    code: 0,
    stdout: "gws 0.22.5\nThis is not an officially supported Google product.\n",
    stderr: "",
  }));
  assert.equal(await gws.version(), "gws 0.22.5");
});

test("Cori environment validation rejects an unrelated binary with the same name", () => {
  assert.equal(
    isCoriWorkflowCliHelp(
      "Preflight a workflow folder\nUsage: cori check [OPTIONS] <PATH>\n--update\n--yes",
    ),
    true,
  );
  assert.equal(
    isCoriWorkflowCliHelp(
      "Validate configuration files for consistency and correctness.\nUsage: cori check [OPTIONS]",
    ),
    false,
  );
});

test("benchmark defaults to the repository development Cori binary", () => {
  const expectedName = process.platform === "win32" ? "cori.exe" : "cori";
  assert.equal(
    workspaceCoriBinary(),
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../target/debug",
      expectedName,
    ),
  );
});

test("GWS client accepts successful 204-style empty responses", async () => {
  const gws = new GwsClient(async () => ({ code: 0, stdout: "", stderr: "" }));
  assert.equal(await gws.call(["gmail", "users", "drafts", "delete"]), null);
});

test("GWS client retries recognized transient API failures", async () => {
  let attempts = 0;
  const gws = new GwsClient(
    async () => {
      attempts += 1;
      return attempts < 3
        ? {
          code: 1,
          stdout: "",
          stderr: "error[api]: The service is currently unavailable.",
        }
        : { code: 0, stdout: '{"spreadsheetId":"sheet-1"}', stderr: "" };
    },
    "gws",
    async () => undefined,
  );
  assert.deepEqual(await gws.call(["sheets", "spreadsheets", "create"]), {
    spreadsheetId: "sheet-1",
  });
  assert.equal(attempts, 3);
});

test("benchmark calendar configuration requires a dedicated secondary calendar", () => {
  const previous = process.env[benchmarkCalendarEnv];
  try {
    delete process.env[benchmarkCalendarEnv];
    assert.equal(configuredBenchmarkCalendarId(), undefined);
    assert.throws(
      () => requireBenchmarkCalendarId(),
      /CORI_BENCH_CALENDAR_ID is required/u,
    );

    process.env[benchmarkCalendarEnv] = "  shared-calendar-1  ";
    assert.equal(requireBenchmarkCalendarId(), "shared-calendar-1");

    process.env[benchmarkCalendarEnv] = "primary";
    assert.throws(
      () => requireBenchmarkCalendarId(),
      /dedicated secondary calendar/u,
    );
  } finally {
    if (previous === undefined) delete process.env[benchmarkCalendarEnv];
    else process.env[benchmarkCalendarEnv] = previous;
  }
});

test("calendar fixtures reuse the configured calendar and cleanup never deletes it", async () => {
  const operations: string[] = [];
  const gws = new GwsClient(async (_file, args) => {
    const flagAt = args.findIndex((arg) => arg.startsWith("--"));
    const operation = args.slice(0, flagAt).join(" ");
    operations.push(operation);
    if (operation === "sheets spreadsheets create") {
      return {
        code: 0,
        stdout: JSON.stringify({ spreadsheetId: "sheet-1" }),
        stderr: "",
      };
    }
    if (
      operation === "sheets spreadsheets values update" ||
      operation === "drive files update"
    ) {
      return { code: 0, stdout: "{}", stderr: "" };
    }
    throw new Error(`unexpected fake GWS operation: ${operation}`);
  });
  const driver = new WorkspaceScenarioDriver(
    gws,
    async () => undefined,
    "shared-calendar-1",
  );
  const provisioned = await driver.provision(buildScenario(
    "preapproved_pto_processing",
    42,
    "author",
    "shared-calendar",
  ));
  const calendar = provisioned.resources.find((resource) =>
    resource.service === "calendar"
  );
  assert.deepEqual(calendar, {
    id: "shared-calendar-1",
    role: "PTO calendar",
    service: "calendar",
    createdByBenchmark: false,
  });
  assert.equal(provisioned.parameters.calendar_id, "shared-calendar-1");

  await driver.cleanup(provisioned.resources);
  await driver.cleanup([{
    ...calendar,
    createdByBenchmark: true,
  }]);
  assert.equal(
    operations.some((operation) => operation === "calendar calendars insert"),
    false,
  );
  assert.equal(
    operations.some((operation) => operation === "calendar calendars delete"),
    false,
  );
});

test("tag cleanup removes events from the configured calendar without deleting it", async () => {
  const calls: Array<{ operation: string; params: Record<string, Json> }> = [];
  const gws = new GwsClient(async (_file, args) => {
    const flagAt = args.findIndex((arg) => arg.startsWith("--"));
    const operation = args.slice(0, flagAt).join(" ");
    const paramsAt = args.indexOf("--params");
    const params = paramsAt >= 0
      ? JSON.parse(args[paramsAt + 1]!) as Record<string, Json>
      : {};
    calls.push({ operation, params });
    const body = operation === "calendar events list"
      ? { items: [{ id: "event-1" }] }
      : operation === "gmail users labels list"
        ? { labels: [] }
        : operation.endsWith(" list")
          ? {}
          : null;
    return {
      code: 0,
      stdout: body === null ? "" : JSON.stringify(body),
      stderr: "",
    };
  });
  const driver = new WorkspaceScenarioDriver(
    gws,
    async () => undefined,
    "shared-calendar-1",
  );
  await driver.cleanupTagged("cori-bench-test-tag");

  const eventList = calls.find((call) =>
    call.operation === "calendar events list"
  );
  assert.deepEqual(eventList?.params, {
    calendarId: "shared-calendar-1",
    q: "cori-bench-test-tag",
    singleEvents: false,
    showDeleted: false,
  });
  assert.ok(calls.some((call) =>
    call.operation === "calendar events delete" &&
    call.params.calendarId === "shared-calendar-1" &&
    call.params.eventId === "event-1"
  ));
  assert.equal(
    calls.some((call) => call.operation === "calendar calendars delete"),
    false,
  );
});

test("settled snapshots wait for tagged Drive output discovery", async () => {
  const base = buildScenario(
    "customer_meeting_prep",
    42,
    "author",
    "drive-settle",
  );
  const scenario: Scenario = {
    ...base,
    parameters: {
      ...base.parameters,
      calendar_id: "calendar-1",
      account_brief_id: "brief-1",
      source_message_id: "message-1",
    },
    resources: [
      {
        id: "calendar-1",
        role: "customer calendar",
        service: "calendar",
        createdByBenchmark: true,
      },
      {
        id: "brief-1",
        role: "account brief",
        service: "docs",
        createdByBenchmark: true,
      },
      {
        id: "message-1",
        role: "customer message",
        service: "gmail",
        createdByBenchmark: true,
      },
    ],
  };
  let driveLists = 0;
  let driveQuery = "";
  let calendarQuery = "";
  const gws = new GwsClient(async (_file, args) => {
    const paramsAt = args.indexOf("--params");
    const params = paramsAt >= 0
      ? JSON.parse(args[paramsAt + 1]!) as Record<string, string>
      : {};
    const operation = args.slice(0, paramsAt >= 0 ? paramsAt : 4).join(" ");
    let body: Json;
    if (operation === "calendar events list") {
      calendarQuery = params.q ?? "";
      body = {
        items: [{
          summary: `Acme renewal ${scenario.runTag}`,
          description: "Prep Doc",
        }],
      };
    } else if (operation === "docs documents get") {
      body = params.documentId === "output-doc"
        ? {
          title: `Prep ${scenario.runTag}`,
          body: {
            content: [{
              paragraph: {
                elements: [{
                  textRun: {
                    content:
                      `Objectives Account Facts Acme 120 SSO Risks security review delayed Questions ${scenario.runTag}`,
                  },
                }],
              },
            }],
          },
        }
        : { title: "Account brief", body: { content: [] } };
    } else if (operation === "gmail users messages get") {
      body = { id: "message-1", payload: { body: { data: "" } } };
    } else if (operation === "gmail users drafts list") {
      body = { drafts: [{ id: "draft-1" }] };
    } else if (operation === "gmail users drafts get") {
      body = { id: "draft-1", snippet: `Acme ${scenario.runTag}` };
    } else if (operation === "gmail users messages list") {
      body = { messages: [] };
    } else if (operation === "drive files list") {
      driveLists += 1;
      driveQuery = params.q ?? "";
      body = driveLists === 1
        ? { files: [] }
        : {
          files: [
            {
              id: "brief-1",
              name: `Account brief ${scenario.runTag}`,
              mimeType: "application/vnd.google-apps.document",
              trashed: false,
            },
            {
              id: "output-doc",
              name: `Prep ${scenario.runTag}`,
              mimeType: "application/vnd.google-apps.document",
              trashed: false,
            },
          ],
        };
    } else {
      throw new Error(`unexpected fake GWS operation: ${operation}`);
    }
    return { code: 0, stdout: JSON.stringify(body), stderr: "" };
  });
  const driver = new WorkspaceScenarioDriver(gws, async () => undefined);
  const snapshot = await driver.snapshot(
    scenario,
    { settleTaggedOutputs: true },
  );
  assert.equal(driveLists, 2);
  assert.match(driveQuery, /name contains/u);
  assert.equal(calendarQuery, scenario.runTag);
  assert.ok(snapshot.resources["__drive_file_output-doc"]);
});

test("Gmail fixture readiness requires the exact query-visible unread message", () => {
  assert.equal(
    gmailFixtureReady(
      { id: "message-1", labelIds: ["INBOX", "UNREAD"] },
      { messages: [{ id: "message-1" }] },
      "message-1",
    ),
    true,
  );
  assert.equal(
    gmailFixtureReady(
      { id: "message-1", labelIds: ["INBOX"] },
      { messages: [{ id: "message-1" }] },
      "message-1",
    ),
    false,
  );
  assert.equal(
    gmailFixtureReady(
      { id: "message-1", labelIds: ["INBOX", "UNREAD"] },
      { messages: [{ id: "other-message" }] },
      "message-1",
    ),
    false,
  );
});

test("Codex harness is isolated from user plugins and can reach Workspace", () => {
  const args = codexAutomationArgs();
  assert.equal(DEFAULT_CODEX_MODEL, "gpt-5.6-terra");
  assert.deepEqual(
    args.slice(args.indexOf("--model"), args.indexOf("--model") + 2),
    ["--model", codexModel()],
  );
  assert.ok(args.includes("--ignore-user-config"));
  assert.ok(args.includes("--dangerously-bypass-approvals-and-sandbox"));
  assert.equal(args.includes("--full-auto"), false);
});

test("workflow check diagnostics surface policy failures after a successful Cori check", () => {
  const diagnostic = formatWorkflowCheckFailure(
    { code: 0, stdout: "Result: ✓ ready", stderr: "" },
    {
      ok: false,
      violations: [
        "steps/08_apply_message_updates.ts reads workflow input property message_ids",
      ],
      workflowHash: "abc",
    },
  );
  assert.equal(
    diagnostic,
    "workflow policy failed: steps/08_apply_message_updates.ts reads workflow input property message_ids",
  );
});

test("support grading verifies queue rows and Gmail label transitions semantically", () => {
  const scenario = buildScenario(
    "support_inbox_triage",
    42,
    "author",
    "semantic-grade",
  );
  const sheetId =
    scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const messages = scenario.resources.filter((resource) =>
    resource.service === "gmail"
  );
  const fixtures = scenario.fixtures.filter((fixture) =>
    fixture.service === "gmail"
  );
  const rows = messages.map((message, index) => {
    const fixture = fixtures[index]!.messages![0] as Record<string, string>;
    return [
      message.id,
      "2026-07-13T08:00:00.000Z",
      "benchmark@example.test",
      fixture.subject!,
      fixture.expected_category!,
      fixture.expected_priority!,
      "triaged",
      "Factual summary",
      scenario.runTag,
      scenario.parameters.as_of!,
    ];
  });
  const labels = messages.flatMap((message, index) => {
    const fixture = fixtures[index]!.messages![0] as Record<string, string>;
    return [
      {
        id: `category-${index}`,
        name: `${scenario.runTag}/category/${fixture.expected_category}`,
      },
      {
        id: `priority-${index}`,
        name: `${scenario.runTag}/priority/${fixture.expected_priority}`,
      },
    ];
  });
  const capturedAt = "2026-07-13T09:00:00Z";
  const before: WorkspaceSnapshot = {
    capturedAt,
    resources: {
      [sheetId]: grid([["benchmark_tag"], [scenario.runTag]]),
      ...Object.fromEntries(
        messages.map((
          message,
        ) => [message.id, { labelIds: ["UNREAD", "INBOX"] }]),
      ),
      [`__drafts_${scenario.id}`]: {},
      [`__sent_${scenario.id}`]: {},
    },
    drafts: [],
    calendarEvents: [],
  };
  const after: WorkspaceSnapshot = {
    capturedAt,
    resources: {
      [sheetId]: grid([[
        "message_id",
        "received_at",
        "sender",
        "subject",
        "category",
        "priority",
        "status",
        "summary",
        "run_tag",
        "as_of",
      ], ...rows]),
      ...Object.fromEntries(
        messages.map((
          message,
          index,
        ) => [message.id, {
          labelIds: ["INBOX", `category-${index}`, `priority-${index}`],
        }]),
      ),
      [`__labels_${scenario.id}`]: { labels },
      [`__drafts_${scenario.id}`]: { drafts: [{ id: "draft-1" }] },
      [`__sent_${scenario.id}`]: {},
    },
    drafts: [{
      id: "draft-1",
      to: "support-lead@example.test",
      body:
        `${scenario.runTag} outage: 1 access: 1 how_to: 1 P0: 1 P1: 1 P2: 1`,
    }],
    calendarEvents: [],
  };
  const grade = gradeExternalState(scenario, before, after);
  assert.equal(grade.score, 100);
  assert.equal(grade.passed, true);

  const wrongHeader = structuredClone(after);
  const queue = wrongHeader.resources[sheetId] as {
    sheets: {
      data: { rowData: { values: { formattedValue: string }[] }[] }[];
    }[];
  };
  queue.sheets[0]!.data[0]!.rowData[0]!.values[8]!.formattedValue =
    "benchmark_tag";
  const wrongGrade = gradeExternalState(scenario, before, wrongHeader);
  assert.equal(
    wrongGrade.items.find((item) => item.id === "classification")?.earned,
    40,
  );
  assert.equal(wrongGrade.items.find((item) => item.id === "queue")?.earned, 0);
});

test("SLA grading verifies deadline and boundary flags from result rows", () => {
  const scenario = buildScenario(
    "sla_breach_pack",
    42,
    "author",
    "semantic-sla",
  );
  const sheetId =
    scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const docId =
    scenario.resources.find((resource) => resource.service === "docs")!.id;
  const tag = scenario.runTag;
  const rows = [
    [
      "case_id",
      "status",
      "priority",
      "opened_at",
      "sla_deadline",
      "breached",
      "due_within_two_hours",
      "run_tag",
    ],
    [
      "CASE-P0-BREACHED",
      "open",
      "P0",
      "2026-07-13T07:30:00Z",
      "2026-07-13T08:30:00.000Z",
      "true",
      "false",
      tag,
    ],
    [
      "CASE-P1-WARNING",
      "in_progress",
      "P1",
      "2026-07-13T05:30:00Z",
      "2026-07-13T09:30:00.000Z",
      "false",
      "true",
      tag,
    ],
    [
      "CASE-P2-WARNING",
      "open",
      "P2",
      "2026-07-12T10:30:00Z",
      "2026-07-13T10:30:00.000Z",
      "false",
      "true",
      tag,
    ],
    [
      "CASE-P3-BREACHED",
      "in_progress",
      "P3",
      "2026-07-10T08:59:00Z",
      "2026-07-13T08:59:00.000Z",
      "true",
      "false",
      tag,
    ],
    [
      "CASE-P1-HEALTHY",
      "open",
      "P1",
      "2026-07-13T08:00:00Z",
      "2026-07-13T12:00:00.000Z",
      "false",
      "false",
      tag,
    ],
  ];
  const table = {
    sheets: [{
      data: [{
        rowData: rows.map((row) => ({
          values: row.map((formattedValue) => ({ formattedValue })),
        })),
      }],
    }],
  };
  const before: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:00:00Z",
    resources: {
      [sheetId]: { sheets: [] },
      [docId]: { text: `SLA report template ${tag}` },
      [`__drive_file_${docId}`]: {
        text: `SLA report template ${tag}`,
      },
      [`__drafts_${scenario.id}`]: {},
      [`__sent_${scenario.id}`]: {},
    },
    drafts: [],
    calendarEvents: [],
  };
  const after: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:00:00Z",
    resources: {
      [sheetId]: table,
      [docId]: { text: `SLA report template ${tag}` },
      [`__drive_file_${docId}`]: {
        text: `SLA report template ${tag}`,
      },
      __drive_file_report: { text: `SLA report ${tag}` },
      [`__drafts_${scenario.id}`]: { drafts: [{ id: "draft-1" }] },
      [`__sent_${scenario.id}`]: {},
    },
    drafts: [{ id: "draft-1" }],
    calendarEvents: [],
  };
  const grade = gradeExternalState(scenario, before, after);
  assert.equal(grade.score, 100);
  assert.equal(grade.passed, true);
});

test("external-state grading ignores snapshot capture timestamps", () => {
  const scenario = buildScenario(
    "sla_breach_pack",
    42,
    "author",
    "timestamp-noop",
  );
  const sheetId =
    scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const docId =
    scenario.resources.find((resource) => resource.service === "docs")!.id;
  const resources: Record<string, Json> = {
    [sheetId]: { text: `Source ${scenario.runTag}` },
    [docId]: { text: `SLA report template ${scenario.runTag}` },
    [`__drafts_${scenario.id}`]: {},
    [`__sent_${scenario.id}`]: {},
  };
  const before: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:00:00Z",
    resources,
    drafts: [],
    calendarEvents: [],
  };
  const after: WorkspaceSnapshot = {
    ...before,
    capturedAt: "2026-07-13T09:01:00Z",
  };
  const grade = gradeExternalState(scenario, before, after);
  assert.equal(grade.score, 0);
  assert.equal(grade.passed, false);
  assert.ok(grade.items.every((item) => item.earned === 0));
});

test("task-specific semantic graders accept exact boundary outputs", () => {
  const lead = buildScenario(
    "lead_follow_up_queue",
    42,
    "author",
    "semantic-lead",
  );
  const leadQueue = [
    ["lead_id", "lead_score", "next_action", "run_tag"],
    ["LEAD-001", "80", "Send personalized follow-up", lead.runTag],
    ["LEAD-003", "80", "Send pricing", lead.runTag],
    ["LEAD-002", "70", "Confirm legal review", lead.runTag],
    ["LEAD-004", "60", "Book discovery", lead.runTag],
  ];
  const leadSource = lead.fixtures[0]!.table!.map((row) => [...row]);
  leadSource[1]![8] = "Send personalized follow-up";
  assert.equal(
    gradeSynthetic(lead, { source: grid(leadSource), queue: grid(leadQueue) }, [
      { to: "avery@example.test", body: lead.runTag },
    ]).score,
    100,
  );
  const wrongQueue = leadQueue.map((row) => [...row]);
  wrongQueue[1]![1] = "79";
  assert.equal(
    gradeSynthetic(
      lead,
      { source: grid(leadSource), queue: grid(wrongQueue) },
      [{ to: "avery@example.test", body: lead.runTag }],
    ).items.find((item) => item.id === "ranking")?.earned,
    0,
  );

  const pto = buildScenario(
    "preapproved_pto_processing",
    42,
    "author",
    "semantic-pto",
  );
  const ptoRows = pto.fixtures[0]!.table!.map((row) => [...row]);
  ptoRows[1]![2] = "scheduled";
  ptoRows[1]![8] = "8";
  ptoRows[1]![10] = "4";
  const validPtoEvent = {
    items: [{
      summary: `PTO - Riley Martin - ${pto.runTag}`,
      description: `Out of office: Riley Martin\nRun tag: ${pto.runTag}`,
      eventType: "default",
      start: { date: "2026-07-14" },
      end: { date: "2026-07-21" },
    }],
  };
  assert.equal(
    gradeSynthetic(pto, { pto: grid(ptoRows) }, [{
      to: "riley@example.test",
      body: pto.runTag,
    }], [validPtoEvent]).score,
    100,
  );
  assert.equal(
    gradeSynthetic(pto, { pto: grid(ptoRows) }, [{
      to: "riley@example.test",
      body: pto.runTag,
    }], [{
      ...validPtoEvent,
      items: [{ ...validPtoEvent.items[0]!, eventType: "outOfOffice" }],
    }]).items.find((item) => item.id === "calendar")?.earned,
    0,
  );

  const weekly = buildScenario(
    "weekly_operating_review",
    42,
    "author",
    "semantic-weekly",
  );
  const review = [
    ["project_id", "rag", "escalations", "run_tag"],
    ["PROJ-RED-BLOCKED", "red", "blocked", weekly.runTag],
    ["PROJ-RED-OVERDUE", "red", "overdue", weekly.runTag],
    ["PROJ-RED-PROGRESS", "red", "progress", weekly.runTag],
    ["PROJ-AMBER-BOUNDARY", "amber", "", weekly.runTag],
    ["PROJ-AMBER-PROGRESS", "amber", "", weekly.runTag],
    ["PROJ-GREEN", "green", "", weekly.runTag],
  ];
  assert.equal(
    gradeSynthetic(weekly, {
      review: grid(review),
      [`__drive_file_doc`]: {
        text: `Weekly Operating Review red amber green ${weekly.runTag}`,
      },
    }, [{ body: weekly.runTag }]).score,
    100,
  );

  const actions = buildScenario(
    "meeting_action_register",
    42,
    "author",
    "semantic-actions",
  );
  const actionRows = [
    ["action", "owner", "due_date", "source_section", "run_tag"],
    [
      "Publish the migration plan",
      "Alice",
      "2026-07-16",
      "Decisions",
      actions.runTag,
    ],
    ["Update the risk register", "Bob", "TBD", "Decisions", actions.runTag],
    [
      "Schedule the customer workshop",
      "Carol",
      "2026-07-20",
      "Follow-ups",
      actions.runTag,
    ],
  ];
  assert.equal(
    gradeSynthetic(actions, { tracker: grid(actionRows) }, [{
      body: actions.runTag,
    }]).score,
    100,
  );

  const expense = buildScenario(
    "expense_policy_audit",
    42,
    "author",
    "semantic-expense",
  );
  const audit = [
    ["expense_id", "audit", "reasons", "run_tag"],
    ["EXP-001", "PASS", "", expense.runTag],
    ["EXP-002", "FAIL", "missing_receipt", expense.runTag],
    ["EXP-003", "FAIL", "hotel_rate", expense.runTag],
    ["EXP-004", "FAIL", "meal_per_person", expense.runTag],
    ["EXP-005", "FAIL", "personal", expense.runTag],
    ["EXP-006", "FAIL", "duplicate_invoice", expense.runTag],
    ["EXP-007", "FAIL", "duplicate_invoice", expense.runTag],
  ];
  assert.equal(
    gradeSynthetic(expense, {
      audit: grid(audit),
      [`__drive_file_report`]: {
        text: `Expense Exceptions 6 ${expense.runTag}`,
      },
    }, [{ body: expense.runTag }]).score,
    100,
  );

  const budget = buildScenario(
    "budget_variance_deck",
    42,
    "author",
    "semantic-budget",
  );
  const slides = {
    slides: [
      {
        text:
          `Executive Summary ${budget.runTag}\nCloud variance +$150 (+15.00%)\nSubscriptions variance -$1,500 (-15.00%)\nNew Program variance +$500 (N/A)`,
      },
      { text: "Unfavorable Variances Cloud Subscriptions unfavorable" },
      { text: "Detail" },
    ],
  };
  assert.equal(
    gradeSynthetic(budget, { [`__drive_file_deck`]: slides }, [{
      body: budget.runTag,
    }]).score,
    100,
  );

  const hire = buildScenario(
    "new_hire_onboarding_pack",
    42,
    "author",
    "semantic-hire",
  );
  const hireRows = hire.fixtures[0]!.table!.map((row) => [...row]);
  hireRows[1]![1] = "prepared";
  hireRows[1]![8] = "true";
  hireRows[1]![9] = "https://docs.google.com/document/d/pack";
  hireRows[1]![10] = "https://calendar.google.com/event?id=event";
  assert.equal(
    gradeSynthetic(
      hire,
      {
        hires: grid(hireRows),
        [`__drive_file_${hire.resources[1]!.id}`]: {
          text:
            `Onboarding Pack {{NAME}} {{EMAIL}} {{MANAGER}} {{START_DATE}} {{RUN_TAG}} ${hire.runTag}`,
        },
        [`__drive_file_pack`]: {
          text:
            `Jordan Lee jordan.lee@example.test Morgan Patel 2026-07-20 ${hire.runTag}`,
        },
      },
      [{ to: "jordan.lee@example.test", body: hire.runTag }],
      [{
        items: [{
          summary: `${hire.runTag} Orientation`,
          start: {
            dateTime: "2026-07-20T07:00:00Z",
            timeZone: "Europe/Paris",
          },
          end: {
            dateTime: "2026-07-20T08:00:00Z",
            timeZone: "Europe/Paris",
          },
        }],
      }],
    ).score,
    100,
  );

  const meeting = buildScenario(
    "customer_meeting_prep",
    42,
    "author",
    "semantic-meeting",
  );
  assert.equal(
    gradeSynthetic(
      meeting,
      {
        [`__drive_file_prep`]: {
          text:
            `Acme 120 SSO security review delayed Objectives Account Facts Risks Questions ${meeting.runTag}`,
        },
      },
      [{ body: `Acme ${meeting.runTag}` }],
      [{
        items: [{
          description:
            `Prep https://docs.google.com/document/d/prep ${meeting.runTag}`,
        }],
      }],
    ).score,
    100,
  );
});

test("preview gate inspects executed commands, not documentation text", () => {
  assert.equal(
    transcriptExecutedCoriRun({
      transcript: [{ aggregated_output: "Run with cori run ./workflow" }],
    }),
    false,
  );
  assert.equal(
    transcriptExecutedCoriRun({
      transcript: [{
        item: { type: "command_execution", command: "cori run ./workflow" },
      }],
    }),
    true,
  );
});

test("capture evidence is task-scoped and an aggregate cannot reuse one task's workflow", () => {
  const grade = { score: 100, passed: true, safetyViolations: [], items: [] };
  const policy = { ok: true, violations: [], workflowHash: "abc" };
  const support = {
    taskId: "support_inbox_triage",
    authorGrade: grade,
    previewDidNotWrite: true,
    checkPassed: true,
    qualificationPassed: true,
    qualificationGrade: grade,
    policy,
    workflowPath: "/tmp/support",
  };
  const sla = {
    taskId: "sla_breach_pack",
    authorGrade: grade,
    previewDidNotWrite: true,
    checkPassed: false,
    qualificationPassed: false,
    policy,
    workflowPath: null,
  };
  const aggregate = aggregateCaptures([support, sla]);
  assert.equal(captureReady(support), true);
  assert.equal(captureReady(sla), false);
  assert.equal(aggregate.checkPassed, false);
  assert.equal(aggregate.tasks.length, 2);
  assert.equal(aggregate.policy, null);
});

test("capture readiness requires a perfect qualification score", () => {
  const lowGrade = {
    score: 30,
    passed: false,
    safetyViolations: [],
    items: [],
  };
  const policy = { ok: true, violations: [], workflowHash: "abc" };
  for (const task of TASKS) {
    const notReady = {
      taskId: task.id,
      authorGrade: lowGrade,
      previewDidNotWrite: true,
      checkPassed: true,
      qualificationPassed: true,
      qualificationGrade: lowGrade,
      policy,
      workflowPath: `/tmp/${task.id}`,
    };
    assert.equal(captureReady(notReady), false, task.id);
  }
  const ready = {
    taskId: "customer_meeting_prep",
    authorGrade: { ...lowGrade, score: 100, passed: true },
    previewDidNotWrite: true,
    checkPassed: true,
    qualificationPassed: true,
    qualificationGrade: { ...lowGrade, score: 100, passed: true },
    policy,
    workflowPath: "/tmp/customer-meeting-prep",
  };
  assert.equal(
    captureReady({
      ...ready,
      authorGrade: {
        ...lowGrade,
        safetyViolations: ["a tagged Gmail message has the SENT label"],
      },
    }),
    false,
  );
});

test("capture retries invalid workflows and replay-integrity failures but not safety or preview violations", () => {
  const grade = {
    score: 30,
    passed: false,
    safetyViolations: [],
    items: [],
  };
  const base = {
    taskId: "customer_meeting_prep",
    authorGrade: grade,
    previewDidNotWrite: true,
    checkPassed: false,
    policy: null,
    workflowPath: null,
  };
  assert.equal(retryableCaptureFailure(base), true);
  assert.equal(
    retryableCaptureFailure({
      ...base,
      checkPassed: true,
      qualificationPassed: false,
      qualificationGrade: {
        ...grade,
        safetyViolations: [
          "Cori replay failed or did not emit a successful JSON trace",
        ],
      },
    }),
    true,
  );
  assert.equal(
    retryableCaptureFailure({
      ...base,
      checkPassed: true,
      qualificationPassed: false,
      qualificationGrade: {
        ...grade,
        safetyViolations: ["a tagged Gmail message has the SENT label"],
      },
    }),
    false,
  );
  assert.equal(
    retryableCaptureFailure({
      ...base,
      authorGrade: {
        ...grade,
        safetyViolations: ["a tagged Gmail message has the SENT label"],
      },
    }),
    false,
  );
  assert.equal(
    retryableCaptureFailure({
      ...base,
      previewDidNotWrite: false,
      error: "preview gate detected workflow writes",
    }),
    false,
  );
});

test("Cori trace usage includes replay wall time", () => {
  assert.deepEqual(
    traceUsage({
      duration_ms: 1_234,
      cost: { input_tokens: 5, output_tokens: 7, total_eur: 0.01 },
    }),
    {
      wallTimeMs: 1_234,
      inputTokens: 5,
      outputTokens: 7,
      costEur: 0.01,
    },
  );
});

test("failed Cori traces preserve their concrete diagnostic for capture retries", () => {
  assert.equal(
    failedTraceDiagnostic(
      {
        status: "failed",
        error:
          "could not import steps/06_read_presentation.ts: SyntaxError: Expression expected at 13:182",
      },
      { code: 1, stdout: "ignored", stderr: "generic process output" },
    ),
    "could not import steps/06_read_presentation.ts: SyntaxError: Expression expected at 13:182",
  );
});

test("policy rejects the non-runtime SDK package name", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: sdk_import_test\nname: SDK import test\ndescription: Test invalid import.\ncreated: 2026-07-13\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "--version"] });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) => violation.includes("@cori-do/sdk")),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("policy rejects invented gws CLI flags before functional replay", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: invalid_gws_flag\nname: Invalid GWS flag\ndescription: Test invalid flag.\ncreated: 2026-07-15\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "sheets", "spreadsheets", "get", "--params", "{}", "--allow-already-exists"] });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("unsupported gws flag --allow-already-exists")
      ),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("policy rejects CLI parse functions that mistake metadata for workflow input", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: invalid_parse_context\nname: Invalid parse context\ndescription: Test parse context.\ncreated: 2026-07-15\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "sheets", "spreadsheets", "get", "--params", "{}"], parse: (_stdout, input) => ({ count: input.rows.length }) });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("workflow input property rows")
      ),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("policy rejects invalid Sheets userEnteredValue null clears", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: invalid_sheets_clear\nname: Invalid Sheets clear\ndescription: Test Sheets schema.\ncreated: 2026-07-15\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "sheets", "spreadsheets", "batchUpdate", "--params", "{}", "--json", JSON.stringify({ requests: [{ repeatCell: { cell: { userEnteredValue: null } } }] })], parse: () => ({ ok: true }) });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("userEnteredValue: null")
      ),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("policy rejects captured run tags and resource IDs in reusable runtime files", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    const runTag = "cori-bench-support-author-secret";
    const resourceId = "1FixtureSpreadsheetId";
    await writeFile(
      join(workflow, "manifest.md"),
      `---\nid: fixture_leak\nname: Fixture leak\ndescription: Test fixture leakage.\ncreated: 2026-07-15\nversion: 1\nparameters:\n  - name: run_tag\n    type: string\n    default: ${runTag}\ntools_required: [gws]\nmcp_servers: []\n---\n`,
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      `import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "sheets", "spreadsheets", "get", "--params", JSON.stringify({ spreadsheetId: "${resourceId}" })] });\n`,
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow, [runTag, resourceId]);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("manifest hard-codes captured fixture value")
      ),
    );
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("steps/01_test.ts hard-codes captured fixture value")
      ),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("policy rejects parameters outside the benchmark task contract", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: parameter_leak\nname: Parameter leak\ndescription: Test parameter contract.\ncreated: 2026-07-15\nversion: 1\nparameters:\n  - name: run_tag\n    type: string\n  - name: invented_input\n    type: string\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "--version"] });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow, [], [
      "run_tag",
      "as_of",
    ]);
    assert.equal(report.ok, false);
    assert.ok(
      report.violations.some((violation) =>
        violation.includes("missing: as_of; extra: invented_input")
      ),
    );
  } finally {
    await rm(workflow, { recursive: true, force: true });
  }
});

test("bootstrap and reuse decision use the publication threshold", () => {
  const ci = pairedDifferenceCi95([90, 91, 92], [90, 91, 93], 7, 1000);
  assert.ok(ci);
  assert.equal(reuseAdvantage(0, ci, 5, 10, 4), true);
  assert.equal(reuseAdvantage(1, ci, 5, 10, 4), false);
});

test("scorecard presents direct variability while requiring perfect Cori replays", () => {
  const result = sampleBenchmarkResult([
    sampleTrial(43, "direct", 70, ["sheet"], 61_026),
    sampleTrial(43, "replay", 100, [], 12_090),
    sampleTrial(44, "direct", 100, [], 59_282),
    sampleTrial(44, "replay", 100, [], 12_095),
    sampleTrial(45, "direct", 100, [], 61_445),
    sampleTrial(45, "replay", 100, [], 13_311),
  ]);
  const markdown = scorecard(result);
  assert.match(markdown, /Run status: \*\*completed\*\*/u);
  assert.match(markdown, /direct agents 30 points \(70–100\)/u);
  assert.match(markdown, /unchanged Cori replays 0 points \(100–100\)/u);
  assert.match(
    markdown,
    /\| Direct agent \| 90 \| 70–100 \| 2\/3 \| 2\/3 \| 60\.6s \| 479,569 \| n\/a \|/u,
  );
  assert.match(
    markdown,
    /\| Cori replay \| 100 \| 100–100 \| 3\/3 \| 3\/3 \| 12\.5s \| 0 \| \$0\.0000 \|/u,
  );
  assert.match(markdown, /\| lead_follow_up_queue \| 43 \| 70 \| 100 \| \+30 \| sheet \| none \|/u);
  assert.match(markdown, /Cori replays scored 100; direct-agent scores remain comparative measurements/u);
});

test("scorecard and CSV calculate token prices in USD", () => {
  const direct = sampleTrial(43, "direct", 100, [], 1_000);
  const replay = sampleTrial(43, "replay", 100, [], 1_000);
  direct.harness!.usage = {
    inputTokens: 1_000_000,
    outputTokens: 1_000_000,
    toolCalls: 3,
  };
  replay.runtime = {
    wallTimeMs: 1_000,
    inputTokens: 2_000_000,
    outputTokens: 0,
    costEur: 0.01,
  };
  const result = sampleBenchmarkResult([direct, replay]);

  const markdown = scorecard(result);
  assert.match(markdown, /\| Direct agent .* \| \$17\.5000 \|/u);
  assert.match(markdown, /\| Cori replay .* \| \$5\.0000 \|/u);
  assert.match(
    markdown,
    /Token prices use \$2\.50 per 1M input tokens and \$15\.00 per 1M output tokens\./u,
  );

  const [header, directRow, replayRow] = normalizedCsv(result).trim().split("\n");
  assert.equal(header?.split(",").at(-1), "price_usd");
  assert.equal(directRow?.split(",").at(-1), "17.5");
  assert.equal(replayRow?.split(",").at(-1), "5");
});

test("benchmark viewer keeps transcript, trace, snapshots, and workflow evidence together", () => {
  const result = sampleBenchmarkResult([
    sampleTrial(43, "direct", 80, ["draft"], 60_584),
    sampleTrial(43, "replay", 100, [], 12_499),
  ]);
  result.trials[1]!.tracePath = "/tmp/cori-traces/lead_follow_up_queue-0-0.json";
  result.trials[1]!.workflowHash = "workflow-hash";
  const document = benchmarkViewerDocument(result, [
    {
      path: "transcripts/authors/lead_follow_up_queue-direct.json",
      kind: "transcript",
      content: JSON.stringify({
        transcript: [
          { type: "user_message", message: { role: "user", content: "Review the source sheet" } },
          { type: "item.completed", item: { type: "agent_message", text: "I created the queue. Review https://docs.google.com/spreadsheets/d/example/edit." } },
          {
            type: "item.completed",
            item: {
              id: "tool-1",
              type: "command_execution",
              command: "gws sheets values batchUpdate",
              aggregated_output: "3 rows updated",
              status: "completed",
              exit_code: 0,
            },
          },
        ],
      }),
    },
    {
      path: "snapshots/lead_follow_up_queue-0-0-direct-after.json",
      kind: "snapshot",
      content: JSON.stringify({ resources: { sheet: { rows: 3 } } }),
    },
    {
      path: "cori-traces/lead_follow_up_queue-0-0.json",
      kind: "trace",
      content: JSON.stringify({
        code: 0,
        stdout: JSON.stringify({
          run_id: "cori-run-43",
          status: "succeeded",
          started_at: "2026-07-16T17:12:00.000Z",
          duration_ms: 12_499,
          activities: [
            { kind: "cli" },
            { kind: "llm" },
            { kind: "code" },
          ],
        }),
      }),
    },
    {
      path: "generated-workflows/lead_follow_up_queue/manifest.md",
      kind: "workflow",
      content: "# Lead follow-up queue",
    },
  ]);

  assert.match(document, /All benchmark artifacts/u);
  assert.match(document, /Agent exchange/u);
  assert.match(document, /Session table/u);
  assert.match(document, /All benchmark sessions/u);
  assert.match(document, /Paired Δ tokens/u);
  assert.match(document, /previous session in the same track/u);
  assert.match(document, /Cori replay · seed 43/u);
  assert.match(document, /"toolCalls":3/u);
  assert.match(document, /"cliCalls":1/u);
  assert.match(document, /Conversation/u);
  assert.match(document, /Captured workflow files/u);
  assert.match(document, /Review the source sheet/u);
  assert.match(document, /function appendTextWithLinks\(node, value\)/u);
  assert.match(document, /link\.href = linkText/u);
  assert.doesNotMatch(document, /link\.target = "_blank"/u);
  assert.match(document, /link\.rel = "noopener noreferrer"/u);
  assert.match(document, /splitLinkSuffix\(match\[0\]\)/u);
  assert.match(document, /Open raw file/u);
  assert.match(document, /gws sheets values batchUpdate/u);
  assert.match(document, /3 rows updated/u);
  assert.match(document, /Show tool output/u);
  assert.match(document, /key\.slice\(2\)\.toLowerCase\(\)/u);
  assert.doesNotMatch(document, /addEventListener\(key\.slice\(2\), value\)/u);
  assert.match(document, /artifact-filter-button/u);
  assert.match(document, /rows\\":3/u);
  assert.doesNotMatch(document, /<script>Review the source sheet/u);
  const interactiveScript = document.match(
    /<script>\n([\s\S]+?)\n  <\/script>/u,
  )?.[1];
  assert.ok(interactiveScript, "viewer should include its interaction script");
  assert.doesNotThrow(() => new Function(interactiveScript));
});

test("benchmark viewer normalizes messy logs without embedding large raw evidence", () => {
  const base = sampleBenchmarkResult([
    sampleTrial(43, "direct", 100, [], 60_584),
    sampleTrial(43, "replay", 100, [], 12_499),
  ]);
  const direct = base.trials[0]!;
  const rawOnly = "RAW_ONLY_MARKER".repeat(20_000);
  const result: BenchmarkResultV1 = {
    ...base,
    trials: [
      {
        ...direct,
        harness: {
          sessionId: "held-out-session",
          prompt: "Exact recorded benchmark prompt",
          transcript: [
            { type: "item.completed", item: { type: "agent_message", text: "Kept normalized message" } },
            { type: "item.completed", item: { type: "command_execution", command: "gws sheets spreadsheets get" } },
          ],
          usage: { inputTokens: 10, outputTokens: 5, toolCalls: 1 },
          wallTimeMs: 60_584,
          exitCode: 0,
          stdout: rawOnly,
          stderr: "",
        },
      },
      base.trials[1]!,
    ],
  };
  const spreadsheetUrl = "https://docs.google.com/spreadsheets/d/example/edit";
  const document = benchmarkViewerDocument(result, [
    {
      path: "transcripts/authors/lead_follow_up_queue-jsonl.json",
      kind: "transcript",
      content: [
        JSON.stringify({ type: "user_message", message: { role: "user", content: "JSONL user message" } }),
        "not-json-but-preserved",
        JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: "JSONL assistant message" } }),
      ].join("\n"),
    },
    {
      path: "snapshots/lead_follow_up_queue-large-after.json",
      kind: "snapshot",
      content: JSON.stringify({ spreadsheetUrl, rawOnly }),
    },
  ]);

  assert.match(document, /Kept normalized message/u);
  assert.match(document, /Exact recorded benchmark prompt/u);
  assert.match(document, /JSONL user message/u);
  assert.match(document, /JSONL assistant message/u);
  assert.match(document, /Open spreadsheet/u);
  assert.match(document, /https:\/\/docs\.google\.com\/spreadsheets\/d\/example\/edit/u);
  assert.ok((document.match(/RAW_ONLY_MARKER/gu) ?? []).length < 1_000);
  assert.match(document, /Preview truncated for performance/u);
  assert.ok(document.length < rawOnly.length, "viewer should be smaller than one omitted raw payload");
});

test("scorecard exposes capture retries and the selected author attempt", () => {
  const base = sampleBenchmarkResult([
    sampleTrial(43, "direct", 100, [], 61_026),
    sampleTrial(43, "replay", 100, [], 12_090),
  ]);
  const capture = base.capture.tasks[0]!;
  const failedGrade: Grade = {
    score: 30,
    passed: false,
    safetyViolations: [],
    items: [
      {
        id: "facts",
        earned: 0,
        max: 45,
        note: "missing snapshot evidence",
      },
      {
        id: "doc",
        earned: 0,
        max: 25,
        note: "missing snapshot evidence",
      },
    ],
  };
  const result: BenchmarkResultV1 = {
    ...base,
    capture: {
      ...base.capture,
      tasks: [{
        ...capture,
        attempts: [
          {
            attempt: 1,
            seed: 42,
            authorGrade: failedGrade,
            ready: false,
            error: "workflow policy failed",
          },
          {
            attempt: 2,
            seed: 100_042,
            authorGrade: capture.authorGrade,
            ready: true,
          },
        ],
        selectedAttempt: 2,
      }],
    },
  };
  const markdown = scorecard(result);
  assert.match(
    markdown,
    /\| lead_follow_up_queue \| 2 \| 2 \| #1: 30; #2: 100 \| #1: facts, doc; #2: ready \|/u,
  );
  assert.match(markdown, /\| Capture attempts \| 2 across 1 task\(s\); 1 retried \|/u);
});

test("atomic JSON artifacts tolerate concurrent writers", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cori-artifact-write-"));
  const path = join(directory, "result.json");
  try {
    await Promise.all([
      writeJson(path, { writer: 1 }),
      writeJson(path, { writer: 2 }),
    ]);
    const value = await readJson<{ writer: number }>(path);
    assert.ok(value.writer === 1 || value.writer === 2);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("direct score misses are nonfatal while sub-100 Cori replays fail", () => {
  const scoreMiss = sampleTrial(43, "direct", 70, ["sheet"], 61_026);
  assert.equal(trialIntegrityError([scoreMiss]), undefined);
  const replayScoreMiss = sampleTrial(
    43,
    "replay",
    70,
    ["sheet"],
    12_090,
  );
  assert.match(
    trialIntegrityError([scoreMiss, replayScoreMiss]) ?? "",
    /replay: scored 70\/100; expected 100 \(incomplete: sheet\)/u,
  );
  const replayIntegrityFailure = sampleTrial(
    43,
    "replay",
    0,
    ["ranking", "sheet", "draft"],
    12_090,
  );
  const safetyFailure = {
    ...replayIntegrityFailure,
    grade: {
      ...replayIntegrityFailure.grade,
      safetyViolations: [
        "Cori replay failed or did not emit a successful JSON trace",
      ],
    },
  };
  assert.match(
    trialIntegrityError([scoreMiss, safetyFailure]) ?? "",
    /1 benchmark Cori replay, safety, or replay-integrity failure/u,
  );
});

test("report upgrades legacy score-only failures to completed runs", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-benchmark-report-"));
  const runId = "legacy-score-only";
  const runDir = join(artifactsRoot, runId);
  await mkdir(runDir);
  const legacy = {
    ...sampleBenchmarkResult([
      sampleTrial(43, "direct", 70, ["sheet"], 61_026),
      sampleTrial(43, "replay", 100, [], 12_090),
    ]),
    status: "failed" as const,
    error:
      "1 benchmark trial(s) failed external-state or execution grading",
  };
  await writeFile(
    join(runDir, "result.json"),
    `${JSON.stringify(legacy, null, 2)}\n`,
    "utf8",
  );
  try {
    const regenerated = await report(runId, artifactsRoot);
    assert.equal(regenerated.status, "succeeded");
    assert.equal(regenerated.error, undefined);
    assert.match(
      await readFile(join(runDir, "scorecard.md"), "utf8"),
      /Run status: \*\*completed\*\*/u,
    );
    assert.match(
      await readFile(join(runDir, "viewer.html"), "utf8"),
      /All benchmark artifacts/u,
    );
  } finally {
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

test("failed harness startup still writes a terminal result artifact", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-benchmark-test-"));
  const previous = process.env.CORI_BENCH_CODEX_BIN;
  process.env.CORI_BENCH_CODEX_BIN = join(artifactsRoot, "missing-codex");
  try {
    await assert.rejects(
      runBenchmark({
        profile: "smoke",
        harness: "codex",
        seed: 8,
        artifactsRoot,
        runId: "missing-harness",
      }),
      /Benchmark artifacts were written/u,
    );
    const raw = await readFile(
      join(artifactsRoot, "missing-harness", "result.json"),
      "utf8",
    );
    const result = JSON.parse(raw) as { status: string; error?: string };
    assert.equal(result.status, "failed");
    assert.match(result.error ?? "", /cannot find codex harness executable/u);
    const progress = JSON.parse(
      await readFile(
        join(artifactsRoot, "missing-harness", "progress.json"),
        "utf8",
      ),
    ) as { status: string; phase: string };
    assert.deepEqual(progress, {
      ...progress,
      status: "failed",
      phase: "failed",
    });
  } finally {
    if (previous === undefined) delete process.env.CORI_BENCH_CODEX_BIN;
    else process.env.CORI_BENCH_CODEX_BIN = previous;
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

test("hybrid run fails before provisioning when the runtime model is missing", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-benchmark-test-"));
  const previousBinary = process.env.CORI_BENCH_CODEX_BIN;
  const previousModel = process.env.CORI_BENCH_LLM_MODEL;
  process.env.CORI_BENCH_CODEX_BIN = process.execPath;
  delete process.env.CORI_BENCH_LLM_MODEL;
  try {
    await assert.rejects(
      runBenchmark({
        profile: "smoke",
        harness: "codex",
        seed: 8,
        artifactsRoot,
        runId: "missing-model",
      }),
      /CORI_BENCH_LLM_MODEL is required/u,
    );
    const raw = await readFile(
      join(artifactsRoot, "missing-model", "result.json"),
      "utf8",
    );
    const result = JSON.parse(raw) as { status: string; error?: string };
    assert.equal(result.status, "failed");
    assert.match(result.error ?? "", /CORI_BENCH_LLM_MODEL is required/u);
  } finally {
    if (previousBinary === undefined) delete process.env.CORI_BENCH_CODEX_BIN;
    else process.env.CORI_BENCH_CODEX_BIN = previousBinary;
    if (previousModel === undefined) delete process.env.CORI_BENCH_LLM_MODEL;
    else process.env.CORI_BENCH_LLM_MODEL = previousModel;
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

function gradeSynthetic(
  scenario: Scenario,
  resources: Record<string, Json>,
  drafts: Json[] = [],
  calendarEvents: Json[] = [],
) {
  const before: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:00:00Z",
    resources: {
      baseline: { value: "before" },
      [`__drafts_${scenario.id}`]: {},
      [`__sent_${scenario.id}`]: {},
    },
    drafts: [],
    calendarEvents: [],
  };
  const after: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:01:00Z",
    resources: {
      ...resources,
      [`__drafts_${scenario.id}`]: drafts.length > 0
        ? { drafts: [{ id: "draft-1" }] }
        : {},
      [`__sent_${scenario.id}`]: {},
    },
    drafts,
    calendarEvents,
  };
  return gradeExternalState(scenario, before, after);
}

function sampleBenchmarkResult(
  trials: readonly TrialResult[],
): BenchmarkResultV1 {
  const direct = trials.filter((trial) => trial.lane === "direct");
  const replay = trials.filter((trial) => trial.lane === "replay");
  const mean = (values: readonly number[]) =>
    values.length === 0
      ? null
      : values.reduce((sum, value) => sum + value, 0) / values.length;
  const completeGrade: Grade = {
    score: 100,
    passed: true,
    safetyViolations: [],
    items: [],
  };
  return {
    version: 1,
    status: "succeeded",
    runId: "sample-run",
    profile: "full",
    harness: "codex",
    seed: 42,
    startedAt: "2026-07-16T17:10:13.174Z",
    finishedAt: "2026-07-16T17:21:15.414Z",
    environment: {
      cori: "cori",
      gws: "gws",
      author_model: "gpt-5.6-terra",
      llm_model: "gpt-5.4",
      os: "darwin",
      timezone: "Europe/Paris",
    },
    capture: {
      previewDidNotWrite: true,
      checkPassed: true,
      policy: { ok: true, violations: [], workflowHash: "abc" },
      tasks: [{
        taskId: "lead_follow_up_queue",
        authorGrade: completeGrade,
        previewDidNotWrite: true,
        checkPassed: true,
        qualificationPassed: true,
        qualificationGrade: completeGrade,
        policy: { ok: true, violations: [], workflowHash: "abc" },
        workflowPath: "/tmp/captured-workflow",
      }],
    },
    trials,
    metrics: {
      directWallTimeMs: 60_584,
      replayWallTimeMs: 12_499,
      designTokens: 479_569,
      runtimeTokens: 0,
      runtimeCostEur: 0,
      breakEvenRepetitions: 2,
    },
    summary: {
      directScore: mean(direct.map((trial) => trial.grade.score)),
      replayScore: mean(replay.map((trial) => trial.grade.score)),
      pairedDifferenceCi95: [0, 30],
      reuseAdvantageDemonstrated: true,
    },
  };
}

function sampleTrial(
  seed: number,
  lane: TrialResult["lane"],
  score: number,
  incompleteItems: readonly string[],
  wallTimeMs: number,
): TrialResult {
  const itemIds = ["ranking", "sheet", "draft"];
  const points: Record<string, number> = {
    ranking: 50,
    sheet: 30,
    draft: 20,
  };
  return {
    taskId: "lead_follow_up_queue",
    seed,
    lane,
    grade: {
      score,
      passed: score >= 90,
      safetyViolations: [],
      items: itemIds.map((id) => ({
        id,
        earned: incompleteItems.includes(id) ? 0 : points[id]!,
        max: points[id]!,
        note: incompleteItems.includes(id)
          ? `missing snapshot evidence: ${id}`
          : "verified from Workspace snapshot",
      })),
    },
    ...(lane === "direct"
      ? {
          harness: {
            sessionId: `session-${seed}`,
            transcript: [],
            usage: { inputTokens: null, outputTokens: null, toolCalls: null },
            wallTimeMs,
            exitCode: 0,
            stdout: "",
            stderr: "",
          },
        }
      : {
          runtime: {
            wallTimeMs,
            inputTokens: 0,
            outputTokens: 0,
            costEur: 0,
          },
        }),
  };
}

function grid(table: readonly (readonly string[])[]): Json {
  return {
    sheets: [{
      data: [{
        rowData: table.map((row) => ({
          values: row.map((formattedValue) => ({ formattedValue })),
        })),
      }],
    }],
  };
}

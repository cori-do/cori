import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { delimiter, dirname, join, resolve } from "node:path";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  codexAutomationArgs,
  codexModel,
  CodexAdapter,
  DEFAULT_CODEX_MODEL,
  parseJsonl,
} from "../src/harness.js";
import type { HarnessAdapter } from "../src/harness.js";
import { gradeExternalState } from "../src/grader.js";
import {
  benchmarkCalendarEnv,
  configuredBenchmarkCalendarId,
  fixtureWriteRange,
  gmailFixtureReady,
  GwsClient,
  messageIsUnread,
  parseGwsAuditLog,
  requireBenchmarkCalendarId,
  WorkspaceScenarioDriver,
} from "../src/gws.js";
import { inspectWorkflowPolicy } from "../src/policy.js";
import {
  benchmarkDiagnosticOutput,
  benchmarkProgressOutput,
  benchmarkProgressText,
} from "../src/progress.js";
import { normalizedCsv, readJson, scorecard, writeJson } from "../src/artifacts.js";
import {
  aggregateCaptures,
  approvalPrompt,
  assertResultCoriIdentity,
  captureAuditHasNoMutations,
  captureConversationTurns,
  captureReady,
  captureRequestPrompt,
  cleanup,
  combineRuns,
  createBenchmarkHarnessEnvironment,
  failedTraceDiagnostic,
  formatWorkflowCheckFailure,
  hardGate,
  inferentiallyEligible,
  isCanonicalCoriReadyOutput,
  isCoriWorkflowCliHelp,
  missingRuntimeModelFailure,
  parseBatch,
  profilePairs,
  prepareCaptureWorkspace,
  prepareDirectWorkspace,
  previewHadNoSideEffects,
  probeHarnessCoriEnvironment,
  renderedTaskPrompt,
  report,
  runBenchmark,
  selectTasks,
  traceRanRuntimeModel,
  traceUsage,
  trialIntegrityError,
  transcriptExecutedCoriRun,
  transcriptExecutedCoriCheck,
  transcriptHasWorkflowPreview,
  transcriptSuccessfulCoriCheck,
  validateCoriExecutableProbe,
  workspaceCoriBinary,
} from "../src/runner.js";
import {
  assertHybridBanksAreRegexResistant,
  assertRegexResistant,
  assertSeedsProduceDistinctFixtures,
  assertTwinEquivalent,
  buildScenario,
  validateScenarioFixtures,
} from "../src/scenario.js";
import { pairedDifferenceCi95, reuseAdvantage } from "../src/statistics.js";
import { assertTaskCatalog, TASKS } from "../src/tasks.js";
import {
  benchmarkViewerDocument,
  writeBenchmarkViewer,
} from "../src/viewer.js";
import type {
  BenchmarkResultV2,
  Grade,
  Json,
  Scenario,
  TaskCapture,
  TrialResult,
  WorkspaceSnapshot,
} from "../src/types.js";

const packageRoot = join(fileURLToPath(new URL("../..", import.meta.url)));

test("task catalog contains ten 100-point tasks", () => {
  assertTaskCatalog();
  assert.equal(TASKS.length, 10);
  assert.equal(
    TASKS.filter((task) => task.runtimeTrack === "deterministic").length,
    5,
  );
  assert.equal(
    TASKS.filter((task) => task.runtimeTrack === "hybrid").length,
    5,
  );
  // The hybrid track is a claim about the data, so it has to be enforceable.
  for (const task of TASKS) {
    assert.equal(
      task.requiresRuntimeModel === true,
      task.runtimeTrack === "hybrid",
      `${task.id} must declare requiresRuntimeModel exactly on the hybrid track`,
    );
  }
});

test("no task prompt states the answer for its own fixture", () => {
  for (const task of TASKS.filter((candidate) => candidate.requiresRuntimeModel)) {
    const scenario = buildScenario(task.id, 42, "author", "prompt-leak");
    const prompt = task.prompt.toLowerCase();
    for (const record of scenario.expected.groundTruth) {
      for (const [name, value] of Object.entries(record.fields)) {
        // Enum answers legitimately name their own vocabulary in the policy;
        // free values must never appear, or the prompt is the answer key.
        if (value.length < 4 || /^(true|false|p0|p1|p2)$/u.test(value)) continue;
        if (["category", "priority", "band", "status", "party", "metric"].includes(name)) continue;
        assert.ok(
          !prompt.includes(value.toLowerCase()),
          `${task.id} prompt contains the expected ${name} value "${value}"`,
        );
      }
    }
  }
});

test("the staged real skill owns the workflow dataflow contract", async () => {
  const skill = await readFile(
    join(packageRoot, "..", "..", "skills", "cori-save-workflow", "SKILL.md"),
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

test("the staged real skill preserves explicit parameter contracts", async () => {
  const skill = await readFile(
    join(packageRoot, "..", "..", "skills", "cori-save-workflow", "SKILL.md"),
    "utf8",
  );
  assert.match(
    skill,
    /explicit `Parameters`, `Inputs`, or run-arguments list is authoritative/u,
  );
  assert.match(
    skill,
    /Do not add another parameter merely\s+because a fixed requirement could be made configurable/u,
  );
  assert.match(
    skill,
    /manifest parameter names with any explicit input[\s\S]*must match exactly/u,
  );
});

test("the staged real skill requires valid Gmail raw-message separators", async () => {
  const skillRoot = join(
    packageRoot,
    "..",
    "..",
    "skills",
    "cori-save-workflow",
  );
  const [skill, activityKinds] = await Promise.all([
    readFile(join(skillRoot, "SKILL.md"), "utf8"),
    readFile(join(skillRoot, "references", "activity_kinds.md"), "utf8"),
  ]);
  assert.match(skill, /Gmail raw messages use real RFC line breaks/u);
  assert.ok(activityKinds.includes(String.raw`].join("\r\n");`));
  assert.ok(
    activityKinds.includes(String.raw`message.includes("\\r\\n")`),
  );
  assert.match(
    activityKinds,
    /Gmail raw message must use real CRLF separators/u,
  );
});

test("capture uses the natural skill request and literal approval", () => {
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
  // This task declares a re-run contract, so it must not be told its fixtures
  // are always fresh; being safe to run twice is part of what it is scored on.
  assert.match(direct, /honour the re-run rules stated above/u);
  assert.doesNotMatch(
    direct,
    /Do not add stale-state cleanup or cross-run already-exists guards/u,
  );
  assert.match(direct, /The source content differs every time this job runs/u);
  assert.match(
    direct,
    /do not create a Cori workflow, manifest\.md, steps\/, or tests\//u,
  );
  const fresh = renderedTaskPrompt(
    "sla_breach_pack",
    buildScenario("sla_breach_pack", 42, "author", "prompt-contract"),
    "direct",
  );
  assert.match(fresh, /resources are freshly provisioned[\s\S]*run tag is unique/u);
  assert.doesNotMatch(fresh, /The source content differs every time this job runs/u);
  assert.doesNotMatch(direct, /read \.\/CORI_AUTHORING\.md/u);
  assert.doesNotMatch(direct, /cori-save-workflow/u);
  assert.equal(
    captureRequestPrompt(),
    "Save this as a Cori workflow under ./captured-workflow.",
  );
  assert.equal(approvalPrompt(), "yes");
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
          "cori-save-workflow",
          "SKILL.md",
        ),
        "utf8",
      ),
    );

    await prepareCaptureWorkspace(workspace);
    await assert.rejects(readFile(join(workspace, "CORI_AUTHORING.md"), "utf8"));
    assert.match(
      await readFile(
        join(
          workspace,
          ".agents",
          "skills",
          "cori-save-workflow",
          "SKILL.md",
        ),
        "utf8",
      ),
      /name: cori-save-workflow/u,
    );
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
});

test("capture resumes one conversation for task, preview, and approval", async () => {
  const calls: Array<{ sessionId: string; prompt: string }> = [];
  const session = (sessionId: string, prompt: string) => ({
    sessionId,
    prompt,
    transcript: [],
    usage: { inputTokens: 1, outputTokens: 1, toolCalls: 0 },
    wallTimeMs: 1,
    exitCode: 0,
    stdout: "",
    stderr: "",
  });
  const adapter: HarnessAdapter = {
    name: "codex",
    identity: async () => ({
      command: "codex",
      path: "/tmp/codex",
      sha256: "a".repeat(64),
      version: "test",
    }),
    version: async () => "test",
    start: async (prompt) => session("author-session", prompt),
    resume: async (sessionId, prompt) => {
      calls.push({ sessionId, prompt });
      return session(
        prompt === approvalPrompt() ? "approval-session" : "preview-session",
        prompt,
      );
    },
  };
  let previewObserved = false;
  await captureConversationTurns(adapter, "author-session", "/tmp", {
    afterPreview: () => {
      previewObserved = true;
    },
  });
  assert.equal(previewObserved, true);
  assert.deepEqual(calls, [
    { sessionId: "author-session", prompt: captureRequestPrompt() },
    { sessionId: "preview-session", prompt: "yes" },
  ]);
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
  assert.ok(
    buildScenario("preapproved_pto_processing", 42, "author", "calendar-param")
      .parameters.calendar_id,
  );
  const support = buildScenario(
    "support_inbox_triage",
    42,
    "author",
    "support-contract",
  );
  // The daily volume is not a constant, and one fixture provisions the whole
  // inbox rather than one message per blueprint entry.
  const inbox = support.resources.filter((resource) => resource.service === "gmail");
  assert.ok(inbox.length >= 9, `expected a realistic inbox, got ${inbox.length}`);
  assert.equal(inbox.length, support.expected.groundTruth.length);
  assert.ok(
    support.expected.groundTruth.some((record) => record.fields.skip === "true"),
    "a re-run task must provision state left by a previous run",
  );
  assert.deepEqual(support.fixtures[0]?.table, [
    ["benchmark_tag"],
    [support.runTag],
  ]);
  assert.equal(fixtureWriteRange(support.fixtures[0]!.table!), "Source!A1:A2");
  assert.match(
    TASKS.find((task) => task.id === "support_inbox_triage")!.prompt,
    /The daily volume varies and is not fixed/u,
  );
  assert.match(
    TASKS.find((task) => task.id === "support_inbox_triage")!.prompt,
    /message_id, received_at, sender, subject, category, priority, status, run_tag, as_of/u,
  );
});

test("held-out seeds pose different problems, not the same one relabelled", () => {
  for (const task of TASKS) {
    assertSeedsProduceDistinctFixtures(task.id, [88, 89, 90, 91]);
  }
});

test("hybrid fixture banks cannot be separated by a single literal", () => {
  assertHybridBanksAreRegexResistant();
  // The check must reject the kind of fixture that let a keyword matcher score
  // this benchmark before: one member per class, each with a unique giveaway.
  assert.throws(
    () =>
      assertRegexResistant([
        { text: "Checkout unavailable for all customers, HTTP 503 at checkout.", label: "outage" },
        { text: "Administrator cannot sign in and needs account access restored.", label: "access" },
        { text: "Please share the steps to export the monthly report as CSV.", label: "how_to" },
      ], "separable bank"),
    /separates class/u,
  );
});

test("v2 profiles plan exactly 1, 1, and 3 held-out pairs per task", () => {
  assert.equal(profilePairs("smoke"), 1);
  assert.equal(profilePairs("full"), 1);
  assert.equal(profilePairs("publication"), 3);
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

test("terminated harness turns leave readable partial transcript evidence", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cori-harness-timeout-"));
  const executable = join(directory, "slow-codex");
  const partialPath = join(directory, "partial.json");
  const previous = process.env.CORI_BENCH_CODEX_BIN;
  try {
    await writeFile(
      executable,
      '#!/bin/sh\nprintf \'{"session_id":"partial-session","type":"agent_message","text":"working"}\\n\'\nsleep 5\n',
      "utf8",
    );
    await chmod(executable, 0o755);
    process.env.CORI_BENCH_CODEX_BIN = executable;
    const result = await new CodexAdapter().start("task", directory, {
      timeoutMs: 300,
      onProgress: async (partial) => writeJson(partialPath, partial),
    });
    assert.equal(result.exitCode, 124);
    assert.equal(result.timedOut, true);
    const partial = await readJson<{ stdout: string; wallTimeMs: number }>(partialPath);
    assert.match(partial.stdout, /partial-session/u);
    assert.ok(partial.wallTimeMs >= 0);
  } finally {
    if (previous === undefined) delete process.env.CORI_BENCH_CODEX_BIN;
    else process.env.CORI_BENCH_CODEX_BIN = previous;
    await rm(directory, { recursive: true, force: true });
  }
});

test("an interrupted run remains readable and can clean up without result.json", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-partial-run-"));
  const runId = "partial-run";
  const runDir = join(artifactsRoot, runId);
  try {
    await writeJson(join(runDir, "progress.json"), {
      version: 2,
      status: "running",
      phase: "capture_preview",
      detail: "skill is preparing the preview (30s elapsed)",
    });
    await writeJson(join(runDir, "transcripts", "authors", "partial.json"), {
      status: "running",
      stdout: '{"type":"agent_message","text":"partial"}\n',
    });
    await writeJson(join(runDir, "cleanup-registry.json"), {
      runId,
      resources: [],
      runTags: [],
    });
    await cleanup(runId, artifactsRoot);
    assert.match(
      await readFile(join(runDir, "progress.json"), "utf8"),
      /capture_preview/u,
    );
    assert.match(
      await readFile(join(runDir, "transcripts", "authors", "partial.json"), "utf8"),
      /partial/u,
    );
  } finally {
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

test("artifact commands reject traversal run IDs before reading run data", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-run-id-traversal-"));
  try {
    const attempts = [
      () => cleanup("../outside", artifactsRoot),
      () => report("../outside", artifactsRoot),
      () => writeBenchmarkViewer("../outside", artifactsRoot),
      () =>
        combineRuns(
          ["missing-valid-run", "../outside"],
          artifactsRoot,
        ),
    ];
    for (const attempt of attempts) {
      await assert.rejects(
        attempt(),
        /run ID must contain only letters, numbers, dots, underscores, and hyphens/u,
      );
    }
  } finally {
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

test("artifact commands reject valid-looking run IDs that symlink outside the artifacts root", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-run-id-root-"));
  const outside = await mkdtemp(join(tmpdir(), "cori-run-id-outside-"));
  const runId = "linked-run";
  try {
    await symlink(outside, join(artifactsRoot, runId), "dir");
    const attempts = [
      () => cleanup(runId, artifactsRoot),
      () => report(runId, artifactsRoot),
      () => writeBenchmarkViewer(runId, artifactsRoot),
      () => combineRuns([runId, runId], artifactsRoot),
    ];
    for (const attempt of attempts) {
      await assert.rejects(
        attempt(),
        /run ID resolves outside the artifacts root/u,
      );
    }
  } finally {
    await rm(artifactsRoot, { recursive: true, force: true });
    await rm(outside, { recursive: true, force: true });
  }
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

test(
  "benchmark environment pins the selected Cori ahead of a conflicting binary",
  async () => {
    const directory = await mkdtemp(join(tmpdir(), "cori-login-pin-"));
    const selectedDir = join(directory, "selected");
    const conflictDir = join(directory, "conflict");
    const selected = join(selectedDir, "cori");
    try {
      await mkdir(selectedDir);
      await mkdir(conflictDir);
      await writeFile(
        selected,
        [
          "#!/bin/sh",
          'if [ "$1" = "--version" ]; then echo "cori repository-test"; exit 0; fi',
          'if [ "$1" = "check" ] && [ "$2" = "--help" ]; then',
          "  printf 'Preflight a workflow folder\\nUsage: cori check [OPTIONS] <PATH>\\n--update\\n--yes\\n'",
          "  exit 0",
          "fi",
          "exit 1",
          "",
        ].join("\n"),
        "utf8",
      );
      await writeFile(
        join(conflictDir, "cori"),
        "#!/bin/sh\necho 'cori 0.6.6'\n",
        "utf8",
      );
      await chmod(selected, 0o755);
      await chmod(join(conflictDir, "cori"), 0o755);
      const environment = await createBenchmarkHarnessEnvironment(
        directory,
        selected,
        {
          ...process.env,
          PATH: [conflictDir, process.env.PATH ?? ""].join(delimiter),
        },
      );
      const digest = createHash("sha256")
        .update(await readFile(selected))
        .digest("hex");
      const identity = await probeHarnessCoriEnvironment(environment, {
        path: selected,
        version: "cori repository-test",
        sha256: digest,
      });
      assert.equal(identity.path, selected);
      assert.equal(environment.CORI_BENCH_CORI, selected);
      assert.equal(environment.PATH?.split(delimiter)[0], selectedDir);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  },
);

test("author Cori probe rejects path, help, version, and digest mismatches", () => {
  const selected = {
    path: "/repo/target/debug/cori",
    version: "cori 0.2.4",
    sha256: "a".repeat(64),
  };
  const valid = {
    ...selected,
    help:
      "Preflight a workflow folder\nUsage: cori check [OPTIONS] <PATH>\n--update\n--yes",
  };
  validateCoriExecutableProbe(selected, valid);
  assert.throws(
    () => validateCoriExecutableProbe(selected, { ...valid, path: "/usr/local/bin/cori" }),
    /path mismatch/u,
  );
  assert.throws(
    () => validateCoriExecutableProbe(selected, { ...valid, help: "Usage: cori check [OPTIONS]" }),
    /help mismatch/u,
  );
  assert.throws(
    () => validateCoriExecutableProbe(selected, { ...valid, version: "cori 0.6.6" }),
    /version mismatch/u,
  );
  assert.throws(
    () => validateCoriExecutableProbe(selected, { ...valid, sha256: "b".repeat(64) }),
    /digest mismatch/u,
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
        : { code: 0, stdout: '{"sheets":[]}', stderr: "" };
    },
    "gws",
    async () => undefined,
  );
  assert.deepEqual(await gws.call(["sheets", "spreadsheets", "get"]), {
    sheets: [],
  });
  assert.equal(attempts, 3);
});

test("GWS client never blindly retries an ambiguously failed mutation", async () => {
  let attempts = 0;
  const gws = new GwsClient(
    async () => {
      attempts += 1;
      return {
        code: 1,
        stdout: "",
        stderr: "error[api]: The service is currently unavailable.",
      };
    },
    "gws",
    async () => undefined,
  );
  await assert.rejects(
    gws.call(
      ["gmail", "users", "drafts", "create"],
      { userId: "me" },
      { message: { raw: "dGVzdA" } },
    ),
    /service is currently unavailable/u,
  );
  assert.equal(attempts, 1);
});

test("GWS audit parsing fails closed on malformed or partial JSONL", () => {
  const valid = JSON.stringify({
    argv: ["drive", "files", "list"],
    cwd: "/benchmark",
    at: "2026-07-13T09:00:00Z",
    pid: 123,
  });
  assert.deepEqual(parseGwsAuditLog(`${valid}\n`), {
    complete: true,
    events: [{
      argv: ["drive", "files", "list"],
      cwd: "/benchmark",
      at: "2026-07-13T09:00:00Z",
      pid: 123,
    }],
  });
  const corrupt = parseGwsAuditLog(`${valid}\n{"argv":`);
  assert.equal(corrupt.complete, false);
  assert.equal(corrupt.events.length, 1);
});

test("GWS authentication probe is read-only and explains invalid_rapt", async () => {
  const calls: string[][] = [];
  const gws = new GwsClient(async (_file, args) => {
    calls.push([...args]);
    return {
      code: 1,
      stdout: "",
      stderr:
        "error[auth]: invalid_grant: reauth related error (invalid_rapt)",
    };
  });
  await assert.rejects(
    gws.verifyAuthentication(),
    /gws auth login --services drive,gmail,sheets,docs,calendar,slides/u,
  );
  assert.deepEqual(calls, [[
    "drive",
    "about",
    "get",
    "--params",
    '{"fields":"user(permissionId,emailAddress)"}',
    "--format",
    "json",
  ]]);
});

test("GWS authentication probe exposes only a stable account fingerprint", async () => {
  const gws = new GwsClient(async () => ({
    code: 0,
    stdout: JSON.stringify({
      user: {
        permissionId: "drive-principal-123",
        emailAddress: "private@example.test",
      },
    }),
    stderr: "",
  }));
  assert.equal(
    await gws.verifyAuthentication(),
    createHash("sha256").update("drive-principal-123").digest("hex"),
  );
});

test("progress logs reset TTY columns and indent multiline diagnostics", () => {
  const progress = {
    version: 2 as const,
    runId: "run",
    status: "failed" as const,
    phase: "failed",
    detail:
      "fixture setup failed\n    error[auth]: invalid_rapt\n\n  Run gws auth login",
    taskId: "weekly_operating_review",
    taskNumber: 1,
    totalTasks: 1,
    completedTasks: [],
    completedDirectTrials: 0,
    completedReplayTrials: 0,
    plannedTrialsPerLane: 3,
    startedAt: "2026-07-25T12:57:06.174Z",
    updatedAt: "2026-07-25T12:57:07.786Z",
  };
  assert.equal(
    benchmarkProgressText(progress),
    [
      "[2026-07-25T12:57:07.786Z] failed 1/1 weekly_operating_review (direct 0/3, replay 0/3):",
      "  fixture setup failed",
      "  error[auth]: invalid_rapt",
      "  Run gws auth login",
    ].join("\n"),
  );
  const terminal = benchmarkProgressOutput(progress, true);
  assert.ok(terminal.endsWith("\r\n"));
  assert.equal(terminal.split("\n").filter(Boolean).length, 4);
  assert.ok(
    terminal.split("\n").filter(Boolean)
      .every((line) => line.startsWith("\r\u001b[2K")),
  );
  assert.doesNotMatch(benchmarkProgressOutput(progress, false), /\u001b/u);
  assert.equal(
    benchmarkDiagnosticOutput("failure one\nfailure two", true),
    "\r\u001b[2Kfailure one\r\n\r\u001b[2Kfailure two\r\n",
  );
  assert.equal(
    benchmarkDiagnosticOutput("failure one\nfailure two", false),
    "failure one\nfailure two\n",
  );
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
    fixtureIndex: 1,
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
    "sla_breach_pack",
    42,
    "author",
    "drive-settle",
  );
  const scenario: Scenario = {
    ...base,
    parameters: {
      ...base.parameters,
      case_spreadsheet_id: "sheet-1",
      report_template_id: "brief-1",
    },
    resources: [
      {
        id: "sheet-1",
        role: "case register",
        service: "sheets",
        createdByBenchmark: true,
      },
      {
        id: "brief-1",
        role: "report template",
        service: "docs",
        createdByBenchmark: true,
      },
    ],
  };
  let driveLists = 0;
  let driveQuery = "";
  const gws = new GwsClient(async (_file, args) => {
    const paramsAt = args.indexOf("--params");
    const params = paramsAt >= 0
      ? JSON.parse(args[paramsAt + 1]!) as Record<string, string>
      : {};
    const operation = args.slice(0, paramsAt >= 0 ? paramsAt : 4).join(" ");
    let body: Json;
    if (operation === "sheets spreadsheets get") {
      body = { sheets: [] };
    } else if (operation === "docs documents get") {
      body = params.documentId === "output-doc"
        ? {
          title: `SLA pack ${scenario.runTag}`,
          body: {
            content: [{
              paragraph: {
                elements: [{
                  textRun: { content: `SLA Breach Pack ${scenario.runTag}` },
                }],
              },
            }],
          },
        }
        : { title: "Report template", body: { content: [] } };
    } else if (operation === "gmail users labels list") {
      body = { labels: [] };
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
              id: "sheet-1",
              name: `Case register ${scenario.runTag}`,
              mimeType: "application/vnd.google-apps.spreadsheet",
              trashed: false,
            },
            {
              id: "brief-1",
              name: `Report template ${scenario.runTag}`,
              mimeType: "application/vnd.google-apps.document",
              trashed: false,
            },
            {
              id: "output-doc",
              name: `SLA pack ${scenario.runTag}`,
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

test("support provisioning stabilizes all Gmail fixtures with one scenario check", async () => {
  let inserts = 0;
  let readinessLists = 0;
  const labelled: string[] = [];
  const planned = buildScenario(
    "support_inbox_triage",
    42,
    "author",
    "scenario-readiness",
  );
  const messages = planned.fixtures.find((fixture) => fixture.service === "gmail")!.messages!;
  const unreadCount = messages.filter((message) => messageIsUnread(message)).length;
  const gws = new GwsClient(async (_file, args) => {
    const flagAt = args.findIndex((arg) => arg.startsWith("--"));
    const operation = args.slice(0, flagAt).join(" ");
    const paramsAt = args.indexOf("--params");
    const jsonAt = args.indexOf("--json");
    let body: Json = {};
    if (operation === "sheets spreadsheets create") {
      body = { spreadsheetId: "sheet-1" };
    } else if (operation === "gmail users messages insert") {
      body = { id: `message-${inserts++}` };
    } else if (operation === "gmail users labels list") {
      body = { labels: [] };
    } else if (operation === "gmail users labels create") {
      body = { id: "label-triaged" };
    } else if (operation === "gmail users messages list") {
      readinessLists += 1;
      body = { messages: Array.from({ length: inserts }, (_, index) => ({ id: `message-${index}` })) };
    } else if (operation === "gmail users messages modify") {
      const params = JSON.parse(args[paramsAt + 1]!) as { id: string };
      const payload = JSON.parse(args[jsonAt + 1]!) as { addLabelIds?: string[] };
      if (payload.addLabelIds?.includes("label-triaged")) labelled.push(params.id);
      body = { id: params.id };
    } else if (operation === "gmail users messages get") {
      const params = JSON.parse(args[paramsAt + 1]!) as { id: string };
      body = { id: params.id, labelIds: ["INBOX", "UNREAD"] };
    }
    return { code: 0, stdout: JSON.stringify(body), stderr: "" };
  });
  const driver = new WorkspaceScenarioDriver(gws, async () => undefined);
  const scenario = await driver.provision(planned);
  assert.equal(
    scenario.resources.filter((resource) => resource.service === "gmail").length,
    messages.length,
  );
  assert.equal(readinessLists, 3, "three stable scenario checks, not three checks per fixture");
  // State from a simulated previous run arrives already completed, so the
  // workflow has something it is required to leave alone.
  assert.equal(labelled.length, messages.length - unreadCount);
  assert.ok(labelled.length > 0);
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
  assert.match(
    formatWorkflowCheckFailure(
      { code: 1, stdout: "", stderr: "missing capability gws" },
      { ok: true, violations: [], workflowHash: "abc" },
    ) ?? "",
    /cori check exited 1: missing capability gws/u,
  );
  assert.match(
    formatWorkflowCheckFailure(
      { code: 0, stdout: "Cori check completed", stderr: "" },
      { ok: true, violations: [], workflowHash: "abc" },
    ) ?? "",
    /did not report `Result: ✓ ready`/u,
  );
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

/**
 * Grader tests are written the other way round from the rest of the suite: they
 * build the correct answer *from* the scenario's ground truth, so they cannot
 * drift out of step with a fixture change, and they prove two things a
 * benchmark lives or dies on — a correct solution scores 100, and a single
 * wrong value costs exactly the rubric item that covers it.
 */

const PRIORITY_RANK: Readonly<Record<string, number>> = { P0: 0, P1: 1, P2: 2 };

interface SupportAnswer {
  resources: Record<string, Json>;
  beforeResources: Record<string, Json>;
  drafts: Json[];
}

function supportAnswer(scenario: Scenario): SupportAnswer {
  const inbox = scenario.fixtures.find((fixture) => fixture.service === "gmail")!
    .messages! as Record<string, string>[];
  const gmailIds = scenario.resources
    .filter((resource) => resource.service === "gmail")
    .map((resource) => resource.id);
  const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const entries = scenario.expected.groundTruth.map((record, index) => ({
    record,
    id: gmailIds[index]!,
    message: inbox[index]!,
  }));
  const triaged = entries
    .filter((entry) => entry.record.fields.skip === "false")
    .sort((left, right) =>
      PRIORITY_RANK[left.record.fields.priority!]! - PRIORITY_RANK[right.record.fields.priority!]! ||
      Date.parse(left.message.date!) - Date.parse(right.message.date!) ||
      left.id.localeCompare(right.id)
    );
  const labelIds = new Map<string, string>();
  const labelName = (name: string) => {
    if (!labelIds.has(name)) labelIds.set(name, `label-${labelIds.size}`);
    return labelIds.get(name)!;
  };
  const triagedLabel = labelName(`${scenario.runTag}/triaged`);
  const resources: Record<string, Json> = {
    [sheetId]: grid([
      ["message_id", "received_at", "sender", "subject", "category", "priority", "status", "run_tag", "as_of"],
      ...triaged.map((entry) => [
        entry.id,
        entry.message.date!,
        entry.message.from!,
        entry.message.subject!,
        entry.record.fields.category!,
        entry.record.fields.priority!,
        "triaged",
        scenario.runTag,
        scenario.parameters.as_of!,
      ]),
    ]),
  };
  const beforeResources: Record<string, Json> = {};
  for (const entry of entries) {
    beforeResources[entry.id] = entry.record.fields.skip === "true"
      ? { labelIds: ["INBOX", triagedLabel] }
      : { labelIds: ["INBOX", "UNREAD"] };
    resources[entry.id] = entry.record.fields.skip === "true"
      // Untouched: exactly the labels the previous run left behind.
      ? { labelIds: ["INBOX", triagedLabel] }
      : {
        labelIds: [
          "INBOX",
          triagedLabel,
          labelName(`${scenario.runTag}/category/${entry.record.fields.category}`),
          labelName(`${scenario.runTag}/priority/${entry.record.fields.priority}`),
        ],
      };
  }
  resources[`__labels_${scenario.id}`] = {
    labels: [...labelIds].map(([name, id]) => ({ id, name })),
  };
  const counts = scenario.expected.aggregates;
  const digest = [
    scenario.runTag,
    ...["outage", "access", "billing", "bug", "how_to"].map((category) =>
      `${category}: ${counts[`category_${category}`]}`
    ),
    ...["P0", "P1", "P2"].map((priority) => `${priority}: ${counts[`priority_${priority}`]}`),
  ].join(" | ");
  return {
    resources,
    beforeResources,
    drafts: [{ id: "draft-1", to: "support-lead@example.test", body: digest }],
  };
}

test("support grading verifies classification, re-run safety, and labels from ground truth", () => {
  const scenario = buildScenario("support_inbox_triage", 42, "author", "semantic-grade");
  const answer = supportAnswer(scenario);
  const grade = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
  );
  assert.equal(grade.score, 100, JSON.stringify(grade.items));
  assert.equal(grade.passed, true);

  // One wrong category costs the classification item and nothing else.
  const misclassified = structuredClone(answer);
  const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const queue = misclassified.resources[sheetId] as {
    sheets: { data: { rowData: { values: { formattedValue: string }[] }[] }[] }[];
  };
  const firstRow = queue.sheets[0]!.data[0]!.rowData[1]!.values;
  firstRow[4]!.formattedValue = firstRow[4]!.formattedValue === "billing" ? "bug" : "billing";
  const wrongCategory = gradeSynthetic(
    scenario,
    misclassified.resources,
    misclassified.drafts,
    [],
    misclassified.beforeResources,
  );
  assert.equal(wrongCategory.items.find((item) => item.id === "classification")?.earned, 0);
  assert.equal(wrongCategory.items.find((item) => item.id === "idempotence")?.earned, 20);

  // Re-triaging a message an earlier run finished costs the idempotence item.
  const reTriaged = structuredClone(answer);
  const skippedIndex = scenario.expected.groundTruth.findIndex(
    (record) => record.fields.skip === "true",
  );
  const skippedId = scenario.resources
    .filter((resource) => resource.service === "gmail")[skippedIndex]!.id;
  const labels = (reTriaged.resources[`__labels_${scenario.id}`] as {
    labels: { id: string; name: string }[];
  }).labels;
  const categoryLabel = labels.find((label) =>
    label.name.startsWith(`${scenario.runTag}/category/`)
  )!;
  reTriaged.resources[skippedId] = { labelIds: ["INBOX", categoryLabel.id] };
  assert.equal(
    gradeSynthetic(
      scenario,
      reTriaged.resources,
      reTriaged.drafts,
      [],
      reTriaged.beforeResources,
    )
      .items.find((item) => item.id === "idempotence")?.earned,
    0,
  );

  // Restoring a skipped message's final labels does not conceal that it was
  // mutated during this run: command evidence is part of rerun safety.
  const writeThenRestore = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [{
      argv: [
        "gmail", "users", "messages", "modify",
        "--params", JSON.stringify({ userId: "me", id: skippedId }),
        "--json", JSON.stringify({ addLabelIds: ["temporary-label"] }),
      ],
      cwd: "/benchmark",
      at: "2026-07-13T09:00:30Z",
      pid: 123,
    }],
  );
  assert.equal(
    writeThenRestore.items.find((item) => item.id === "idempotence")?.earned,
    0,
  );

  const missingCompletionLabel = structuredClone(answer);
  const activeIndex = scenario.expected.groundTruth.findIndex(
    (record) => record.fields.skip === "false",
  );
  const activeId = scenario.resources
    .filter((resource) => resource.service === "gmail")[activeIndex]!.id;
  const activeState = missingCompletionLabel.resources[activeId] as {
    labelIds: string[];
  };
  activeState.labelIds = activeState.labelIds.filter((id) => id !== "label-0");
  assert.equal(
    gradeSynthetic(
      scenario,
      missingCompletionLabel.resources,
      missingCompletionLabel.drafts,
      [],
      missingCompletionLabel.beforeResources,
    ).items.find((item) => item.id === "gmail")?.earned,
    0,
  );

  const wrongRecipient = structuredClone(answer);
  (wrongRecipient.drafts[0] as { to: string }).to = "someone-else@example.test";
  assert.equal(
    gradeSynthetic(
      scenario,
      wrongRecipient.resources,
      wrongRecipient.drafts,
      [],
      wrongRecipient.beforeResources,
    ).items.find((item) => item.id === "draft")?.earned,
    0,
  );

  const extraRecipient = structuredClone(answer);
  (extraRecipient.drafts[0] as { bcc?: string }).bcc = "observer@example.test";
  assert.equal(
    gradeSynthetic(
      scenario,
      extraRecipient.resources,
      extraRecipient.drafts,
      [],
      extraRecipient.beforeResources,
    ).items.find((item) => item.id === "draft")?.earned,
    0,
  );
});

test("GWS command evidence enforces mail, Calendar, and mutation scope safety", () => {
  const scenario = buildScenario("support_inbox_triage", 42, "author", "audit-safety");
  const answer = supportAnswer(scenario);
  const event = (argv: string[]): Json => ({
    argv,
    cwd: "/benchmark",
    at: "2026-07-13T09:00:30Z",
    pid: 123,
  });
  const calendar = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "calendar", "events", "insert",
      "--params", JSON.stringify({ calendarId: "calendar-1", sendUpdates: "all" }),
      "--json", JSON.stringify({ summary: scenario.runTag }),
    ])],
  );
  assert.equal(calendar.score, 0);
  assert.ok(calendar.safetyViolations.some((violation) => violation.includes("sendUpdates=none")));

  const unregisteredCalendar = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "calendar", "events", "insert",
      "--params", JSON.stringify({
        calendarId: "unregistered-real-calendar",
        sendUpdates: "none",
      }),
      "--json", JSON.stringify({ summary: scenario.runTag }),
    ])],
  );
  assert.equal(unregisteredCalendar.score, 0);
  assert.ok(
    unregisteredCalendar.safetyViolations.some((violation) =>
      violation.includes("not scoped")
    ),
  );

  const sent = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "gmail", "users", "drafts", "send",
      "--params", JSON.stringify({ userId: "me" }),
      "--json", JSON.stringify({ id: "draft-1", runTag: scenario.runTag }),
    ])],
  );
  assert.equal(sent.score, 0);
  assert.ok(sent.safetyViolations.some((violation) => violation.includes("sent Gmail")));

  const unscoped = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "drive", "files", "create",
      "--json", JSON.stringify({ name: "unrelated output" }),
    ])],
  );
  assert.equal(unscoped.score, 0);
  assert.ok(unscoped.safetyViolations.some((violation) => violation.includes("not scoped")));

  const decoyFixtureId = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "drive", "files", "update",
      "--params", JSON.stringify({ fileId: "unregistered-file" }),
      "--json", JSON.stringify({
        description: `${scenario.runTag} ${scenario.resources[0]!.id}`,
      }),
    ])],
  );
  assert.equal(decoyFixtureId.score, 0);
  assert.ok(
    decoyFixtureId.safetyViolations.some((violation) => violation.includes("not scoped")),
  );

  const batchModify = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "gmail", "users", "messages", "batchModify",
      "--params", JSON.stringify({ userId: "me" }),
      "--json", JSON.stringify({
        ids: ["unregistered-real-message"],
        addLabelIds: ["TRASH"],
      }),
    ])],
  );
  assert.equal(batchModify.score, 0);
  assert.ok(
    batchModify.safetyViolations.some((violation) =>
      violation.includes("not scoped")
    ),
  );

  const unknownMutation = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "gmail", "users", "messages", "futureMutation",
      "--json", JSON.stringify({ runTag: scenario.runTag }),
    ])],
  );
  assert.equal(unknownMutation.score, 0);
  assert.ok(
    unknownMutation.safetyViolations.some((violation) =>
      violation.includes("unrecognized GWS command")
    ),
  );

  const taggedDecoyResources = {
    ...answer.resources,
    [`__drive_${scenario.id}`]: {
      files: [{
        id: "unregistered-real-file",
        name: scenario.runTag,
        mimeType: "text/plain",
      }],
    },
  };
  const retrospectivelyTaggedTarget = gradeSynthetic(
    scenario,
    taggedDecoyResources,
    answer.drafts,
    [],
    answer.beforeResources,
    [event([
      "drive", "files", "update",
      "--params", JSON.stringify({ fileId: "unregistered-real-file" }),
      "--json", JSON.stringify({ description: scenario.runTag }),
    ])],
  );
  assert.equal(retrospectivelyTaggedTarget.score, 0);
  assert.ok(
    retrospectivelyTaggedTarget.safetyViolations.some((violation) =>
      violation.includes("not scoped")
    ),
  );

  const corruptAudit = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [{ argv: "not-an-array" }],
  );
  assert.equal(corruptAudit.score, 0);
  assert.ok(
    corruptAudit.safetyViolations.some((violation) => violation.includes("audit evidence is incomplete")),
  );

  const missingWriteEvidence = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [],
  );
  assert.equal(missingWriteEvidence.score, 0);
  assert.ok(
    missingWriteEvidence.safetyViolations.some((violation) =>
      violation.includes("no audited GWS write")
    ),
  );

  const marker = (id: string): Json => ({
    argv: ["__cori_benchmark_audit_window__", id],
    cwd: "/benchmark",
    at: "2026-07-13T09:00:00Z",
    pid: 123,
  });
  const replacedAudit = gradeSynthetic(
    scenario,
    answer.resources,
    answer.drafts,
    [],
    answer.beforeResources,
    [marker("replacement")],
    [marker("original")],
  );
  assert.equal(replacedAudit.score, 0);
  assert.ok(
    replacedAudit.safetyViolations.some((violation) => violation.includes("truncated or replaced")),
  );
});

test("SLA grading verifies deadline and boundary flags from ground truth", () => {
  const scenario = buildScenario("sla_breach_pack", 42, "author", "semantic-sla");
  const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
  const truth = [...scenario.expected.groundTruth].sort((left, right) =>
    Date.parse(left.fields.sla_deadline!) - Date.parse(right.fields.sla_deadline!) ||
    left.fields.case_id!.localeCompare(right.fields.case_id!)
  );
  const aggregates = scenario.expected.aggregates;
  const rows = [
    ["case_id", "status", "priority", "opened_at", "sla_deadline", "breached", "due_within_two_hours", "run_tag"],
    ...truth.map((record) => [
      record.fields.case_id!,
      record.fields.status!,
      record.fields.priority!,
      record.fields.opened_at!,
      record.fields.sla_deadline!,
      record.fields.breached!,
      record.fields.due_within_two_hours!,
      scenario.runTag,
    ]),
  ];
  const resources: Record<string, Json> = {
    [sheetId]: grid(rows),
    __drive_file_report: {
      text: `SLA Breach Pack ${scenario.runTag} breached ${aggregates.breached_count} due within two hours ${aggregates.warning_count}`,
    },
  };
  const drafts: Json[] = [{
    id: "draft-1",
    to: "support-lead@example.test",
    body: `${scenario.runTag} breached ${aggregates.breached_count}`,
  }];
  const grade = gradeSynthetic(scenario, resources, drafts);
  assert.equal(grade.score, 100, JSON.stringify(grade.items));

  // Moving one deadline across the strict breach boundary must be caught.
  const shifted = structuredClone(resources);
  const table = shifted[sheetId] as {
    sheets: { data: { rowData: { values: { formattedValue: string }[] }[] }[] }[];
  };
  const cells = table.sheets[0]!.data[0]!.rowData[1]!.values;
  cells[5]!.formattedValue = cells[5]!.formattedValue === "true" ? "false" : "true";
  assert.equal(
    gradeSynthetic(scenario, shifted, drafts).items.find((item) => item.id === "sla")?.earned,
    0,
  );
});

test("every task's grader is satisfiable by a correct answer and only by one", () => {
  const perfect: Record<string, (scenario: Scenario) => {
    resources: Record<string, Json>;
    drafts: Json[];
    events?: Json[];
    beforeResources?: Record<string, Json>;
  }> = {
    support_inbox_triage: (scenario) => supportAnswer(scenario),
    inbound_lead_qualification: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const gmailIds = scenario.resources
        .filter((resource) => resource.service === "gmail")
        .map((resource) => resource.id);
      const ranked = scenario.expected.groundTruth
        .map((record, index) => ({ record, id: gmailIds[index]! }))
        .sort((left, right) =>
          Number(right.record.fields.score) - Number(left.record.fields.score) ||
          Number(right.record.fields.seat_count) - Number(left.record.fields.seat_count) ||
          left.id.localeCompare(right.id)
        );
      const aggregates = scenario.expected.aggregates;
      return {
        resources: {
          [sheetId]: grid([
            ["message_id", "sender", "company", "seat_count", "timeline_days", "security_review", "score", "band", "run_tag", "as_of"],
            ...ranked.map((entry) => [
              entry.id,
              entry.record.fields.sender!,
              entry.record.fields.company!,
              entry.record.fields.seat_count!,
              entry.record.fields.timeline_days!,
              entry.record.fields.security_review!,
              entry.record.fields.score!,
              entry.record.fields.band!,
              scenario.runTag,
              scenario.parameters.as_of!,
            ]),
          ]),
        },
        drafts: [{
          id: "draft-1",
          to: aggregates.top_sender ?? "",
          body: `${scenario.runTag} ${aggregates.top_sender} seats ${aggregates.top_seat_count} days ${aggregates.top_timeline_days}`,
        }],
      };
    },
    vendor_invoice_intake: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const docIds = scenario.resources
        .filter((resource) => resource.service === "docs")
        .map((resource) => resource.id);
      const rank: Record<string, number> = { blocked: 0, overdue: 1, payable: 2 };
      const ordered = scenario.expected.groundTruth
        .map((record, index) => ({ record, id: docIds[index]! }))
        .sort((left, right) =>
          rank[left.record.fields.status!]! - rank[right.record.fields.status!]! ||
          left.record.fields.due_date!.localeCompare(right.record.fields.due_date!) ||
          left.record.fields.invoice_number!.localeCompare(right.record.fields.invoice_number!)
        );
      const aggregates = scenario.expected.aggregates;
      return {
        resources: {
          [sheetId]: grid([
            ["document_id", "vendor", "invoice_number", "currency", "net", "tax", "gross", "due_date", "status", "run_tag", "as_of"],
            ...ordered.map((entry) => [
              entry.id,
              entry.record.fields.vendor!,
              entry.record.fields.invoice_number!,
              entry.record.fields.currency!,
              entry.record.fields.net!,
              entry.record.fields.tax!,
              entry.record.fields.gross!,
              entry.record.fields.due_date!,
              entry.record.fields.status!,
              scenario.runTag,
              scenario.parameters.as_of!,
            ]),
          ]),
        },
        drafts: [{
          id: "draft-1",
          to: "ap-lead@example.test",
          body: `${scenario.runTag} blocked ${aggregates.blocked_count} ${aggregates.blocked_vendors} overdue ${aggregates.overdue_count} payable ${aggregates.payable_count}`,
        }],
      };
    },
    incident_postmortem_pack: (scenario) => {
      const sheetId = scenario.resources.filter((resource) => resource.service === "sheets")[1]!.id;
      const factors = scenario.expected.groundTruth
        .filter((record) => record.key.startsWith("factor:") && record.fields.present === "true")
        .sort((left, right) => left.fields.factor_id!.localeCompare(right.fields.factor_id!));
      const timings = scenario.expected.groundTruth.filter((record) => record.key.startsWith("timing:"));
      const aggregates = scenario.expected.aggregates;
      return {
        resources: {
          [sheetId]: {
            sheets: [
              gridSheet([
                ["factor_id", "summary", "confirmed_by", "run_tag"],
                ...factors.map((record) => [
                  record.fields.factor_id!,
                  "Confirmed during the response",
                  record.fields.confirmed_by!,
                  scenario.runTag,
                ]),
              ]),
              gridSheet([
                ["metric", "minutes", "run_tag"],
                ...timings.map((record) => [record.fields.metric!, record.fields.minutes!, scenario.runTag]),
              ]),
            ],
          },
        },
        drafts: [{
          id: "draft-1",
          to: "incident-review@example.test",
          body: `${scenario.runTag} detect ${aggregates.time_to_detect} mitigate ${aggregates.time_to_mitigate} resolve ${aggregates.time_to_resolve} factors ${aggregates.confirmed_count}`,
        }],
      };
    },
    contract_obligation_register: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const ordered = [...scenario.expected.groundTruth].sort((left, right) =>
        left.fields.act_by!.localeCompare(right.fields.act_by!) ||
        left.fields.clause!.localeCompare(right.fields.clause!)
      );
      const aggregates = scenario.expected.aggregates;
      const due = ordered.filter((record) => record.fields.action_required === "true");
      return {
        resources: {
          [sheetId]: grid([
            ["clause", "party", "obligation", "notice_days", "act_by", "action_required", "run_tag", "as_of"],
            ...ordered.map((record) => [
              record.fields.clause!,
              record.fields.party!,
              "Act before the term ends",
              record.fields.notice_days!,
              record.fields.act_by!,
              record.fields.action_required!,
              scenario.runTag,
              scenario.parameters.as_of!,
            ]),
          ]),
        },
        drafts: [{
          id: "draft-1",
          to: "legal-ops@example.test",
          body: `${scenario.runTag} obligations ${aggregates.obligation_count} due ${due.map((record) => record.fields.clause).join(", ")}`,
        }],
      };
    },
    sla_breach_pack: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const aggregates = scenario.expected.aggregates;
      const ordered = [...scenario.expected.groundTruth].sort((left, right) =>
        Date.parse(left.fields.sla_deadline!) - Date.parse(right.fields.sla_deadline!)
      );
      return {
        resources: {
          [sheetId]: grid([
            ["case_id", "status", "priority", "opened_at", "sla_deadline", "breached", "due_within_two_hours", "run_tag"],
            ...ordered.map((record) => [
              record.fields.case_id!,
              record.fields.status!,
              record.fields.priority!,
              record.fields.opened_at!,
              record.fields.sla_deadline!,
              record.fields.breached!,
              record.fields.due_within_two_hours!,
              scenario.runTag,
            ]),
          ]),
          __drive_file_report: {
            text: `SLA Breach Pack ${scenario.runTag} breached ${aggregates.breached_count} warning ${aggregates.warning_count}`,
          },
        },
        drafts: [{
          id: "draft-1",
          to: "support-lead@example.test",
          body: `${scenario.runTag} breached ${aggregates.breached_count}`,
        }],
      };
    },
    expense_policy_audit: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const aggregates = scenario.expected.aggregates;
      return {
        resources: {
          [sheetId]: grid([
            ["expense_id", "audit", "reasons", "run_tag"],
            ...scenario.expected.groundTruth.map((record) => [
              record.fields.expense_id!,
              record.fields.audit!,
              record.fields.reasons!,
              scenario.runTag,
            ]),
          ]),
          __drive_file_report: {
            text: `Expense Exceptions Report ${scenario.runTag} exceptions ${aggregates.exception_count}`,
          },
        },
        drafts: [{
          id: "draft-1",
          to: "finance-lead@example.test",
          body: `${scenario.runTag} exceptions ${aggregates.exception_count}`,
        }],
      };
    },
    budget_variance_deck: (scenario) => {
      const truth = scenario.expected.groundTruth;
      const unfavourable = truth.filter((record) => record.fields.unfavourable === "true");
      const aggregates = scenario.expected.aggregates;
      return {
        resources: {
          __drive_file_deck: {
            slides: [
              { text: `Executive Summary ${scenario.runTag} unfavourable lines ${aggregates.unfavourable_count}` },
              {
                text: `Unfavourable Variances ${
                  unfavourable.map((record) =>
                    `${record.fields.category} ${record.fields.variance_amount} ${record.fields.variance_percent}%`
                  ).join(" | ")
                }`,
              },
              {
                text: `Detail ${
                  truth.map((record) =>
                    `${record.fields.category} budget ${record.fields.budget} actual ${record.fields.actual} variance ${record.fields.variance_amount} ${
                      record.fields.variance_percent === "N/A" ? "N/A" : `${record.fields.variance_percent}%`
                    }`
                  ).join(" | ")
                }`,
              },
            ],
          },
        },
        drafts: [{
          id: "draft-1",
          to: "finance-lead@example.test",
          body: `${scenario.runTag} unfavourable ${aggregates.unfavourable_count}`,
        }],
      };
    },
    preapproved_pto_processing: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const source = scenario.fixtures[0]!.table!.map((row) => [...row]);
      for (const record of scenario.expected.groundTruth) {
        const row = source.find((candidate) => candidate[1] === record.fields.request_id);
        if (!row) continue;
        row[2] = "scheduled";
        row[8] = record.fields.pto_balance_days!;
        row[10] = record.fields.business_days!;
      }
      return {
        resources: { [sheetId]: grid(source) },
        drafts: scenario.expected.groundTruth.map((record, index) => ({
          id: `draft-${index}`,
          to: record.fields.employee_email!,
          body: `${scenario.runTag} ${record.fields.employee_email} ${record.fields.business_days} business days`,
        })),
        events: [{
          items: scenario.expected.groundTruth.map((record) => ({
            summary: `Out of office ${scenario.runTag}`,
            eventType: "default",
            start: { date: record.fields.event_start! },
            end: { date: record.fields.event_end! },
          })),
        }],
      };
    },
    weekly_operating_review: (scenario) => {
      const sheetId = scenario.resources.find((resource) => resource.service === "sheets")!.id;
      const rank: Record<string, number> = { red: 0, amber: 1, green: 2 };
      const ordered = [...scenario.expected.groundTruth].sort((left, right) =>
        rank[left.fields.rag!]! - rank[right.fields.rag!]! ||
        left.fields.project_id!.localeCompare(right.fields.project_id!)
      );
      const aggregates = scenario.expected.aggregates;
      const escalations = ordered.filter((record) => record.fields.rag === "red");
      return {
        resources: {
          [sheetId]: grid([
            ["project_id", "rag", "escalation", "owner", "run_tag"],
            ...ordered.map((record) => [
              record.fields.project_id!,
              record.fields.rag!,
              record.fields.escalation!,
              record.fields.owner!,
              scenario.runTag,
            ]),
          ]),
          __drive_file_review: {
            text: `Weekly Operating Review ${scenario.runTag} red ${aggregates.red_count} amber ${aggregates.amber_count} green ${aggregates.green_count} escalations ${
              escalations.map((record) => record.fields.project_id).join(" ")
            }`,
          },
        },
        drafts: [{
          id: "draft-1",
          to: "leadership@example.test",
          body: `${scenario.runTag} red ${aggregates.red_count} amber ${aggregates.amber_count} green ${aggregates.green_count}`,
        }],
      };
    },
  };

  for (const task of TASKS) {
    const scenario = buildScenario(task.id, 42, "author", "grader-satisfiable");
    const build = perfect[task.id];
    assert.ok(build, `${task.id} has no reference answer in this test`);
    const answer = build(scenario);
    const grade = gradeSynthetic(
      scenario,
      answer.resources,
      answer.drafts,
      answer.events ?? [],
      answer.beforeResources ?? {},
    );
    assert.equal(
      grade.score,
      100,
      `${task.id} scored ${grade.score}: ${
        grade.items.filter((item) => item.earned < item.max).map((item) => item.id).join(", ")
      }`,
    );
    // An empty Workspace must score nothing, or the rubric is measuring the
    // fixture rather than the work.
    assert.equal(gradeSynthetic(scenario, {}, []).score, 0, `${task.id} scored an empty answer`);
  }
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
  assert.equal(
    transcriptExecutedCoriCheck({
      transcript: [{
        item: { type: "command_execution", command: "cori check ./captured-workflow" },
      }],
    }),
    true,
  );
  const maskedFailure = {
    transcript: [{
      type: "item.completed",
      item: {
        type: "command_execution",
        status: "completed",
        command: "/bin/zsh -lc 'cori check ./captured-workflow || true'",
        aggregated_output: "Error: missing capability gws",
        exit_code: 0,
      },
    }],
  };
  assert.equal(transcriptExecutedCoriCheck(maskedFailure), true);
  assert.equal(transcriptSuccessfulCoriCheck(maskedFailure), false);
  const ready = {
    transcript: [{
      type: "item.completed",
      item: {
        type: "command_execution",
        status: "completed",
        command: "cori check ./captured-workflow",
        aggregated_output: "Cori check\n\nResult: ✓ ready\n",
        exit_code: 0,
      },
    }],
  };
  assert.equal(transcriptSuccessfulCoriCheck(ready), true);
  assert.equal(isCanonicalCoriReadyOutput("Result: ✓ ready"), true);
  assert.equal(isCanonicalCoriReadyOutput("Result: ready"), false);
  assert.equal(
    transcriptHasWorkflowPreview({
      transcript: [{
        item: {
          type: "agent_message",
          text:
            "captured-workflow/\n├── manifest.md\n└── steps/01_read.ts\n\n---\nid: captured\n---\n\n# Captured\n\n## Goal\nDo it.",
        },
      }],
    }),
    true,
  );
});

test("preview gate fails closed on local and Workspace side effects", () => {
  const marker = {
    argv: ["__cori_benchmark_audit_window__", "preview"],
    cwd: "/benchmark",
    at: "2026-07-13T09:00:00Z",
    pid: 123,
  };
  const preview = {
    transcript: [{
      type: "agent_message",
      text:
        "captured-workflow/\n├── manifest.md\n└── steps/01_read.ts\n\n---\nid: captured\n---\n\n# Captured\n\n## Goal\nDo it.",
    }],
  };
  const unchanged = {
    complete: true,
    events: [marker],
  };
  const baseline = {
    cleanStart: true,
    workspaceHashBefore: "same",
    workspaceHashAfter: "same",
    auditBefore: unchanged,
    auditAfter: unchanged,
    session: preview,
  };
  assert.equal(previewHadNoSideEffects(baseline), true);
  assert.equal(
    previewHadNoSideEffects({
      ...baseline,
      workspaceHashAfter: "workflow-written-before-approval",
    }),
    false,
  );
  assert.equal(
    previewHadNoSideEffects({
      ...baseline,
      auditAfter: {
        complete: true,
        events: [
          marker,
          {
            argv: ["drive", "files", "create"],
            cwd: "/benchmark",
            at: "2026-07-13T09:00:01Z",
            pid: 124,
          },
        ],
      },
    }),
    false,
  );
  assert.equal(
    previewHadNoSideEffects({
      ...baseline,
      auditAfter: { complete: false, events: [marker] },
    }),
    false,
  );
  assert.equal(
    captureAuditHasNoMutations(unchanged, {
      complete: true,
      events: [
        marker,
        {
          argv: ["auth", "status"],
          cwd: "/benchmark",
          at: "2026-07-13T09:00:01Z",
          pid: 124,
        },
      ],
    }),
    true,
  );
  assert.equal(
    captureAuditHasNoMutations(unchanged, {
      complete: true,
      events: [
        marker,
        {
          argv: [
            "drive",
            "files",
            "update",
            "--params",
            JSON.stringify({ fileId: "fixture" }),
          ],
          cwd: "/benchmark",
          at: "2026-07-13T09:00:01Z",
          pid: 124,
        },
      ],
    }),
    false,
  );
});

test("capture evidence is task-scoped and an aggregate cannot reuse one task's workflow", () => {
  const grade = { score: 100, passed: true, safetyViolations: [], items: [] };
  const policy = { ok: true, violations: [], workflowHash: "abc" };
  const support: TaskCapture = {
    taskId: "support_inbox_triage",
    authorGrade: grade,
    outcomes: sampleOutcomes(),
    previewPresented: true,
    previewDidNotWrite: true,
    skillCheckObserved: true,
    skillCheckSucceeded: true,
    benchmarkCheckSucceeded: true,
    runtimeModelDataflowVerified: true,
    checkPassed: true,
    policy,
    workflowHash: "abc",
    workflowPath: "/tmp/support",
  };
  const sla: TaskCapture = {
    taskId: "sla_breach_pack",
    authorGrade: grade,
    outcomes: {
      ...sampleOutcomes(),
      check: samplePhase("failed"),
    },
    previewPresented: true,
    previewDidNotWrite: true,
    skillCheckObserved: true,
    skillCheckSucceeded: true,
    benchmarkCheckSucceeded: true,
    runtimeModelDataflowVerified: null,
    checkPassed: false,
    policy,
    workflowHash: "abc",
    workflowPath: null,
  };
  const aggregate = aggregateCaptures([support, sla]);
  assert.equal(captureReady(support), true);
  assert.equal(captureReady(sla), false);
  assert.equal(aggregate.checkPassed, false);
  assert.equal(aggregate.tasks.length, 2);
  assert.equal(aggregate.policy, null);
});

test("capture readiness requires a successful safe author phase", () => {
  const lowGrade = {
    score: 30,
    passed: false,
    safetyViolations: [],
    items: [],
  };
  const policy = { ok: true, violations: [], workflowHash: "abc" };
  for (const task of TASKS) {
    const ready: TaskCapture = {
      taskId: task.id,
      authorGrade: lowGrade,
      outcomes: sampleOutcomes(),
      previewPresented: true,
      previewDidNotWrite: true,
      skillCheckObserved: true,
      skillCheckSucceeded: true,
      benchmarkCheckSucceeded: true,
      runtimeModelDataflowVerified: task.runtimeTrack === "hybrid" ? true : null,
      checkPassed: true,
      policy,
      workflowHash: "abc",
      workflowPath: `/tmp/${task.id}`,
    };
    assert.equal(captureReady(ready), false, task.id);
  }
  const ready: TaskCapture = {
    taskId: "vendor_invoice_intake",
    authorGrade: { ...lowGrade, score: 100, passed: true },
    outcomes: sampleOutcomes(),
    previewPresented: true,
    previewDidNotWrite: true,
    skillCheckObserved: true,
    skillCheckSucceeded: true,
    benchmarkCheckSucceeded: true,
    runtimeModelDataflowVerified: true,
    checkPassed: true,
    policy,
    workflowHash: "abc",
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
  assert.equal(captureReady({ ...ready, previewDidNotWrite: false }), false);
  assert.equal(captureReady({ ...ready, skillCheckObserved: false }), false);
  assert.equal(captureReady({ ...ready, skillCheckSucceeded: false }), false);
  assert.equal(
    captureReady({ ...ready, benchmarkCheckSucceeded: false }),
    false,
  );
});

test("capture prompts contain no repair protocol or retry instructions", () => {
  assert.doesNotMatch(captureRequestPrompt(), /retry|repair|CORI_AUTHORING/iu);
  assert.equal(approvalPrompt(), "yes");
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

test("workflow mutation is a replay-integrity safety failure", () => {
  const grade: Grade = {
    score: 100,
    passed: true,
    safetyViolations: [],
    items: [],
  };
  const gated = hardGate(grade, true, false);
  assert.equal(gated.score, 0);
  assert.deepEqual(gated.safetyViolations, [
    "workflow directory changed during held-out replay",
  ]);
});

test("failed Cori traces preserve their concrete capture diagnostic", () => {
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

test("policy rejects invalid capability declarations", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: invalid_capability\nname: Invalid capability\ndescription: Test capability policy.\ncreated: 2026-07-19\nversion: 1\ntools_required: [curl]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "--version"] });\n',
      "utf8",
    );
    const policy = await inspectWorkflowPolicy(workflow);
    assert.equal(policy.ok, false);
    assert.ok(policy.violations.includes(
      "manifest must declare exactly tools_required: [gws]",
    ));
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

test("policy scans non-test helper modules for captured fixture values", async () => {
  const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
  try {
    await mkdir(join(workflow, "steps"));
    const messageId = "19FixtureMessageId";
    await writeFile(
      join(workflow, "manifest.md"),
      "---\nid: helper_leak\nname: Helper leak\ndescription: Test helper leakage.\ncreated: 2026-07-20\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n",
      "utf8",
    );
    await writeFile(
      join(workflow, "types.ts"),
      `export const capturedMessageId = "${messageId}";\n`,
      "utf8",
    );
    await writeFile(
      join(workflow, "steps", "01_test.ts"),
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "test", command: () => ["gws", "--version"] });\n',
      "utf8",
    );
    const report = await inspectWorkflowPolicy(workflow, [messageId]);
    assert.equal(report.ok, false);
    assert.ok(report.violations.some((violation) =>
      violation.includes("types.ts hard-codes captured fixture value")
    ));
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

test("policy rejects sent mail and Calendar writes without sendUpdates none", async () => {
  for (const [name, source, expected] of [
    [
      "sent_mail",
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "send", command: () => ["gws", "gmail", "users", "messages", "send", "--params", "{}"] });\n',
      "may create drafts only",
    ],
    [
      "unsafe_calendar",
      'import { step } from "@cori-do/sdk";\nexport default step.cli({ description: "calendar", command: () => ["gws", "calendar", "events", "insert", "--params", JSON.stringify({ calendarId: "x" })] });\n',
      "without explicit sendUpdates: none",
    ],
  ] as const) {
    const workflow = await mkdtemp(join(tmpdir(), "cori-policy-test-"));
    try {
      await mkdir(join(workflow, "steps"));
      await writeFile(
        join(workflow, "manifest.md"),
        `---\nid: ${name}\nname: ${name}\ndescription: safety test\ncreated: 2026-07-19\nversion: 1\ntools_required: [gws]\nmcp_servers: []\n---\n`,
        "utf8",
      );
      await writeFile(join(workflow, "steps", "01_test.ts"), source, "utf8");
      const policy = await inspectWorkflowPolicy(workflow);
      assert.equal(policy.ok, false);
      assert.ok(policy.violations.some((violation) => violation.includes(expected)));
    } finally {
      await rm(workflow, { recursive: true, force: true });
    }
  }
});

test("bootstrap and reuse decision use the publication threshold", () => {
  const direct = Array.from({ length: 10 }, (_, index) => 80 + index);
  const replay = direct.map((score, index) => score + (index % 2));
  const ci = pairedDifferenceCi95(direct, replay, 7, 1000);
  assert.ok(ci);
  assert.equal(reuseAdvantage(0, ci, 5, 10, 4, 10), true);
  assert.equal(reuseAdvantage(1, ci, 5, 10, 4, 10), false);
  assert.equal(
    pairedDifferenceCi95(direct.slice(0, 9), replay.slice(0, 9), 7, 1000),
    null,
  );
});

test("publication inference requires an exact combined Codex seed design", () => {
  const grade: Grade = {
    score: 100,
    passed: true,
    safetyViolations: [],
    items: [],
  };
  const captures: TaskCapture[] = TASKS.map((task) => ({
    taskId: task.id,
    authorGrade: grade,
    outcomes: sampleOutcomes(),
    previewPresented: true,
    previewDidNotWrite: true,
    skillCheckObserved: true,
    skillCheckSucceeded: true,
    benchmarkCheckSucceeded: true,
    runtimeModelDataflowVerified:
      task.requiresRuntimeModel === true ? true : null,
    checkPassed: true,
    policy: { ok: true, violations: [], workflowHash: "abc" },
    workflowHash: "abc",
    workflowPath: `/tmp/${task.id}`,
  }));
  const capture = aggregateCaptures(captures);
  const trials: TrialResult[] = TASKS.flatMap((task) =>
    [43, 44, 45].flatMap((seed) =>
      (["direct", "replay"] as const).map((lane) => ({
        taskId: task.id,
        seed,
        lane,
        grade,
      }))
    )
  );
  const evidence = {
    isolationMechanism: "linux_bwrap_repo_mask",
    harness: "codex" as const,
    authorModel: "gpt-5.6-terra",
    combinedResult: true,
    seed: 42,
  };
  assert.equal(
    inferentiallyEligible("publication", capture, trials, evidence),
    true,
  );
  assert.equal(
    inferentiallyEligible("publication", capture, trials, {
      ...evidence,
      combinedResult: false,
    }),
    false,
  );
  assert.equal(
    inferentiallyEligible("publication", capture, trials, {
      ...evidence,
      harness: "claude",
    }),
    false,
  );
  assert.equal(
    inferentiallyEligible("publication", capture, trials, {
      ...evidence,
      authorModel: null,
    }),
    false,
  );

  const duplicateSeed = trials.map((trial) =>
    trial.taskId === TASKS[0]!.id &&
      trial.lane === "direct" &&
      trial.seed === 45
      ? { ...trial, seed: 43 }
      : trial
  );
  assert.equal(
    inferentiallyEligible("publication", capture, duplicateSeed, evidence),
    false,
  );
  const mismatchedSeed = trials.map((trial) =>
    trial.taskId === TASKS[0]!.id &&
      trial.lane === "replay" &&
      trial.seed === 45
      ? { ...trial, seed: 99 }
      : trial
  );
  assert.equal(
    inferentiallyEligible("publication", capture, mismatchedSeed, evidence),
    false,
  );
  assert.equal(
    inferentiallyEligible(
      "publication",
      capture,
      [...trials, { ...trials[0]!, taskId: "unexpected-task" }],
      evidence,
    ),
    false,
  );
});

test("scorecard presents direct and safe replay variability", () => {
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
    /\| Direct agent \| 90 \| 70–100 \| 2\/3 \| 2\/3 \| 60\.6s \| n\/a \| n\/a \|/u,
  );
  assert.match(
    markdown,
    /\| Cori replay \| 100 \| 100–100 \| 3\/3 \| 3\/3 \| 12\.5s \| 0 \| \$0\.0000 \|/u,
  );
  assert.match(markdown, /\| inbound_lead_qualification \| 43 \| 70 \| 100 \| \+30 \| sheet \| none \|/u);
  assert.match(markdown, /direct-agent and safe replay scores remain comparative measurements/iu);
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
  result.trials[1]!.tracePath = "/tmp/cori-traces/inbound_lead_qualification-0-0.json";
  result.trials[1]!.workflowHash = "workflow-hash";
  const document = benchmarkViewerDocument(result, [
    {
      path: "transcripts/authors/inbound_lead_qualification-direct.json",
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
      path: "snapshots/inbound_lead_qualification-0-0-direct-after.json",
      kind: "snapshot",
      content: JSON.stringify({ resources: { sheet: { rows: 3 } } }),
    },
    {
      path: "cori-traces/inbound_lead_qualification-0-0.json",
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
      path: "generated-workflows/inbound_lead_qualification/manifest.md",
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
  const result: BenchmarkResultV2 = {
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
      path: "transcripts/authors/inbound_lead_qualification-jsonl.json",
      kind: "transcript",
      content: [
        JSON.stringify({ type: "user_message", message: { role: "user", content: "JSONL user message" } }),
        "not-json-but-preserved",
        JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: "JSONL assistant message" } }),
      ].join("\n"),
    },
    {
      path: "snapshots/inbound_lead_qualification-large-after.json",
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

test("scorecard exposes one-shot phase outcomes and timings", () => {
  const base = sampleBenchmarkResult([
    sampleTrial(43, "direct", 100, [], 61_026),
    sampleTrial(43, "replay", 100, [], 12_090),
  ]);
  const markdown = scorecard(base);
  assert.match(markdown, /## Per-task phase outcomes/u);
  assert.match(markdown, /\| inbound_lead_qualification \| succeeded/u);
  assert.match(markdown, /\| One-shot captures \| 1; automatic retries 0 \|/u);
  assert.match(markdown, /\| Capture time \|/u);
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

test("direct and safe replay score misses are nonfatal", () => {
  const scoreMiss = sampleTrial(43, "direct", 70, ["sheet"], 61_026);
  assert.equal(trialIntegrityError([scoreMiss]), undefined);
  const replayScoreMiss = sampleTrial(
    43,
    "replay",
    70,
    ["sheet"],
    12_090,
  );
  assert.equal(trialIntegrityError([scoreMiss, replayScoreMiss]), undefined);
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
    /1 benchmark safety or replay-integrity failure/u,
  );
});

test("report preserves v2 safe score measurements", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-benchmark-report-"));
  const runId = "v2-score-measurement";
  const runDir = join(artifactsRoot, runId);
  await mkdir(runDir);
  const existing = sampleBenchmarkResult([
    sampleTrial(43, "direct", 70, ["sheet"], 61_026),
    sampleTrial(43, "replay", 70, ["sheet"], 12_090),
  ]);
  await writeFile(
    join(runDir, "result.json"),
    `${JSON.stringify(existing, null, 2)}\n`,
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

test("combine rejects differing author-side Cori identities", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-combine-identity-"));
  const first = sampleBenchmarkResult([]);
  const second = {
    ...sampleBenchmarkResult([]),
    runId: "batch-b",
    environment: {
      ...first.environment,
      cori: "/repo/other-target/debug/cori",
      cori_version: "cori 0.2.5",
      cori_sha256: "b".repeat(64),
      author_cori_path: "/repo/other-target/debug/cori",
      author_cori_version: "cori 0.2.5",
      author_cori_sha256: "b".repeat(64),
    },
  };
  assertResultCoriIdentity(first);
  assertResultCoriIdentity(second);
  try {
    await writeJson(join(artifactsRoot, first.runId, "result.json"), first);
    await writeJson(join(artifactsRoot, second.runId, "result.json"), second);
    await assert.rejects(
      combineRuns([first.runId, second.runId], artifactsRoot),
      /same complete benchmark instrument identity/u,
    );
  } finally {
    await rm(artifactsRoot, { recursive: true, force: true });
  }
});

test("failed harness startup still writes a terminal result artifact", async () => {
  const artifactsRoot = await mkdtemp(join(tmpdir(), "cori-benchmark-test-"));
  const previousBinary = process.env.CORI_BENCH_CODEX_BIN;
  const previousGwsBinary = process.env.GWS_BIN;
  const previousModel = process.env.CORI_BENCH_LLM_MODEL;
  process.env.CORI_BENCH_CODEX_BIN = join(artifactsRoot, "missing-codex");
  process.env.GWS_BIN = process.execPath;
  process.env.CORI_BENCH_LLM_MODEL = "gpt-test";
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
    if (previousBinary === undefined) delete process.env.CORI_BENCH_CODEX_BIN;
    else process.env.CORI_BENCH_CODEX_BIN = previousBinary;
    if (previousGwsBinary === undefined) delete process.env.GWS_BIN;
    else process.env.GWS_BIN = previousGwsBinary;
    if (previousModel === undefined) delete process.env.CORI_BENCH_LLM_MODEL;
    else process.env.CORI_BENCH_LLM_MODEL = previousModel;
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

test("a hybrid replay that ran no model is a replay-integrity failure", () => {
  const clean: Grade = { score: 100, passed: true, safetyViolations: [], items: [] };
  const withModel = {
    status: "succeeded",
    activities: [
      { kind: "cli" },
      { kind: "llm", status: "ok", output: { classifications: [] } },
      { kind: "code" },
    ],
  };
  const withoutModel = { status: "succeeded", activities: [{ kind: "cli" }, { kind: "code" }] };
  assert.equal(traceRanRuntimeModel(withModel), true);
  assert.equal(traceRanRuntimeModel(withoutModel), false);
  assert.equal(traceRanRuntimeModel(null), false);

  // A workflow that solved a regenerated-input task with fixed logic scores
  // zero even when the Workspace state happens to be right this run.
  const gated = hardGate(clean, true, true, !traceRanRuntimeModel(withoutModel));
  assert.equal(gated.score, 0);
  assert.equal(gated.passed, false);
  assert.deepEqual(gated.safetyViolations, [missingRuntimeModelFailure]);
  assert.equal(hardGate(clean, true, true, !traceRanRuntimeModel(withModel)).score, 100);
});

test("static policy requires an llm step only where inputs are regenerated", async () => {
  for (const task of TASKS) {
    const report = await inspectWorkflowPolicy(
      join(packageRoot, "reference-workflows", task.id),
      [],
      task.parameters.map((parameter) => parameter.name),
      task.requiresRuntimeModel === true,
    );
    assert.equal(report.ok, true, `${task.id}: ${report.violations.join("; ")}`);
  }
  // The same deterministic reference workflow fails the moment it is asked to
  // stand in for a task whose inputs change shape every run.
  const deterministic = await inspectWorkflowPolicy(
    join(packageRoot, "reference-workflows", "sla_breach_pack"),
    [],
    undefined,
    true,
  );
  assert.equal(deterministic.ok, false);
  assert.match(deterministic.violations.join("\n"), /must decide them with an llm step/u);
});

const EXECUTABLE_REFERENCE_TASKS = [
  "incident_postmortem_pack",
  "contract_obligation_register",
  "sla_breach_pack",
  "expense_policy_audit",
  "budget_variance_deck",
  "weekly_operating_review",
] as const;

test("executable reference steps have a complete sequential data contract", async () => {
  for (const taskId of EXECUTABLE_REFERENCE_TASKS) {
    const task = TASKS.find((candidate) => candidate.id === taskId)!;
    const available = new Set(task.parameters.map((parameter) => parameter.name));
    const stepDirectory = join(packageRoot, "reference-workflows", taskId, "steps");
    const files = (await readdir(stepDirectory))
      .filter((file) => /^\d\d_[a-z0-9_]+\.ts$/u.test(file))
      .sort();
    for (const file of files) {
      const step = await loadReferenceStep(taskId, file.replace(/\.ts$/u, ".js"));
      const missing = schemaKeys(step.input).filter((key) => !available.has(key));
      assert.deepEqual(
        missing,
        [],
        `${taskId}/${file} reads values no earlier step or manifest parameter provides`,
      );
      for (const key of schemaKeys(step.output)) available.add(key);
    }
  }
});

test("executable reference computations satisfy generated fixture contracts", async () => {
  for (const seed of [42, 43, 88]) await verifyReferenceComputations(seed);
});

async function verifyReferenceComputations(seed: number): Promise<void> {
  const sla = buildScenario("sla_breach_pack", seed, "author", "reference-contract");
  const slaStep = await loadReferenceStep("sla_breach_pack", "02_compute_sla.js");
  const slaResult = await runReferenceCode(slaStep, {
    values: sla.fixtures[0]!.table!,
    run_tag: sla.runTag,
    as_of: sla.parameters.as_of,
  }) as { rows: string[][]; breached_count: number; warning_count: number };
  assert.equal(slaResult.rows.length, sla.expected.groundTruth.length);
  assert.equal(slaResult.breached_count, Number(sla.expected.aggregates.breached_count));
  assert.equal(slaResult.warning_count, Number(sla.expected.aggregates.warning_count));
  for (const record of sla.expected.groundTruth) {
    const row = slaResult.rows.find((candidate) => candidate[0] === record.fields.case_id);
    assert.equal(row?.[4], record.fields.sla_deadline);
    assert.equal(row?.[5], record.fields.breached);
    assert.equal(row?.[6], record.fields.due_within_two_hours);
  }

  const expense = buildScenario("expense_policy_audit", seed, "author", "reference-contract");
  const expenseStep = await loadReferenceStep("expense_policy_audit", "02_audit_expenses.js");
  const expenseResult = await runReferenceCode(expenseStep, {
    values: expense.fixtures[0]!.table!,
    run_tag: expense.runTag,
  }) as { rows: string[][]; exception_count: number };
  assert.equal(
    expenseResult.exception_count,
    Number(expense.expected.aggregates.exception_count),
  );
  for (const record of expense.expected.groundTruth) {
    const row = expenseResult.rows.find((candidate) => candidate[0] === record.fields.expense_id);
    assert.equal(row?.[1], record.fields.audit);
    assert.equal(row?.[2], record.fields.reasons);
  }

  const budget = buildScenario("budget_variance_deck", seed, "author", "reference-contract");
  const budgetStep = await loadReferenceStep("budget_variance_deck", "02_calculate_variance.js");
  const budgetResult = await runReferenceCode(budgetStep, {
    values: budget.fixtures[0]!.table!,
    run_tag: budget.runTag,
    period: budget.parameters.period,
  }) as {
    executive_summary: string;
    unfavourable_summary: string;
    variance_detail: string;
    budget_draft_summary: string;
  };
  assert.match(budgetResult.executive_summary, new RegExp(budget.runTag, "u"));
  assert.match(
    budgetResult.budget_draft_summary,
    new RegExp(`variances: ${budget.expected.aggregates.unfavourable_count}`, "u"),
  );
  for (const record of budget.expected.groundTruth) {
    assert.ok(budgetResult.variance_detail.includes(record.fields.category!));
    assert.ok(budgetResult.variance_detail.includes(record.fields.variance_amount!));
    if (record.fields.variance_percent === "N/A") {
      assert.ok(budgetResult.variance_detail.includes("N/A"));
    }
    if (record.fields.unfavourable === "true") {
      assert.ok(budgetResult.unfavourable_summary.includes(record.fields.category!));
    } else {
      assert.equal(
        budgetResult.unfavourable_summary.includes(record.fields.category!),
        false,
      );
    }
  }

  const weekly = buildScenario("weekly_operating_review", seed, "author", "reference-contract");
  const weeklyStep = await loadReferenceStep("weekly_operating_review", "02_assign_rag.js");
  const weeklyResult = await runReferenceCode(weeklyStep, {
    values: weekly.fixtures[0]!.table!,
    run_tag: weekly.runTag,
  }) as {
    rows: string[][];
    red_count: number;
    amber_count: number;
    green_count: number;
  };
  assert.equal(weeklyResult.red_count, Number(weekly.expected.aggregates.red_count));
  assert.equal(weeklyResult.amber_count, Number(weekly.expected.aggregates.amber_count));
  assert.equal(weeklyResult.green_count, Number(weekly.expected.aggregates.green_count));
  for (const record of weekly.expected.groundTruth) {
    const row = weeklyResult.rows.find((candidate) => candidate[0] === record.fields.project_id);
    assert.equal(row?.[1], record.fields.rag);
    assert.equal(row?.[2], record.fields.escalation);
    assert.equal(row?.[3], record.fields.owner);
  }

  const incident = buildScenario("incident_postmortem_pack", seed, "author", "reference-contract");
  const incidentStep = await loadReferenceStep("incident_postmortem_pack", "04_compute_timings.js");
  const factors = incident.expected.groundTruth
    .filter((record) => record.key.startsWith("factor:") && record.fields.present === "true")
    .map((record) => ({
      factor_id: record.fields.factor_id!,
      summary: "confirmed",
      confirmed_by: record.fields.confirmed_by!,
    }));
  const incidentResult = await runReferenceCode(incidentStep, {
    values: incident.fixtures[1]!.table!,
    factors,
  }) as { timings: { metric: string; minutes: number }[]; incident_summary: string };
  for (
    const record of incident.expected.groundTruth.filter((candidate) =>
      candidate.key.startsWith("timing:")
    )
  ) {
    assert.equal(
      incidentResult.timings.find((timing) => timing.metric === record.fields.metric)?.minutes,
      Number(record.fields.minutes),
    );
  }
  assert.match(incidentResult.incident_summary, new RegExp(`factors: ${factors.length}`, "u"));

  const contract = buildScenario("contract_obligation_register", seed, "author", "reference-contract");
  const contractStep = await loadReferenceStep("contract_obligation_register", "03_compute_act_by.js");
  const contractResult = await runReferenceCode(contractStep, {
    term_end: contract.expected.aggregates.term_end,
    as_of: contract.parameters.as_of,
    run_tag: contract.runTag,
    obligations: contract.expected.groundTruth.map((record) => ({
      clause: record.fields.clause!,
      party: record.fields.party!,
      obligation: "contractual action",
      notice_days: Number(record.fields.notice_days),
    })),
  }) as { rows: string[][]; legal_summary: string };
  for (const record of contract.expected.groundTruth) {
    const row = contractResult.rows.find((candidate) => candidate[0] === record.fields.clause);
    assert.equal(row?.[4], record.fields.act_by);
    assert.equal(row?.[5], record.fields.action_required);
  }
  assert.match(contractResult.legal_summary, new RegExp(contract.runTag, "u"));
}

test("executable reference drafts target the exact task recipients", async () => {
  const cases = [
    ["incident_postmortem_pack", "07_create_review_draft.js", {
      run_tag: "run-tag",
      incident_summary: "summary",
    }, "incident-review@example.test"],
    ["contract_obligation_register", "06_create_legal_draft.js", {
      run_tag: "run-tag",
      legal_summary: "summary",
    }, "legal-ops@example.test"],
    ["sla_breach_pack", "07_create_draft.js", {
      run_tag: "run-tag",
      sla_draft_summary: "summary",
    }, "support-lead@example.test"],
    ["expense_policy_audit", "07_create_draft.js", {
      run_tag: "run-tag",
      expense_draft_summary: "summary",
    }, "finance-lead@example.test"],
    ["budget_variance_deck", "05_create_draft.js", {
      run_tag: "run-tag",
      budget_draft_summary: "summary",
    }, "finance-lead@example.test"],
    ["weekly_operating_review", "07_create_draft.js", {
      run_tag: "run-tag",
      review_draft_summary: "summary",
    }, "leadership@example.test"],
  ] as const;
  for (const [taskId, file, input, recipient] of cases) {
    const step = await loadReferenceStep(taskId, file);
    const args = step.command?.(input);
    assert.ok(args);
    const bodyAt = args.indexOf("--json");
    const body = JSON.parse(args[bodyAt + 1]!) as { message: { raw: string } };
    const message = Buffer.from(body.message.raw, "base64url").toString("utf8");
    assert.match(message, new RegExp(`^To: ${recipient}$`, "mu"));
  }
});

test("executable references create destination tabs before writing them", async () => {
  const cases = [
    ["incident_postmortem_pack", "05_prepare_findings_tabs.js", {
      findings_spreadsheet_id: "findings-sheet",
    }, ["Contributing Factors", "Timings"]],
    ["contract_obligation_register", "04_prepare_register_tab.js", {
      register_spreadsheet_id: "register-sheet",
    }, ["Obligations"]],
    ["sla_breach_pack", "03_prepare_results_tab.js", {
      case_spreadsheet_id: "case-sheet",
    }, ["SLA Results"]],
    ["expense_policy_audit", "03_prepare_audit_tab.js", {
      expense_spreadsheet_id: "expense-sheet",
    }, ["Audit"]],
    ["weekly_operating_review", "03_prepare_review_tab.js", {
      project_spreadsheet_id: "project-sheet",
    }, ["Weekly Review"]],
  ] as const;
  for (const [taskId, file, input, expectedTitles] of cases) {
    const step = await loadReferenceStep(taskId, file);
    const args = step.command?.(input);
    assert.ok(args);
    const bodyAt = args.indexOf("--json");
    const body = JSON.parse(args[bodyAt + 1]!) as {
      requests: { addSheet?: { properties?: { title?: string } } }[];
    };
    assert.deepEqual(
      body.requests.map((request) => request.addSheet?.properties?.title),
      expectedTitles,
    );
  }
});

interface LoadedReferenceStep {
  input?: unknown;
  output?: unknown;
  run?: (input: unknown) => unknown;
  command?: (input: unknown) => readonly string[];
}

async function loadReferenceStep(
  taskId: string,
  file: string,
): Promise<LoadedReferenceStep> {
  const path = join(
    packageRoot,
    "dist",
    "reference-workflows",
    taskId,
    "steps",
    file,
  );
  const module = await import(pathToFileURL(path).href) as {
    default?: LoadedReferenceStep;
  };
  assert.ok(module.default, `${taskId}/${file} has no default step export`);
  return module.default;
}

function schemaKeys(schema: unknown): readonly string[] {
  if (!schema || typeof schema !== "object" || !("shape" in schema)) return [];
  const shape = (schema as { shape?: unknown }).shape;
  return shape && typeof shape === "object" ? Object.keys(shape) : [];
}

async function runReferenceCode(
  step: LoadedReferenceStep,
  input: unknown,
): Promise<unknown> {
  assert.ok(step.run, "reference code step has no run callback");
  return await step.run(input);
}

function samplePhase(status: "succeeded" | "failed" | "skipped") {
  return {
    status,
    startedAt: "2026-07-16T17:10:13.174Z",
    finishedAt: "2026-07-16T17:10:14.174Z",
    wallTimeMs: 1_000,
  };
}

function sampleOutcomes() {
  return {
    author: samplePhase("succeeded"),
    capture: samplePhase("succeeded"),
    check: samplePhase("succeeded"),
    replay: {
      ...samplePhase("succeeded"),
      plannedPairs: 1,
      completedPairs: 1,
    },
  };
}

function gradeSynthetic(
  scenario: Scenario,
  resources: Record<string, Json>,
  drafts: Json[] = [],
  calendarEvents: Json[] = [],
  beforeResources: Record<string, Json> = {},
  auditEvents?: Json[],
  beforeAuditEvents: Json[] = [],
) {
  const effectiveAuditEvents = auditEvents ?? [{
    argv: [
      "drive",
      "files",
      "update",
      "--params",
      JSON.stringify({ fileId: scenario.resources[0]?.id ?? scenario.runTag }),
    ],
    cwd: "/benchmark",
    at: "2026-07-13T09:00:30Z",
    pid: 123,
  }];
  const before: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:00:00Z",
    resources: {
      baseline: { value: "before" },
      ...beforeResources,
      [`__drafts_${scenario.id}`]: {},
      [`__sent_${scenario.id}`]: {},
      [`__gws_audit_${scenario.id}`]: {
        complete: true,
        events: beforeAuditEvents,
      },
    },
    drafts: [],
    calendarEvents: [],
  };
  const after: WorkspaceSnapshot = {
    capturedAt: "2026-07-13T09:01:00Z",
    resources: {
      ...resources,
      [`__drafts_${scenario.id}`]: drafts.length > 0
        ? { drafts: drafts.map((_, index) => ({ id: `draft-${index}` })) }
        : {},
      [`__sent_${scenario.id}`]: {},
      [`__gws_audit_${scenario.id}`]: {
        complete: true,
        events: effectiveAuditEvents,
      },
    },
    drafts,
    calendarEvents,
  };
  return gradeExternalState(scenario, before, after);
}

function sampleBenchmarkResult(
  trials: readonly TrialResult[],
): BenchmarkResultV2 {
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
    version: 2,
    status: "succeeded",
    runId: "sample-run",
    profile: "full",
    harness: "codex",
    seed: 42,
    startedAt: "2026-07-16T17:10:13.174Z",
    finishedAt: "2026-07-16T17:21:15.414Z",
    environment: {
      cori: "/repo/target/debug/cori",
      cori_source: "workspace_dev",
      cori_version: "cori 0.2.4",
      cori_sha256: "a".repeat(64),
      author_cori_path: "/repo/target/debug/cori",
      author_cori_version: "cori 0.2.4",
      author_cori_sha256: "a".repeat(64),
      harness_path: "/usr/local/bin/codex",
      harness_version: "codex-cli 1.0.0",
      harness_sha256: "b".repeat(64),
      gws: "gws",
      gws_path: "/usr/local/bin/gws",
      gws_version: "gws 0.22.5",
      gws_sha256: "c".repeat(64),
      temporal_path: "/usr/local/bin/temporal",
      temporal_version: "temporal version 1.7.2 (Server 1.31.1, UI 2.49.1)",
      temporal_sha256: "d".repeat(64),
      deno_path: "/usr/local/bin/deno",
      deno_version: "deno 2.8.1",
      deno_sha256: "e".repeat(64),
      node_path: process.execPath,
      node_version: process.version,
      node_sha256: "f".repeat(64),
      subject_isolation: "macos_sandbox_exec_repo_read_deny",
      subject_isolation_path: "/usr/bin/sandbox-exec",
      subject_isolation_sha256: "1".repeat(64),
      benchmark_source_sha256: "2".repeat(64),
      capture_skill_sha256: "3".repeat(64),
      workspace_account_sha256: "4".repeat(64),
      calendar_id: "benchmark@example.com",
      author_model: "gpt-5.6-terra",
      llm_model: "gpt-5.4",
      os: "darwin",
      arch: "arm64",
      timezone: "Europe/Paris",
    },
    capture: {
      previewDidNotWrite: true,
      checkPassed: true,
      policy: { ok: true, violations: [], workflowHash: "abc" },
      tasks: [{
        taskId: "inbound_lead_qualification",
        authorGrade: completeGrade,
        outcomes: sampleOutcomes(),
        previewPresented: true,
        previewDidNotWrite: true,
        skillCheckObserved: true,
        skillCheckSucceeded: true,
        benchmarkCheckSucceeded: true,
        runtimeModelDataflowVerified: true,
        checkPassed: true,
        policy: { ok: true, violations: [], workflowHash: "abc" },
        workflowHash: "abc",
        workflowPath: "/tmp/captured-workflow",
      }],
    },
    phaseTimingsMs: {
      author: 1_000,
      capture: 2_000,
      check: 500,
      replay: 10_000,
    },
    trials,
    metrics: {
      directWallTimeMs: 60_584,
      replayWallTimeMs: 12_499,
      designTokens: 479_569,
      runtimeTokens: 0,
      runtimeCostEur: 0,
      designWallTimeMs: 3_500,
      directSuiteWallTimeMs: 60_584,
      replaySuiteWallTimeMs: 12_499,
      breakEvenRepetitions: 2,
    },
    summary: {
      directScore: mean(direct.map((trial) => trial.grade.score)),
      replayScore: mean(replay.map((trial) => trial.grade.score)),
      pairedSampleSize: Math.min(direct.length, replay.length),
      combinedResult: false,
      inferenceEligible: false,
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
    taskId: "inbound_lead_qualification",
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
  return { sheets: [gridSheet(table)] };
}

/** One tab of a spreadsheet, for fixtures that write several. */
function gridSheet(table: readonly (readonly string[])[]): Json {
  return {
    data: [{
      rowData: table.map((row) => ({
        values: row.map((formattedValue) => ({ formattedValue })),
      })),
    }],
  };
}

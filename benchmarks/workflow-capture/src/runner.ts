import {
  access,
  cp,
  mkdir,
  readdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  artifactPath,
  normalizedCsv,
  readJson,
  scorecard,
  writeJson,
} from "./artifacts.js";
import { gradeExternalState } from "./grader.js";
import { adapterFor, codexModel } from "./harness.js";
import {
  configuredBenchmarkCalendarId,
  GwsClient,
  requireBenchmarkCalendarId,
  runProcess,
  WorkspaceScenarioDriver,
} from "./gws.js";
import { hashDirectory, inspectWorkflowPolicy } from "./policy.js";
import { assertTwinEquivalent, buildScenario } from "./scenario.js";
import {
  breakEvenRepetitions,
  mean,
  pairedDifferenceCi95,
  reuseAdvantage,
} from "./statistics.js";
import { assertTaskCatalog, taskById, TASKS } from "./tasks.js";
import { writeBenchmarkViewerForRun } from "./viewer.js";
import type {
  BenchmarkProfile,
  BenchmarkResultV1,
  HarnessName,
  RegisteredResource,
  Scenario,
  TaskCapture,
  TaskSpec,
  TrialResult,
  WorkspaceSnapshot,
} from "./types.js";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const repoRoot = resolve(packageRoot, "../..");
const referencesRoot = join(packageRoot, "reference-workflows");
const defaultArtifactsRoot = join(packageRoot, "artifacts");
const workspaceCoriTargetRoot = join(repoRoot, "target");
const maxCaptureAttempts = 3;
const replayTraceFailure =
  "Cori replay failed or did not emit a successful JSON trace";
const replayMutationFailure =
  "workflow directory changed during held-out replay";
const retryableReplayIntegrityFailures = new Set([
  replayTraceFailure,
  replayMutationFailure,
]);
let coriPreparation: Promise<void> | undefined;
let coriBinarySha256: string | null = null;
let coriBinarySource: "workspace_dev" | "override" = process.env
    .CORI_BENCH_CORI
  ? "override"
  : "workspace_dev";

export interface RunOptions {
  profile: BenchmarkProfile;
  harness: HarnessName;
  seed: number;
  taskIds?: readonly string[];
  artifactsRoot?: string;
  runId?: string;
  batch?: { index: number; count: number };
  onProgress?: (progress: BenchmarkProgress) => void;
}

export interface BenchmarkProgress {
  version: 1;
  runId: string;
  status: "running" | "succeeded" | "failed";
  phase: string;
  detail: string;
  taskId: string | null;
  taskNumber: number | null;
  totalTasks: number;
  completedTasks: readonly string[];
  completedDirectTrials: number;
  completedReplayTrials: number;
  plannedTrialsPerLane: number;
  startedAt: string;
  updatedAt: string;
}

interface CleanupRegistry {
  runId: string;
  resources: RegisteredResource[];
  runTags: string[];
  /** Shared calendar containing run-tagged fixtures and outputs; never deleted. */
  calendarId?: string;
}

interface CapturedTaskWorkflow {
  capture: TaskCapture;
  workflowDir: string | null;
  authorWorkspace: string;
}

interface CaptureWorkflowArgs {
  task: TaskSpec;
  seed: number;
  runId: string;
  runDir: string;
  agentRoot: string;
  driver: WorkspaceScenarioDriver;
  adapter: ReturnType<typeof adapterFor>;
  registry: CleanupRegistry;
  model: string;
  onProgress: (phase: string, detail: string) => Promise<void>;
}

export async function validate(): Promise<void> {
  assertTaskCatalog();
  for (const task of TASKS) {
    for (const seed of [42, 43, 44]) {
      buildScenario(task.id, seed, "author", "offline-validation");
      buildScenario(task.id, seed, "direct", "offline-validation");
      buildScenario(task.id, seed, "replay", "offline-validation");
    }
    const reference = join(referencesRoot, task.id);
    const policy = await inspectWorkflowPolicy(reference);
    if (!policy.ok) {
      throw new Error(
        `reference workflow ${task.id} violates policy:\n${
          policy.violations.join("\n")
        }`,
      );
    }
    const manifest = await readFile(join(reference, "manifest.md"), "utf8");
    for (const parameter of task.parameters) {
      if (!new RegExp(`name:\\s+${parameter.name}\\b`, "u").test(manifest)) {
        throw new Error(
          `reference workflow ${task.id} is missing parameter ${parameter.name}`,
        );
      }
    }
  }
}

/** Explicit, credentialed environment check. It is the only command that creates a canary. */
export async function preflight(
  artifactsRoot = defaultArtifactsRoot,
): Promise<Record<string, string>> {
  await prepareCoriWorkflowCli();
  const calendarId = requireBenchmarkCalendarId();
  const gws = new GwsClient();
  const driver = new WorkspaceScenarioDriver(gws, undefined, calendarId);
  const gwsVersion = await gws.version();
  if (gwsVersion !== "gws 0.22.5") {
    throw new Error(
      `expected gws 0.22.5, found ${gwsVersion}; update the benchmark lock deliberately`,
    );
  }
  for (const binary of [coriBinary(), "temporal", "deno"]) {
    await ensureExecutable(binary);
  }
  await ensureCoriWorkflowCli();
  if (!process.env.CORI_BENCH_LLM_MODEL) {
    throw new Error(
      "CORI_BENCH_LLM_MODEL is required for the three hybrid tasks",
    );
  }
  const llmProvider = providerForModel(process.env.CORI_BENCH_LLM_MODEL);
  await ensureCoriCapability(llmProvider);
  const schema = await gws.call([
    "schema",
    "sheets.spreadsheets.values.batchUpdate",
  ]);
  const schemaHash = await sha256(JSON.stringify(schema));
  const calendar = await driver.verifyCalendar();
  const runTag = `cori-bench-preflight-${Date.now()}`;
  await gws.canary(runTag);
  const report = {
    gwsVersion,
    schemaHash,
    calendarId: calendar.id,
    calendarSummary: calendar.summary,
    cori: await version(coriBinary()),
    coriPath: coriBinary(),
    coriSource: coriBinarySource,
    coriSha256: coriBinarySha256 ?? "unavailable",
    coriLlmCapability: llmProvider,
    temporal: await version("temporal"),
    deno: await version("deno"),
  };
  await writeJson(join(artifactsRoot, "preflight.json"), report);
  return report;
}

export async function runBenchmark(
  options: RunOptions,
): Promise<BenchmarkResultV1> {
  await validate();
  const tasks = selectTasks(options);
  const batchSuffix = options.batch
    ? `-b${options.batch.index}of${options.batch.count}`
    : "";
  const runId = options.runId ??
    `workflow-capture-${
      new Date().toISOString().replace(/[:.]/gu, "-")
    }-${options.seed}${batchSuffix}`;
  if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/u.test(runId)) {
    throw new Error(
      "run ID must contain only letters, numbers, dots, underscores, and hyphens",
    );
  }
  const runDir = join(options.artifactsRoot ?? defaultArtifactsRoot, runId);
  const agentRoot = join(runDir, "agent-workspace");
  await mkdir(runDir, { recursive: true });
  const calendarId = configuredBenchmarkCalendarId();
  const gws = new GwsClient();
  const driver = new WorkspaceScenarioDriver(gws, undefined, calendarId);
  const adapter = adapterFor(options.harness);
  const registry: CleanupRegistry = {
    runId,
    resources: [],
    runTags: [],
    ...(calendarId ? { calendarId } : {}),
  };
  const trials: TrialResult[] = [];
  const captures: TaskCapture[] = [];
  const startedAt = new Date().toISOString();
  const completedTasks: string[] = [];
  const repetitions = profileShape(options.profile);
  let progress: BenchmarkProgress = {
    version: 1,
    runId,
    status: "running",
    phase: "starting",
    detail: "initializing benchmark",
    taskId: null,
    taskNumber: null,
    totalTasks: tasks.length,
    completedTasks,
    completedDirectTrials: 0,
    completedReplayTrials: 0,
    plannedTrialsPerLane: tasks.length * repetitions.trials *
      repetitions.heldoutPairs,
    startedAt,
    updatedAt: startedAt,
  };
  const publishProgress = async (
    phase: string,
    detail: string,
    task?: TaskSpec,
  ): Promise<void> => {
    progress = {
      ...progress,
      phase,
      detail,
      taskId: task?.id ?? null,
      taskNumber: task
        ? tasks.findIndex((candidate) => candidate.id === task.id) + 1
        : null,
      completedTasks: [...completedTasks],
      completedDirectTrials:
        trials.filter((trial) => trial.lane === "direct").length,
      completedReplayTrials:
        trials.filter((trial) => trial.lane === "replay").length,
      updatedAt: new Date().toISOString(),
    };
    await writeJson(join(runDir, "progress.json"), progress);
    options.onProgress?.(progress);
  };
  let runError: string | undefined;

  try {
    if (tasks.some((task) => task.requiredServices.includes("calendar"))) {
      requireBenchmarkCalendarId();
      await driver.verifyCalendar();
    }
    await publishProgress(
      "environment_check",
      process.env.CORI_BENCH_CORI
        ? "checking the explicitly selected Cori executable and harness capabilities"
        : "building the current workspace Cori development binary and checking harness capabilities",
    );
    await adapter.version();
    if (
      tasks.some((task) => task.runtimeTrack === "hybrid") &&
      !process.env.CORI_BENCH_LLM_MODEL
    ) {
      throw new Error(
        "CORI_BENCH_LLM_MODEL is required when the selected benchmark tasks include the hybrid runtime track",
      );
    }
    await prepareCoriWorkflowCli();
    await publishProgress(
      "environment_check",
      `using ${coriBinary()}${
        coriBinarySha256 ? ` (sha256 ${coriBinarySha256.slice(0, 12)})` : ""
      }`,
    );
    await ensureCoriWorkflowCli();
    if (tasks.some((task) => task.runtimeTrack === "hybrid")) {
      await ensureCoriCapability(
        providerForModel(process.env.CORI_BENCH_LLM_MODEL ?? ""),
      );
    }
    for (const task of tasks) {
      await publishProgress(
        "author_direct",
        "running task author against the live fixture",
        task,
      );
      const captured = await captureWorkflowForTask({
        task,
        seed: options.seed,
        runId,
        runDir,
        agentRoot,
        driver,
        adapter,
        registry,
        model: process.env.CORI_BENCH_LLM_MODEL ?? "",
        onProgress: (phase, detail) => publishProgress(phase, detail, task),
      });
      captures.push(captured.capture);
      if (!captureReady(captured.capture)) {
        throw new Error(
          `captured workflow for ${task.id} is not replayable: ${
            captured.capture.error ?? "capture safety or check gate failed"
          }`,
        );
      }
      for (let trial = 0; trial < repetitions.trials; trial += 1) {
        for (
          let heldout = 0;
          heldout < repetitions.heldoutPairs;
          heldout += 1
        ) {
          const scenarioSeed = options.seed + trial * 10_000 + heldout + 1;
          const directScenarioBase = buildScenario(
            task.id,
            scenarioSeed,
            "direct",
            runId,
          );
          const replayScenarioBase = buildScenario(
            task.id,
            scenarioSeed,
            "replay",
            runId,
          );
          assertTwinEquivalent(directScenarioBase, replayScenarioBase);
          const directScenario = await provision(
            driver,
            directScenarioBase,
            registry,
            runDir,
          );
          const replayScenario = await provision(
            driver,
            replayScenarioBase,
            registry,
            runDir,
          );

          await publishProgress(
            "heldout_direct",
            `running direct pair ${trial + 1}.${heldout + 1}`,
            task,
          );
          const directWorkspace = join(
            agentRoot,
            `${task.id}-${trial}-${heldout}-direct`,
          );
          await prepareDirectWorkspace(
            directWorkspace,
            task.id,
            directScenario,
          );
          const beforeDirect = await driver.snapshot(directScenario);
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${trial}-${heldout}-direct-before.json`,
            ),
            beforeDirect,
          );
          const direct = await adapter.start(
            renderedTaskPrompt(task.id, directScenario, "direct"),
            directWorkspace,
          );
          const afterDirect = await driver.snapshot(
            directScenario,
            { settleTaggedOutputs: true },
          );
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${trial}-${heldout}-direct-after.json`,
            ),
            afterDirect,
          );
          const directGrade = gradeExternalState(
            directScenario,
            beforeDirect,
            afterDirect,
          );
          trials.push({
            taskId: task.id,
            seed: scenarioSeed,
            lane: "direct",
            grade: directGrade,
            harness: direct,
          });
          await publishProgress(
            "heldout_direct_complete",
            `direct pair ${trial + 1}.${
              heldout + 1
            } scored ${directGrade.score}`,
            task,
          );

          if (captured.workflowDir && captureReady(captured.capture)) {
            await publishProgress(
              "heldout_replay",
              `running Cori replay pair ${trial + 1}.${heldout + 1}`,
              task,
            );
            const beforeReplay = await driver.snapshot(replayScenario);
            await writeJson(
              artifactPath(
                runDir,
                "snapshots",
                `${task.id}-${trial}-${heldout}-replay-before.json`,
              ),
              beforeReplay,
            );
            const workflowHash = await hashDirectory(captured.workflowDir);
            const replay = await runCori([
              "run",
              captured.workflowDir,
              "--json",
              ...parameterArgs(replayScenario),
            ], captured.authorWorkspace);
            await writeJson(
              artifactPath(
                runDir,
                "cori-traces",
                `${task.id}-${trial}-${heldout}.json`,
              ),
              replay,
            );
            const afterReplay = await driver.snapshot(
              replayScenario,
              { settleTaggedOutputs: true },
            );
            await writeJson(
              artifactPath(
                runDir,
                "snapshots",
                `${task.id}-${trial}-${heldout}-replay-after.json`,
              ),
              afterReplay,
            );
            const trace = parseTrace(replay.stdout);
            const unchanged =
              workflowHash === await hashDirectory(captured.workflowDir);
            const replayGrade = hardGate(
              gradeExternalState(replayScenario, beforeReplay, afterReplay),
              replay.code === 0 && traceSucceeded(trace),
              unchanged,
            );
            trials.push({
              taskId: task.id,
              seed: scenarioSeed,
              lane: "replay",
              grade: replayGrade,
              tracePath: artifactPath(
                runDir,
                "cori-traces",
                `${task.id}-${trial}-${heldout}.json`,
              ),
              workflowHash,
              runtime: traceUsage(trace),
            });
            await publishProgress(
              "heldout_replay_complete",
              `replay pair ${trial + 1}.${
                heldout + 1
              } scored ${replayGrade.score}`,
              task,
            );
          }
        }
      }
      completedTasks.push(task.id);
      await publishProgress(
        "task_complete",
        `completed all direct/replay pairs for ${task.id}`,
        task,
      );
    }
    runError = trialIntegrityError(trials);
  } catch (error) {
    runError = error instanceof Error ? error.message : String(error);
  } finally {
    await writeJson(join(runDir, "cleanup-registry.json"), registry);
  }

  const result = summarize(
    runId,
    options,
    startedAt,
    aggregateCaptures(captures),
    trials,
    runError,
  );
  await writeArtifacts(runDir, result);
  progress = { ...progress, status: runError ? "failed" : "succeeded" };
  await publishProgress(
    runError ? "failed" : "complete",
    runError ?? "benchmark completed; trial scores are reported as measurements",
  );
  await writeBenchmarkViewerForRun(runDir);
  if (runError) {
    throw new Error(
      `${runError}\nBenchmark artifacts were written to ${runDir}`,
    );
  }
  return result;
}

export async function cleanup(
  runId: string,
  artifactsRoot = defaultArtifactsRoot,
): Promise<void> {
  const runDir = join(artifactsRoot, runId);
  const registry = await readJson<CleanupRegistry>(
    join(runDir, "cleanup-registry.json"),
  );
  const driver = new WorkspaceScenarioDriver(
    new GwsClient(),
    undefined,
    registry.calendarId,
  );
  const failures: string[] = [];
  try {
    await driver.cleanup(registry.resources);
  } catch (error) {
    failures.push(error instanceof Error ? error.message : String(error));
  }
  for (const runTag of registry.runTags) {
    try {
      await driver.cleanupTagged(runTag);
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
  }
  if (failures.length === 0) {
    await writeJson(join(runDir, "cleanup-registry.json"), {
      ...registry,
      resources: [],
      runTags: [],
    });
    await writeBenchmarkViewerForRun(runDir);
  }
  if (failures.length > 0) {
    throw new Error(`cleanup completed with failures:\n${failures.join("\n")}`);
  }
}

export async function report(
  runId: string,
  artifactsRoot = defaultArtifactsRoot,
): Promise<BenchmarkResultV1> {
  const runDir = join(artifactsRoot, runId);
  const existing = await readJson<BenchmarkResultV1>(
    join(runDir, "result.json"),
  );
  const trials = await Promise.all(existing.trials.map(async (trial) => {
    if (trial.lane !== "replay" || !trial.tracePath) return trial;
    try {
      const traceProcess = await readJson<{ stdout: string }>(trial.tracePath);
      return { ...trial, runtime: traceUsage(parseTrace(traceProcess.stdout)) };
    } catch {
      return trial;
    }
  }));
  const currentTrialError = trialIntegrityError(trials);
  const runError = recoverableLegacyScoreError(existing)
    ? currentTrialError
    : existing.error ?? currentTrialError;
  const summarized = summarize(
    existing.runId,
    {
      profile: existing.profile,
      harness: existing.harness,
      seed: existing.seed,
      artifactsRoot,
    },
    existing.startedAt,
    existing.capture,
    trials,
    runError,
  );
  const result = {
    ...summarized,
    status: runError ? "failed" as const : "succeeded" as const,
    finishedAt: existing.finishedAt,
    environment: existing.environment,
  };
  await writeArtifacts(runDir, result);
  await writeBenchmarkViewerForRun(runDir);
  return result;
}

export async function combineRuns(
  runIds: readonly string[],
  artifactsRoot = defaultArtifactsRoot,
  requestedRunId?: string,
): Promise<BenchmarkResultV1> {
  if (runIds.length < 2) {
    throw new Error("combine requires at least two batch run IDs");
  }
  const sources = await Promise.all(
    runIds.map((runId) =>
      readJson<BenchmarkResultV1>(join(artifactsRoot, runId, "result.json"))
    ),
  );
  const first = sources[0]!;
  const calendarIds = sources.flatMap((source) =>
    typeof source.environment.calendar_id === "string"
      ? [source.environment.calendar_id]
      : []
  );
  if (new Set(calendarIds).size > 1) {
    throw new Error(
      "combined runs must use the same CORI_BENCH_CALENDAR_ID",
    );
  }
  for (const source of sources) {
    if (
      source.status !== "succeeded" &&
      !recoverableLegacyScoreError(source)
    ) {
      throw new Error(`cannot combine failed run ${source.runId}`);
    }
    if (
      source.profile !== first.profile || source.harness !== first.harness ||
      source.seed !== first.seed
    ) {
      throw new Error(
        "combined runs must have identical profile, harness, and seed",
      );
    }
    if (
      source.environment.llm_model !== first.environment.llm_model ||
      source.environment.author_model !== first.environment.author_model ||
      source.environment.cori !== first.environment.cori ||
      source.environment.cori_sha256 !== first.environment.cori_sha256
    ) {
      throw new Error(
        "combined runs must use the same Cori executable build, author model, and workflow LLM model",
      );
    }
  }
  const captures = sources.flatMap((source) => source.capture.tasks);
  const capturedIds = captures.map((capture) => capture.taskId);
  const duplicates = capturedIds.filter((id, index) =>
    capturedIds.indexOf(id) !== index
  );
  if (duplicates.length > 0) {
    throw new Error(
      `combined runs overlap on tasks: ${[...new Set(duplicates)].join(", ")}`,
    );
  }
  const missing = TASKS.map((task) => task.id).filter((id) =>
    !capturedIds.includes(id)
  );
  const extra = capturedIds.filter((id) =>
    !TASKS.some((task) => task.id === id)
  );
  if (missing.length > 0 || extra.length > 0) {
    throw new Error(
      `combined runs must cover the ten-task catalog exactly (missing: ${
        missing.join(", ") || "none"
      }; extra: ${extra.join(", ") || "none"})`,
    );
  }
  const repetitions = profileShape(first.profile);
  const expectedPerLane = repetitions.trials * repetitions.heldoutPairs;
  const trials = sources.flatMap((source) => source.trials);
  for (const task of TASKS) {
    for (const lane of ["direct", "replay"] as const) {
      const count = trials.filter((trial) =>
        trial.taskId === task.id && trial.lane === lane
      ).length;
      if (count !== expectedPerLane) {
        throw new Error(
          `${task.id} has ${count} ${lane} trials; expected ${expectedPerLane}`,
        );
      }
    }
  }
  const runId = requestedRunId ??
    `workflow-capture-combined-${
      new Date().toISOString().replace(/[:.]/gu, "-")
    }-${first.seed}`;
  if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/u.test(runId)) {
    throw new Error(
      "run ID must contain only letters, numbers, dots, underscores, and hyphens",
    );
  }
  const startedAt = sources.map((source) => source.startedAt).sort()[0] ??
    new Date().toISOString();
  const result = {
    ...summarize(
      runId,
      { profile: first.profile, harness: first.harness, seed: first.seed },
      startedAt,
      aggregateCaptures(captures),
      trials,
      trialIntegrityError(trials),
    ),
    environment: {
      ...first.environment,
      calendar_id: sources.map((source) => source.environment.calendar_id)
        .find((calendarId): calendarId is string =>
          typeof calendarId === "string"
        ) ?? null,
    },
  };
  const runDir = join(artifactsRoot, runId);
  await mkdir(runDir, { recursive: true });
  await writeArtifacts(runDir, result);
  await writeJson(join(runDir, "source-runs.json"), { runIds });
  await writeBenchmarkViewerForRun(runDir);
  return result;
}

function summarize(
  runId: string,
  options: RunOptions,
  startedAt: string,
  capture: BenchmarkResultV1["capture"],
  trials: readonly TrialResult[],
  runError?: string,
): BenchmarkResultV1 {
  const direct = trials.filter((trial) => trial.lane === "direct");
  const replay = trials.filter((trial) => trial.lane === "replay");
  const directScore = mean(direct.map((trial) => trial.grade.score));
  const replayScore = mean(replay.map((trial) => trial.grade.score));
  const paired = pairedDifferenceCi95(
    direct.map((trial) => trial.grade.score),
    replay.map((trial) => trial.grade.score),
    options.seed,
  );
  const directTime = mean(
    direct.flatMap((trial) => trial.harness ? [trial.harness.wallTimeMs] : []),
  );
  const replayTime = mean(
    replay.flatMap((trial) =>
      trial.runtime?.wallTimeMs !== null &&
        trial.runtime?.wallTimeMs !== undefined
        ? [trial.runtime.wallTimeMs]
        : []
    ),
  );
  const designTokens = sumNullable(
    direct.map((trial) => tokenSum(trial.harness)),
  );
  const safetyViolations = trials.reduce(
    (sum, trial) => sum + trial.grade.safetyViolations.length,
    0,
  );
  const designCost = directTime ?? null;
  const runtimeInputTokens = sumNullable(
    replay.map((trial) => trial.runtime?.inputTokens ?? null),
  );
  const runtimeOutputTokens = sumNullable(
    replay.map((trial) => trial.runtime?.outputTokens ?? null),
  );
  const result: BenchmarkResultV1 = {
    version: 1,
    status: runError ? "failed" : "succeeded",
    runId,
    profile: options.profile,
    harness: options.harness,
    seed: options.seed,
    startedAt,
    finishedAt: new Date().toISOString(),
    environment: {
      cori: coriBinary(),
      cori_source: coriBinarySource,
      cori_sha256: coriBinarySha256,
      gws: process.env.GWS_BIN ?? "gws",
      calendar_id: configuredBenchmarkCalendarId() ?? null,
      author_model: options.harness === "codex" ? codexModel() : null,
      llm_model: process.env.CORI_BENCH_LLM_MODEL ?? null,
      os: process.platform,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    },
    capture,
    trials,
    metrics: {
      directWallTimeMs: directTime,
      replayWallTimeMs: replayTime,
      designTokens,
      runtimeTokens: runtimeInputTokens !== null && runtimeOutputTokens !== null
        ? runtimeInputTokens + runtimeOutputTokens
        : null,
      runtimeCostEur: sumNullable(
        replay.map((trial) => trial.runtime?.costEur ?? null),
      ),
      breakEvenRepetitions:
        designCost !== null && directTime !== null && replayTime !== null
          ? breakEvenRepetitions(designCost, directTime, replayTime)
          : null,
    },
    summary: {
      directScore,
      replayScore,
      pairedDifferenceCi95: paired,
      reuseAdvantageDemonstrated: reuseAdvantage(
        safetyViolations,
        paired,
        designCost,
        directTime,
        replayTime,
      ),
    },
    ...(runError ? { error: runError } : {}),
  };
  return result;
}

async function writeArtifacts(
  runDir: string,
  result: BenchmarkResultV1,
): Promise<void> {
  await writeJson(join(runDir, "result.json"), result);
  await writeFile(
    join(runDir, "scorecard.md"),
    `${scorecard(result)}\n`,
    "utf8",
  );
  await writeFile(join(runDir, "results.csv"), normalizedCsv(result), "utf8");
}

async function provision(
  driver: WorkspaceScenarioDriver,
  scenario: Scenario,
  registry: CleanupRegistry,
  runDir: string,
): Promise<Scenario> {
  const provisioned = await driver.provision(scenario);
  registry.resources.push(...provisioned.resources);
  registry.runTags.push(provisioned.runTag);
  await writeJson(join(runDir, "cleanup-registry.json"), registry);
  return provisioned;
}

/**
 * Capture and validate a workflow for exactly one task.  A captured workflow is
 * deliberately never shared with another task: otherwise the held-out lane is
 * not measuring reuse of the task the agent actually completed.
 */
async function captureWorkflowForTask({
  ...args
}: CaptureWorkflowArgs): Promise<CapturedTaskWorkflow> {
  const attempts: Array<NonNullable<TaskCapture["attempts"]>[number]> = [];
  let latest: CapturedTaskWorkflow | null = null;
  let priorCaptureError: string | undefined;
  for (let attempt = 1; attempt <= maxCaptureAttempts; attempt += 1) {
    const attemptSeed = args.seed + (attempt - 1) * 100_000;
    const captured = await captureWorkflowAttempt({
      ...args,
      seed: attemptSeed,
      attempt,
      priorCaptureError,
    });
    const ready = captureReady(captured.capture);
    attempts.push({
      attempt,
      seed: attemptSeed,
      authorGrade: captured.capture.authorGrade,
      ready,
      ...(captured.capture.error ? { error: captured.capture.error } : {}),
    });
    latest = {
      ...captured,
      capture: {
        ...captured.capture,
        attempts: [...attempts],
        ...(ready ? { selectedAttempt: attempt } : {}),
      },
    };
    if (ready) {
      if (captured.workflowDir) {
        await copyWorkflow(
          captured.workflowDir,
          artifactPath(args.runDir, "generated-workflows", args.task.id),
        );
      }
      return latest;
    }
    if (
      attempt === maxCaptureAttempts ||
      !retryableCaptureFailure(captured.capture)
    ) {
      if (
        attempt === maxCaptureAttempts &&
        latest.capture.error
      ) {
        latest = {
          ...latest,
          capture: {
            ...latest.capture,
            error:
              `capture did not become replayable after ${attempts.length} attempts; last attempt: ${
                latest.capture.error
              }`,
          },
        };
      }
      return latest;
    }
    await args.onProgress(
      "capture_retry",
      `capture attempt ${attempt}/${maxCaptureAttempts} was not replayable (${
        captured.capture.error ?? `score ${captured.capture.authorGrade.score}`
      }); retrying on a fresh fixture`,
    );
    priorCaptureError = captured.capture.error;
  }
  if (!latest) {
    throw new Error(`capture for ${args.task.id} produced no attempts`);
  }
  return latest;
}

async function captureWorkflowAttempt({
  task,
  seed,
  runId,
  runDir,
  agentRoot,
  driver,
  adapter,
  registry,
  model,
  onProgress,
  attempt,
  priorCaptureError,
}: CaptureWorkflowArgs & {
  attempt: number;
  priorCaptureError?: string;
}): Promise<CapturedTaskWorkflow> {
  const attemptNamespace = attempt === 1
    ? runId
    : `${runId}-capture-attempt-${attempt}`;
  const artifactStem = attempt === 1
    ? task.id
    : `${task.id}-attempt-${attempt}`;
  const authorScenario = await provision(
    driver,
    buildScenario(task.id, seed, "author", attemptNamespace),
    registry,
    runDir,
  );
  const authorWorkspace = join(agentRoot, "authors", artifactStem);
  await prepareDirectWorkspace(authorWorkspace, task.id, authorScenario);
  await onProgress(
    "author_direct",
    `agent is completing the author fixture (capture attempt ${attempt}/${maxCaptureAttempts})`,
  );
  const beforeAuthor = await driver.snapshot(authorScenario);
  await writeJson(
    artifactPath(runDir, "snapshots", "authors", `${artifactStem}-before.json`),
    beforeAuthor,
  );
  const directAuthor = await adapter.start(
    renderedTaskPrompt(task.id, authorScenario, "direct"),
    authorWorkspace,
  );
  await writeJson(
    artifactPath(runDir, "transcripts", "authors", `${artifactStem}-direct.json`),
    directAuthor,
  );
  const afterAuthor = await driver.snapshot(
    authorScenario,
    { settleTaggedOutputs: true },
  );
  await writeJson(
    artifactPath(runDir, "snapshots", "authors", `${artifactStem}-after.json`),
    afterAuthor,
  );
  const authorGrade = gradeExternalState(
    authorScenario,
    beforeAuthor,
    afterAuthor,
  );
  await writeJson(
    artifactPath(runDir, "author-grades", `${artifactStem}.json`),
    authorGrade,
  );
  await writeJson(
    artifactPath(runDir, "author-grades", `${task.id}.json`),
    authorGrade,
  );
  if (authorGrade.safetyViolations.length > 0) {
    return {
      capture: {
        taskId: task.id,
        authorGrade,
        previewDidNotWrite: false,
        checkPassed: false,
        policy: null,
        workflowPath: null,
        error:
          `author task violated benchmark safety: ${
            authorGrade.safetyViolations.join("; ")
          }`,
      },
      workflowDir: null,
      authorWorkspace,
    };
  }

  await prepareCaptureWorkspace(authorWorkspace, task.id, model);
  const workflowDir = join(authorWorkspace, "captured-workflow");
  await onProgress("capture_preview", "requesting read-only workflow preview");
  const absentBeforePreview =
    !(await containsWorkflowManifest(authorWorkspace));
  const preview = directAuthor.sessionId
    ? await adapter.resume(
      directAuthor.sessionId,
      previewPrompt(),
      authorWorkspace,
    )
    : directAuthor;
  await writeJson(
    artifactPath(
      runDir,
      "transcripts",
      "authors",
      `${artifactStem}-capture-preview.json`,
    ),
    preview,
  );
  const previewDidNotWrite = absentBeforePreview &&
    !(await containsWorkflowManifest(authorWorkspace)) &&
    !transcriptExecutedCoriRun(preview);

  await onProgress(
    "capture_approval",
    "approving workflow files after preview gate",
  );
  const approval = preview.sessionId
    ? await adapter.resume(
      preview.sessionId,
      approvalPrompt(task, priorCaptureError),
      authorWorkspace,
    )
    : preview;
  await writeJson(
    artifactPath(
      runDir,
      "transcripts",
      "authors",
      `${artifactStem}-capture-approval.json`,
    ),
    approval,
  );

  let policy: TaskCapture["policy"] = null;
  let checkPassed = false;
  let qualificationPassed = false;
  let qualificationGrade: TaskCapture["qualificationGrade"];
  let qualificationTracePath: string | undefined;
  let checkError: string | undefined;
  if (await exists(workflowDir)) {
    policy = await inspectWorkflowPolicy(
      workflowDir,
      [
        authorScenario.runTag,
        ...authorScenario.resources.map((resource) => resource.id),
      ],
      task.parameters.map((parameter) => parameter.name),
    );
    await copyWorkflow(
      workflowDir,
      artifactPath(
        runDir,
        "generated-workflows",
        "attempts",
        artifactStem,
      ),
    );
    await onProgress(
      "workflow_check",
      "running static policy and Cori capability checks",
    );
    const checked = await runCori(["check", workflowDir], authorWorkspace);
    await writeJson(
      artifactPath(runDir, "cori-check", `${artifactStem}.json`),
      checked,
    );
    checkPassed = checked.code === 0 && policy.ok;
    checkError = formatWorkflowCheckFailure(checked, policy);
  } else {
    checkError = "approved author session did not create captured-workflow";
  }

  if (previewDidNotWrite && checkPassed && policy?.ok) {
    await onProgress(
      "workflow_qualification",
      "executing Cori once against an unscored disposable scenario",
    );
    const qualificationBase = buildScenario(
      task.id,
      seed + 900_000,
      "replay",
      `${runId}-qualification`,
    );
    const qualification = await provision(
      driver,
      qualificationBase,
      registry,
      runDir,
    );
    const beforeQualification = await driver.snapshot(qualification);
    await writeJson(
      artifactPath(runDir, "qualification", `${artifactStem}-before.json`),
      beforeQualification,
    );
    const workflowHash = await hashDirectory(workflowDir);
    const executed = await runCori([
      "run",
      workflowDir,
      "--json",
      ...parameterArgs(qualification),
    ], authorWorkspace);
    qualificationTracePath = artifactPath(
      runDir,
      "qualification",
      `${artifactStem}-trace.json`,
    );
    await writeJson(qualificationTracePath, executed);
    const afterQualification = await driver.snapshot(
      qualification,
      { settleTaggedOutputs: true },
    );
    await writeJson(
      artifactPath(runDir, "qualification", `${artifactStem}-after.json`),
      afterQualification,
    );
    const trace = parseTrace(executed.stdout);
    const traceOk = executed.code === 0 && traceSucceeded(trace);
    const traceDiagnostic = traceOk
      ? undefined
      : failedTraceDiagnostic(trace, executed);
    const hashUnchanged = workflowHash === await hashDirectory(workflowDir);
    qualificationGrade = hardGate(
      gradeExternalState(
        qualification,
        beforeQualification,
        afterQualification,
      ),
      traceOk,
      hashUnchanged,
    );
    qualificationPassed = qualificationGrade.score === 100 &&
      qualificationGrade.safetyViolations.length === 0;
    if (!qualificationPassed) {
      const violations = qualificationGrade.safetyViolations.join("; ");
      const incomplete = qualificationGrade.items
        .filter((item) => item.earned < item.max)
        .map((item) => item.id)
        .join(", ");
      checkError = violations
        ? `qualification safety or replay-integrity failed: ${violations}${
          traceDiagnostic ? `; Cori diagnostic: ${traceDiagnostic}` : ""
        }`
        : `qualification scored ${qualificationGrade.score}/100; Cori workflows must score 100${
          incomplete ? ` (incomplete: ${incomplete})` : ""
        }`;
    }
  }

  return {
    capture: {
      taskId: task.id,
      authorGrade,
      previewDidNotWrite,
      checkPassed,
      qualificationPassed,
      ...(qualificationGrade ? { qualificationGrade } : {}),
      ...(qualificationTracePath ? { qualificationTracePath } : {}),
      policy,
      workflowPath: await exists(workflowDir) ? workflowDir : null,
      ...(!previewDidNotWrite
        ? {
          error:
            "preview gate detected workflow writes or a Cori run before approval",
        }
        : checkError
        ? { error: checkError }
        : {}),
    },
    workflowDir: checkPassed ? workflowDir : null,
    authorWorkspace,
  };
}

export function captureReady(capture: TaskCapture): boolean {
  return capture.authorGrade.safetyViolations.length === 0 &&
    capture.previewDidNotWrite && capture.checkPassed &&
    capture.policy?.ok === true && capture.qualificationPassed === true &&
    capture.qualificationGrade?.score === 100 &&
    capture.qualificationGrade.safetyViolations.length === 0;
}

export function retryableCaptureFailure(capture: TaskCapture): boolean {
  if (capture.authorGrade.safetyViolations.length > 0) return false;
  if (!capture.previewDidNotWrite) return false;
  const qualificationViolations =
    capture.qualificationGrade?.safetyViolations ?? [];
  if (
    qualificationViolations.some((violation) =>
      !retryableReplayIntegrityFailures.has(violation)
    )
  ) return false;
  return true;
}

export function formatWorkflowCheckFailure(
  checked: { code: number; stdout: string; stderr: string },
  policy: NonNullable<TaskCapture["policy"]>,
): string | undefined {
  const failures: string[] = [];
  if (checked.code !== 0) {
    const diagnostic = compactDiagnostic(checked.stderr) ||
      compactDiagnostic(checked.stdout) ||
      "no diagnostic output";
    failures.push(`cori check exited ${checked.code}: ${diagnostic}`);
  }
  if (!policy.ok) {
    const diagnostic = compactDiagnostic(
      policy.violations.join("; ") || "no policy diagnostic output",
    );
    failures.push(`workflow policy failed: ${diagnostic}`);
  }
  return failures.length > 0 ? failures.join("; ") : undefined;
}

function compactDiagnostic(value: string): string {
  return value.trim().replace(/\s+/gu, " ").slice(0, 1_000);
}

export function aggregateCaptures(
  tasks: readonly TaskCapture[],
): BenchmarkResultV1["capture"] {
  return {
    previewDidNotWrite: tasks.length > 0 &&
      tasks.every((capture) => capture.previewDidNotWrite),
    checkPassed: tasks.length > 0 && tasks.every(captureReady),
    policy: tasks.length === 1 ? tasks[0]!.policy : null,
    tasks,
  };
}

export async function prepareDirectWorkspace(
  workspace: string,
  taskId: string,
  scenario: Scenario,
): Promise<void> {
  await rm(workspace, { recursive: true, force: true });
  await mkdir(workspace, { recursive: true });
  await writeFile(
    join(workspace, "TASK.md"),
    renderedTaskPrompt(taskId, scenario, "direct"),
    "utf8",
  );
  await writeFile(join(workspace, "GWS.md"), gwsReference(), "utf8");
}

export async function prepareCaptureWorkspace(
  workspace: string,
  taskId: string,
  model: string,
): Promise<void> {
  await writeFile(
    join(workspace, "CORI_AUTHORING.md"),
    authoringReference(model, taskById(taskId).runtimeTrack, taskId),
    "utf8",
  );
  await cp(
    join(repoRoot, "skills", "cori_save_workflow"),
    join(workspace, ".agents", "skills", "cori_save_workflow"),
    { recursive: true },
  );
}

export function renderedTaskPrompt(
  taskId: string,
  scenario: Scenario,
  mode: "direct" | "capture",
): string {
  const task = taskById(taskId);
  return [
    `# ${task.name}`,
    "",
    task.prompt,
    "",
    "Complete the live Workspace task now and verify the requested external state. This is task execution, not workflow authoring: do not create a Cori workflow, manifest.md, steps/, or tests/, and do not read or invoke any workflow-capture skill.",
    "",
    "Before acting, read ./GWS.md from this workspace and follow it as part of the task contract.",
    "",
    "Use the installed `gws` CLI for every Workspace read and write. Do not use app connectors or browser automation.",
    "",
    "Use only the resources listed below. Tag every newly created Workspace resource and draft body with the exact run tag. Put the exact run tag in both the name/title and content of every newly created Drive file. Create drafts only; never send mail. All Calendar writes must pass sendUpdates=none.",
    "",
    `Run tag: ${scenario.runTag}`,
    "Parameters:",
    ...Object.entries(scenario.parameters).map(([name, value]) =>
      `- ${name}: ${value}`
    ),
    "Registered Workspace resources:",
    ...scenario.resources.map((resource) =>
      `- ${resource.role}: ${resource.id} (${resource.service})`
    ),
    "",
    `Mode: ${mode}. Do not read files outside this isolated workspace.`,
  ].join("\n");
}

function previewPrompt(): string {
  return "The direct task attempt is complete. Read and follow the benchmark-local skill at .agents/skills/cori_save_workflow/SKILL.md. Capture the procedure demonstrated in this conversation as a reusable Cori workflow, including corrections needed to satisfy the original task contract. Show only its directory tree and complete manifest.md. Do not create, edit, or run any workflow files yet.";
}

export function approvalPrompt(
  task: TaskSpec,
  priorCaptureError?: string,
): string {
  const parameterNames = task.parameters.map((parameter) => parameter.name)
    .join(", ");
  const retryContext = priorCaptureError
    ? ` This is a fresh retry after a previous independent capture failed. Correct this concrete failure and check for the same defect throughout the new workflow: ${compactDiagnostic(priorCaptureError)}`
    : "";
  return `Approved. First read ./CORI_AUTHORING.md from the current workspace again.${retryContext} Write the workflow to ./captured-workflow, then run \`"$CORI_BENCH_CORI" check ./captured-workflow\`. This variable names the exact Cori development executable selected by the benchmark. Do not substitute another cori from PATH and do not run cori run. Before finishing, inspect every CLI parse callback: its second argument may contain only stderr and exitCode, never workflow parameters or earlier outputs; derive parse outputs from stdout or return a fixed acknowledgement. The manifest must declare exactly these parameters and no others: ${parameterNames}. Fixed values from TASK.md must be constants in the workflow, not extra parameters. The reusable manifest and step files must not contain the current run tag or any registered resource ID as a default or hard-coded value; derive dynamic IDs from the declared task parameters or earlier step outputs. Captured values may appear only under tests/fixtures.`;
}

export function authoringReference(
  model: string,
  runtimeTrack: ReturnType<typeof taskById>["runtimeTrack"],
  taskId: string,
): string {
  return [
    "# Cori authoring constraints",
    "- Workflows are folders with manifest.md and numbered TypeScript files in steps/.",
    "- Every step imports `step` exactly from `@cori-do/sdk`; `@cori/sdk` is invalid.",
    "- Every GWS CLI step must use a literal argv[0] of gws and manifest tools_required: [gws].",
    "- GWS 0.22.5 accepts only documented flags such as --params, --json, --format, --output, --page-all, and --dry-run. Never invent convenience flags such as --allow-already-exists.",
    "- Google Sheets CellData.userEnteredValue must be an ExtendedValue object, never null. If a task truly requires clearing cells, use spreadsheets values clear; fresh benchmark fixtures should not be cleared defensively.",
    "- Use gws batch APIs for variable-size Sheets, Docs, and Slides writes; do not use v1 builtins.",
    "- CLI steps use argv arrays, never shell dispatchers; code steps are pure and cannot perform I/O.",
    "- A CLI parse function is `parse(stdout, { stderr, exitCode })`; it never receives workflow input. Derive output fields from the GWS JSON response (for example totalUpdatedRows) or use a fixed success value; never read `input.*` in parse.",
    "- Step expressions run in Deno: use web globals such as btoa, never Node globals such as Buffer.",
    "- Create Gmail drafts only. Calendar steps must use sendUpdates: none.",
    "- Reusability is mandatory: never hard-code or default the current run tag or registered Workspace resource IDs in manifest.md or steps. Derive dynamic IDs from declared task parameters or previous step outputs; real captured values belong only in tests/fixtures.",
    "- The manifest parameter names must exactly match the Parameters list in TASK.md. Do not invent extra required inputs; fixed task values belong in the workflow implementation.",
    "- Cori execution state begins with manifest parameters as top-level keys. Every required step input key must exactly match a parameter or a top-level key emitted by an earlier step.",
    "- Successful object outputs are shallow-merged into one flat state object; outputs are not nested by step name, and a duplicate top-level key overwrites the earlier value.",
    "- Keep multiple records and side-effect IDs in arrays or unique wrapper keys. Never reuse generic output keys such as message, id, or label_id when later steps need each value.",
    "- cori check statically parses and type-checks every workflow module without executing callbacks. It does not perform cross-step dataflow analysis; runtime Zod validation catches malformed consumption before the consuming side effect.",
    taskId === "support_inbox_triage"
      ? "- The support fixture always has three query results. Return their IDs in one message_ids array, keep all three fetched messages uniquely addressable (for example in a messages array or unique message wrappers), and keep every created category/priority label ID uniquely addressable by message and label purpose. Never reuse a shared message or label_id output key, and do not add fixture-specific message-ID parameters."
      : "",
    runtimeTrack === "deterministic"
      ? "- This task is deterministic: do not use step.llm; encode its rules in code steps."
      : model
      ? `- For hybrid extraction/classification use typed llm steps with model: ${model}.`
      : "- The benchmark runner will provide a required model for hybrid tasks.",
  ].join("\n");
}

function gwsReference(): string {
  return [
    "# GWS CLI",
    "gws <service> <resource> [sub-resource] <method> --params <JSON> --json <JSON> --format json",
    "Examples: gws sheets spreadsheets values get; gws gmail users drafts create; gws calendar events insert.",
    "The benchmark uses GWS 0.22.5. Never send Gmail messages or omit Calendar sendUpdates=none.",
  ].join("\n");
}

function parameterArgs(scenario: Scenario): readonly string[] {
  return Object.entries(scenario.parameters).map(([name, value]) =>
    `${name}=${JSON.stringify(value)}`
  );
}

function profileShape(
  profile: BenchmarkProfile,
): { trials: number; heldoutPairs: number } {
  if (profile === "smoke") return { trials: 1, heldoutPairs: 1 };
  if (profile === "full") return { trials: 1, heldoutPairs: 3 };
  return { trials: 3, heldoutPairs: 3 };
}

export function selectTasks(options: RunOptions) {
  const requested = options.taskIds?.map(taskById) ??
    (options.profile === "smoke" ? [TASKS[0]!] : TASKS);
  if (requested.length === 0) throw new Error("no tasks selected");
  if (!options.batch) return requested;
  if (options.taskIds) throw new Error("--batch and --task cannot be combined");
  const { index, count } = options.batch;
  if (
    !Number.isSafeInteger(index) || !Number.isSafeInteger(count) || count < 1 ||
    index < 1 || index > count
  ) {
    throw new Error(
      "batch must have the form INDEX/COUNT with 1 <= INDEX <= COUNT",
    );
  }
  const size = Math.ceil(requested.length / count);
  const selected = requested.slice((index - 1) * size, index * size);
  if (selected.length === 0) {
    throw new Error(`batch ${index}/${count} selects no tasks`);
  }
  return selected;
}

export function parseBatch(value: string | undefined): RunOptions["batch"] {
  if (!value) return undefined;
  const match = /^(\d+)\/(\d+)$/u.exec(value);
  if (!match) {
    throw new Error("--batch must have the form INDEX/COUNT, for example 1/5");
  }
  return { index: Number(match[1]), count: Number(match[2]) };
}

export function workspaceCoriBinary(): string {
  return join(
    workspaceCoriTargetRoot,
    "debug",
    process.platform === "win32" ? "cori.exe" : "cori",
  );
}

function coriBinary(): string {
  return process.env.CORI_BENCH_CORI ?? workspaceCoriBinary();
}

async function prepareCoriWorkflowCli(): Promise<void> {
  coriPreparation ??= (async () => {
    const override = process.env.CORI_BENCH_CORI?.trim();
    if (override) {
      coriBinarySource = "override";
    } else {
      const build = await runProcess(
        "cargo",
        [
          "build",
          "--package",
          "cori-cli",
          "--target-dir",
          workspaceCoriTargetRoot,
        ],
        repoRoot,
      );
      if (build.code !== 0) {
        throw new Error(
          `failed to build the workspace Cori development binary: ${
            build.stderr || build.stdout
          }`,
        );
      }
      process.env.CORI_BENCH_CORI = workspaceCoriBinary();
      coriBinarySource = "workspace_dev";
    }

    const binary = coriBinary();
    if (await exists(binary)) {
      coriBinarySha256 = await sha256(await readFile(binary));
      pinExecutableDirectoryOnPath(binary);
    }
  })();
  await coriPreparation;
}

function pinExecutableDirectoryOnPath(binary: string): void {
  const directory = dirname(resolve(binary));
  const current = process.env.PATH ?? "";
  const entries = current.split(delimiter).filter(Boolean);
  if (entries[0] === directory) return;
  process.env.PATH = [directory, ...entries.filter((entry) => entry !== directory)]
    .join(delimiter);
}

export function isCoriWorkflowCliHelp(value: string): boolean {
  return /<PATH>/u.test(value) && /--update\b/u.test(value) &&
    /--yes\b/u.test(value);
}

async function ensureCoriWorkflowCli(): Promise<void> {
  await prepareCoriWorkflowCli();
  const binary = coriBinary();
  const result = await runProcess(binary, ["check", "--help"]);
  const help = `${result.stdout}\n${result.stderr}`;
  if (result.code !== 0 || !isCoriWorkflowCliHelp(help)) {
    throw new Error(
      `${binary} is not the Cori workflow CLI expected by this benchmark; set CORI_BENCH_CORI to this repository's cori binary`,
    );
  }
}

async function runCori(args: readonly string[], cwd: string) {
  return runProcess(coriBinary(), args, cwd);
}

async function ensureExecutable(binary: string): Promise<void> {
  const result = await runProcess(binary, ["--version"]);
  if (result.code !== 0) {
    throw new Error(`${binary} is unavailable: ${result.stderr}`);
  }
}

async function version(binary: string): Promise<string> {
  const result = await runProcess(binary, ["--version"]);
  return result.code === 0
    ? result.stdout.trim()
    : `unavailable: ${result.stderr.trim()}`;
}

function providerForModel(model: string): "openai" | "anthropic" | "gemini" {
  if (/^(gpt-|o[1-9]|codex)/iu.test(model)) return "openai";
  if (/^claude/iu.test(model)) return "anthropic";
  if (/^gemini/iu.test(model)) return "gemini";
  throw new Error(
    `cannot infer the Cori LLM provider from model ${model}; use a gpt-, claude-, or gemini-prefixed model`,
  );
}

async function ensureCoriCapability(provider: string): Promise<void> {
  const status = await runProcess(coriBinary(), ["status"]);
  if (status.code !== 0) {
    throw new Error(
      `${coriBinary()} status failed: ${status.stderr || status.stdout}`,
    );
  }
  const capability = new RegExp(`^\\s*[✓✔]\\s+${provider}\\s+\\(LLM\\)`, "mu");
  if (!capability.test(status.stdout)) {
    throw new Error(
      `${coriBinary()} cannot access the ${provider} LLM credential; run \`${coriBinary()} login ${provider}\` and verify \`${coriBinary()} status\` before benchmarking`,
    );
  }
}

async function exists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function copyWorkflow(
  source: string,
  destination: string,
): Promise<void> {
  await rm(destination, { recursive: true, force: true });
  await cp(source, destination, { recursive: true });
}

function tokenSum(session: TrialResult["harness"]): number | null {
  if (
    !session || session.usage.inputTokens === null ||
    session.usage.outputTokens === null
  ) return null;
  return session.usage.inputTokens + session.usage.outputTokens;
}

function sumNullable(values: readonly (number | null)[]): number | null {
  const present = values.filter((value): value is number => value !== null);
  return present.length === 0
    ? null
    : present.reduce((sum, value) => sum + value, 0);
}

export function transcriptExecutedCoriRun(
  session: { transcript: readonly unknown[] },
): boolean {
  return session.transcript.some((event) => hasCoriRunCommand(event));
}

function hasCoriRunCommand(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(hasCoriRunCommand);
  if (!value || typeof value !== "object") return false;
  for (const [key, nested] of Object.entries(value)) {
    if (
      key === "command" && typeof nested === "string" &&
      /\bcori\s+run\b/iu.test(nested)
    ) return true;
    if (typeof nested !== "string" && hasCoriRunCommand(nested)) return true;
  }
  return false;
}

async function containsWorkflowManifest(directory: string): Promise<boolean> {
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name === "manifest.md") return true;
    if (
      entry.isDirectory() &&
      await containsWorkflowManifest(join(directory, entry.name))
    ) return true;
  }
  return false;
}

function hardGate(
  grade: TrialResult["grade"],
  traceOk: boolean,
  hashUnchanged: boolean,
): TrialResult["grade"] {
  const safetyViolations = [...grade.safetyViolations];
  if (!traceOk) {
    safetyViolations.push(replayTraceFailure);
  }
  if (!hashUnchanged) {
    safetyViolations.push(replayMutationFailure);
  }
  return safetyViolations.length === grade.safetyViolations.length
    ? grade
    : { ...grade, score: 0, passed: false, safetyViolations };
}

function recoverableLegacyScoreError(result: BenchmarkResultV1): boolean {
  return result.status === "failed" &&
    /^\d+ benchmark trial\(s\) failed external-state or execution grading$/u
      .test(result.error ?? "") &&
    result.trials.every((trial) =>
      trial.grade.safetyViolations.length === 0
    ) && result.trials
      .filter((trial) => trial.lane === "replay")
      .every((trial) => trial.grade.score === 100);
}

export function trialIntegrityError(
  trials: readonly TrialResult[],
): string | undefined {
  const failures = trials.flatMap((trial) => {
    const safetyFailures = trial.grade.safetyViolations.map((violation) =>
      `${trial.taskId} seed ${trial.seed} ${trial.lane}: ${violation}`
    );
    if (
      trial.lane !== "replay" || trial.grade.score === 100 ||
      safetyFailures.length > 0
    ) return safetyFailures;
    const incomplete = trial.grade.items
      .filter((item) => item.earned < item.max)
      .map((item) => item.id)
      .join(", ");
    return [
      `${trial.taskId} seed ${trial.seed} replay: scored ${trial.grade.score}/100; expected 100${
        incomplete ? ` (incomplete: ${incomplete})` : ""
      }`,
    ];
  });
  return failures.length === 0
    ? undefined
    : `${failures.length} benchmark Cori replay, safety, or replay-integrity failure(s):\n${
      failures.join("\n")
    }`;
}

function parseTrace(stdout: string): unknown {
  try {
    return JSON.parse(stdout) as unknown;
  } catch {
    return null;
  }
}

export function failedTraceDiagnostic(
  trace: unknown,
  process: { code: number; stdout: string; stderr: string },
): string {
  if (trace && typeof trace === "object" && !Array.isArray(trace)) {
    const error = (trace as { error?: unknown }).error;
    if (typeof error === "string" && error.trim()) {
      return compactDiagnostic(error);
    }
  }
  const stderr = compactDiagnostic(process.stderr);
  if (stderr) return stderr;
  const stdout = compactDiagnostic(process.stdout);
  if (stdout) return stdout;
  return `Cori exited ${process.code} without diagnostic output`;
}

function traceSucceeded(trace: unknown): boolean {
  return !!trace && typeof trace === "object" && !Array.isArray(trace) &&
    (trace as { status?: unknown }).status === "succeeded";
}

export function traceUsage(trace: unknown): TrialResult["runtime"] {
  if (!trace || typeof trace !== "object" || Array.isArray(trace)) {
    return {
      wallTimeMs: null,
      inputTokens: null,
      outputTokens: null,
      costEur: null,
    };
  }
  const record = trace as {
    duration_ms?: unknown;
    cost?: {
      input_tokens?: unknown;
      output_tokens?: unknown;
      total_eur?: unknown;
    };
  };
  return {
    wallTimeMs: typeof record.duration_ms === "number"
      ? record.duration_ms
      : null,
    inputTokens: typeof record.cost?.input_tokens === "number"
      ? record.cost.input_tokens
      : null,
    outputTokens: typeof record.cost?.output_tokens === "number"
      ? record.cost.output_tokens
      : null,
    costEur: typeof record.cost?.total_eur === "number"
      ? record.cost.total_eur
      : null,
  };
}

async function sha256(value: string | Uint8Array): Promise<string> {
  const { createHash } = await import("node:crypto");
  return createHash("sha256").update(value).digest("hex");
}

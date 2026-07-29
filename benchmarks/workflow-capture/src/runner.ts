import {
  access,
  chmod,
  copyFile,
  cp,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  artifactPath,
  normalizedCsv,
  readJson,
  resolveExistingRunDirectory,
  scorecard,
  validateRunId,
  writeJson,
} from "./artifacts.js";
import { gradeExternalState } from "./grader.js";
import {
  adapterFor,
  codexModel,
  executableFileIdentity,
  resolveExecutablePath,
} from "./harness.js";
import type {
  ExecutableFileIdentity,
  HarnessAdapter,
  HarnessExecutionOptions,
  HarnessIdentity,
  HarnessSandbox,
} from "./harness.js";
import {
  configuredBenchmarkCalendarId,
  GwsClient,
  requireBenchmarkCalendarId,
  runProcess,
  WorkspaceScenarioDriver,
} from "./gws.js";
import type { ProcessResult, ProcessRunner } from "./gws.js";
import { hashDirectory, inspectWorkflowPolicy } from "./policy.js";
import {
  assertHybridBanksAreRegexResistant,
  assertSeedsProduceDistinctFixtures,
  assertTwinEquivalent,
  buildScenario,
} from "./scenario.js";
import {
  breakEvenRepetitions,
  mean,
  MIN_PAIRED_TASKS,
  pairedDifferenceCi95,
  reuseAdvantage,
} from "./statistics.js";
import { assertTaskCatalog, taskById, TASKS } from "./tasks.js";
import { writeBenchmarkViewerForRun } from "./viewer.js";
import type {
  BenchmarkProfile,
  BenchmarkEnvironment,
  BenchmarkResultV2,
  Grade,
  HarnessName,
  HarnessSession,
  PhaseOutcome,
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
const replayTraceFailure =
  "Cori replay failed or did not emit a successful JSON trace";
const replayMutationFailure =
  "workflow directory changed during held-out replay";
let coriPreparation: Promise<void> | undefined;
let coriBinarySha256: string | null = null;
let coriBinaryVersion: string | null = null;
let coriBinarySource: "workspace_dev" | "override" = process.env
    .CORI_BENCH_CORI
  ? "override"
  : "workspace_dev";
let authorCoriIdentity: CoriIdentity | null = null;
let harnessIdentity: HarnessIdentity | null = null;
let gwsIdentity: (ExecutableFileIdentity & { version: string }) | null = null;
let temporalIdentity: (ExecutableFileIdentity & { version: string }) | null = null;
let denoIdentity: (ExecutableFileIdentity & { version: string }) | null = null;
let nodeIdentity: ExecutableFileIdentity | null = null;
let benchmarkSourceSha256: string | null = null;
let captureSkillSha256: string | null = null;
let subjectIsolationMechanism: string | null = null;
let subjectIsolationIdentity: ExecutableFileIdentity | null = null;
let workspaceAccountSha256: string | null = null;

export interface CoriIdentity {
  path: string;
  version: string;
  sha256: string;
}

export interface CoriExecutableProbe extends CoriIdentity {
  help: string;
}

export interface BenchmarkSubject {
  root: string;
  agentRoot: string;
  coriBinary: string;
}

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
  version: 2;
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
  gws: GwsClient;
  driver: WorkspaceScenarioDriver;
  adapter: HarnessAdapter;
  registry: CleanupRegistry;
  onProgress: (phase: string, detail: string) => Promise<void>;
}

export async function validate(): Promise<void> {
  assertTaskCatalog();
  // A fixture bank that one literal can separate would let a keyword matcher
  // stand in for the understanding a hybrid task is meant to measure.
  assertHybridBanksAreRegexResistant();
  for (const task of TASKS) {
    for (const seed of [42, 43, 44]) {
      buildScenario(task.id, seed, "author", "offline-validation");
      buildScenario(task.id, seed, "direct", "offline-validation");
      buildScenario(task.id, seed, "replay", "offline-validation");
    }
    // Held-out trials must pose different problems, not the same one relabelled.
    assertSeedsProduceDistinctFixtures(task.id, [42, 43, 44, 88, 89, 90, 91]);
    const reference = join(referencesRoot, task.id);
    const policy = await inspectWorkflowPolicy(
      reference,
      [],
      undefined,
      task.requiresRuntimeModel === true,
    );
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
      `CORI_BENCH_LLM_MODEL is required for the ${
        TASKS.filter((task) => task.requiresRuntimeModel).length
      } hybrid tasks`,
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
  const workspaceAccount = await gws.verifyAuthentication();
  await gws.canary(runTag);
  const report = {
    gwsVersion,
    schemaHash,
    workspaceAccountSha256: workspaceAccount,
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
): Promise<BenchmarkResultV2> {
  await validate();
  const tasks = selectTasks(options);
  if (options.profile === "publication" && options.harness !== "codex") {
    throw new Error(
      "publication inference currently requires the Codex harness because Claude and Gemini model/config identities are not pinned by their adapters",
    );
  }
  const batchSuffix = options.batch
    ? `-b${options.batch.index}of${options.batch.count}`
    : "";
  const runId = options.runId ??
    `workflow-capture-${
      new Date().toISOString().replace(/[:.]/gu, "-")
    }-${options.seed}${batchSuffix}`;
  validateRunId(runId);
  const runDir = join(options.artifactsRoot ?? defaultArtifactsRoot, runId);
  let agentRoot = join(runDir, "uninitialized-agent-workspace");
  let subject: BenchmarkSubject | undefined;
  await mkdir(runDir, { recursive: true });
  const calendarId = configuredBenchmarkCalendarId();
  const gws = new GwsClient();
  const driver = new WorkspaceScenarioDriver(gws, undefined, calendarId);
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
  const pairsPerTask = profilePairs(options.profile);
  let progress: BenchmarkProgress = {
    version: 2,
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
    plannedTrialsPerLane: tasks.length * pairsPerTask,
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
  const taskFailures: string[] = [];
  let globalError: string | undefined;
  authorCoriIdentity = null;
  harnessIdentity = null;
  gwsIdentity = null;
  temporalIdentity = null;
  denoIdentity = null;
  nodeIdentity = null;
  benchmarkSourceSha256 = null;
  captureSkillSha256 = null;
  subjectIsolationMechanism = null;
  subjectIsolationIdentity = null;
  workspaceAccountSha256 = null;

  try {
    if (
      tasks.some((task) => task.runtimeTrack === "hybrid") &&
      !process.env.CORI_BENCH_LLM_MODEL
    ) {
      throw new Error(
        "CORI_BENCH_LLM_MODEL is required when the selected benchmark tasks include the hybrid runtime track",
      );
    }
    if (
      options.profile !== "smoke" &&
      !configuredBenchmarkCalendarId()
    ) {
      throw new Error(
        "CORI_BENCH_CALENDAR_ID is required for full and publication benchmark runs",
      );
    }
    await publishProgress(
      "environment_check",
      process.env.CORI_BENCH_CORI
        ? "checking the explicitly selected Cori executable and harness capabilities"
        : "building the current workspace Cori development binary and checking harness capabilities",
    );
    await prepareCoriWorkflowCli();
    subject = await createBenchmarkSubject(coriBinary());
    agentRoot = subject.agentRoot;
    let harnessSandbox = await createHarnessSandbox(subject);
    if (!harnessSandbox && options.profile !== "smoke") {
      throw new Error(
        `${options.profile} benchmark runs require enforced subject isolation; install bwrap on Linux or run on macOS with /usr/bin/sandbox-exec`,
      );
    }
    if (harnessSandbox) {
      try {
        await auditHarnessSandbox(harnessSandbox, subject);
        subjectIsolationMechanism = harnessSandbox.mechanism;
        subjectIsolationIdentity = await executableFileIdentity(
          harnessSandbox.file,
        );
      } catch (error) {
        if (options.profile !== "smoke") throw error;
        harnessSandbox = null;
        subjectIsolationMechanism = "advisory_temp_workspace";
        subjectIsolationIdentity = null;
      }
    } else {
      subjectIsolationMechanism = "advisory_temp_workspace";
    }
    const harnessEnvironment = await createBenchmarkHarnessEnvironment(
      runDir,
      subject.coriBinary,
    );
    const adapter = adapterFor(
      options.harness,
      harnessEnvironment,
      harnessSandbox ?? undefined,
    );
    await collectInstrumentIdentities(adapter, gws, harnessEnvironment);
    await publishProgress(
      "environment_check",
      `using ${coriBinary()}${
        coriBinarySha256 ? ` (sha256 ${coriBinarySha256.slice(0, 12)})` : ""
      }`,
    );
    await ensureCoriWorkflowCli();
    authorCoriIdentity = await probeHarnessCoriEnvironment(
      harnessEnvironment,
      {
        ...selectedCoriIdentity(),
        path: subject.coriBinary,
      },
    );
    await publishProgress(
      "environment_check",
      `isolated author environment resolves ${authorCoriIdentity.path} (${authorCoriIdentity.version}, sha256 ${authorCoriIdentity.sha256.slice(0, 12)})`,
    );
    await publishProgress(
      "environment_check",
      "verifying Google Workspace OAuth credentials with a read-only API call",
    );
    workspaceAccountSha256 = await gws.verifyAuthentication();
    if (tasks.some((task) => task.runtimeTrack === "hybrid")) {
      await ensureCoriCapability(
        providerForModel(process.env.CORI_BENCH_LLM_MODEL ?? ""),
      );
    }
    if (tasks.some((task) => task.requiredServices.includes("calendar"))) {
      requireBenchmarkCalendarId();
      await driver.verifyCalendar();
    }
    for (const task of tasks) {
      await publishProgress(
        "author_direct",
        "running task author against the live fixture",
        task,
      );
      let captured: CapturedTaskWorkflow;
      try {
        captured = await captureWorkflowForTask({
          task,
          seed: options.seed,
          runId,
          runDir,
          agentRoot,
          gws,
          driver,
          adapter,
          registry,
          onProgress: (phase, detail) => publishProgress(phase, detail, task),
        });
      } catch (error) {
        const message = errorMessage(error);
        captured = failedTaskWorkflow(task, message, agentRoot);
      }
      if (!captureReady(captured.capture)) {
        captures.push(captured.capture);
        taskFailures.push(
          `${task.id}: ${
            captured.capture.error ?? "capture or check phase failed"
          }`,
        );
        completedTasks.push(task.id);
        await publishProgress(
          "task_skipped",
          `capture is not replayable; held-out pairs skipped: ${
            captured.capture.error ?? "capture or check phase failed"
          }`,
          task,
        );
        continue;
      }

      const replayTimer = startPhase();
      let replayFailure: string | undefined;
      let completedPairs = 0;
      for (let pair = 0; pair < pairsPerTask; pair += 1) {
        try {
          const scenarioSeed = options.seed + pair + 1;
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
          await publishProgress(
            "heldout_direct",
            `running direct/replay pair ${pair + 1}/${pairsPerTask}`,
            task,
          );
          const directWorkspace = join(
            agentRoot,
            `${task.id}-${pair}-direct`,
          );
          await prepareDirectWorkspace(
            directWorkspace,
            task.id,
            directScenario,
          );
          const beforeDirect = driver.baselineSnapshot(directScenario);
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${pair}-direct-before.json`,
            ),
            beforeDirect,
          );
          const directTranscriptPath = artifactPath(
            runDir,
            "transcripts",
            "direct",
            `${task.id}-${pair}.json`,
          );
          const direct = await adapter.start(
            renderedTaskPrompt(task.id, directScenario, "direct"),
            directWorkspace,
            harnessEvidenceOptions(
              directTranscriptPath,
              "heldout_direct",
              `direct agent pair ${pair + 1}/${pairsPerTask}`,
              (phase, detail) => publishProgress(phase, detail, task),
              phaseTimeoutMs("direct"),
            ),
          );
          await writeJson(directTranscriptPath, direct);
          const afterDirect = await driver.snapshot(
            directScenario,
            { settleTaggedOutputs: true },
          );
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${pair}-direct-after.json`,
            ),
            afterDirect,
          );
          const directGrade = hardGateHarnessTrial(
            gradeExternalState(
              directScenario,
              beforeDirect,
              afterDirect,
            ),
            direct,
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
            `direct pair ${pair + 1}/${pairsPerTask} scored ${directGrade.score}`,
            task,
          );
          if (directGrade.safetyViolations.length > 0) {
            replayFailure = `direct pair ${pair + 1} violated safety: ${
              directGrade.safetyViolations.join("; ")
            }`;
            break;
          }

          const replayScenario = await provision(
            driver,
            replayScenarioBase,
            registry,
            runDir,
          );
          await publishProgress(
            "heldout_replay",
            `running Cori replay pair ${pair + 1}/${pairsPerTask}`,
            task,
          );
          const beforeReplay = driver.baselineSnapshot(replayScenario);
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${pair}-replay-before.json`,
            ),
            beforeReplay,
          );
          const expectedHash = captured.capture.workflowHash!;
          const intactBefore = expectedHash ===
            await hashDirectory(captured.workflowDir!);
          const replay = await runCori([
            "run",
            captured.workflowDir!,
            "--json",
            ...parameterArgs(replayScenario),
          ], captured.authorWorkspace);
          const tracePath = artifactPath(
            runDir,
            "cori-traces",
            `${task.id}-${pair}.json`,
          );
          await writeJson(tracePath, replay);
          const afterReplay = await driver.snapshot(
            replayScenario,
            { settleTaggedOutputs: true },
          );
          await writeJson(
            artifactPath(
              runDir,
              "snapshots",
              `${task.id}-${pair}-replay-after.json`,
            ),
            afterReplay,
          );
          const trace = parseTrace(replay.stdout);
          const unchanged = intactBefore && expectedHash ===
            await hashDirectory(captured.workflowDir!);
          const replayGrade = hardGate(
            gradeExternalState(replayScenario, beforeReplay, afterReplay),
            replay.code === 0 && traceSucceeded(trace),
            unchanged,
            task.requiresRuntimeModel === true && !traceRanRuntimeModel(trace),
          );
          trials.push({
            taskId: task.id,
            seed: scenarioSeed,
            lane: "replay",
            grade: replayGrade,
            tracePath,
            workflowHash: expectedHash,
            runtime: replayRuntimeUsage(trace, replay.wallTimeMs),
          });
          completedPairs += 1;
          await publishProgress(
            "heldout_replay_complete",
            `replay pair ${pair + 1}/${pairsPerTask} scored ${replayGrade.score}`,
            task,
          );
          if (replayGrade.safetyViolations.length > 0) {
            replayFailure = `replay pair ${pair + 1} failed safety or workflow integrity: ${
              replayGrade.safetyViolations.join("; ")
            }`;
            break;
          }
        } catch (error) {
          replayFailure = errorMessage(error);
          break;
        }
      }
      captured.capture.outcomes.replay = finishPhase(
        replayTimer,
        replayFailure ? "failed" : "succeeded",
        replayFailure,
        { plannedPairs: pairsPerTask, completedPairs },
      );
      if (replayFailure) taskFailures.push(`${task.id}: ${replayFailure}`);
      captures.push(captured.capture);
      completedTasks.push(task.id);
      await publishProgress(
        "task_complete",
        replayFailure
          ? `stopped ${task.id} after ${completedPairs}/${pairsPerTask} pairs: ${replayFailure}`
          : `completed ${completedPairs}/${pairsPerTask} direct/replay pairs for ${task.id}`,
        task,
      );
    }
  } catch (error) {
    globalError = errorMessage(error);
  } finally {
    await writeJson(join(runDir, "cleanup-registry.json"), registry);
  }

  const integrityError = trialIntegrityError(trials);
  const failures = [
    ...(globalError ? [globalError] : []),
    ...taskFailures,
    ...(integrityError ? [integrityError] : []),
  ];
  const runError = failures.length > 0
    ? `${failures.length} benchmark phase failure(s):\n${failures.join("\n")}`
    : undefined;

  const result = summarize(
    runId,
    options,
    startedAt,
    aggregateCaptures(captures),
    trials,
    runError,
  );
  try {
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
  } finally {
    if (subject) {
      await rm(subject.root, { recursive: true, force: true }).catch(
        () => undefined,
      );
    }
  }
}

export async function cleanup(
  runId: string,
  artifactsRoot = defaultArtifactsRoot,
): Promise<void> {
  const runDir = await resolveExistingRunDirectory(artifactsRoot, runId);
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
    if (await exists(join(runDir, "result.json"))) {
      await writeBenchmarkViewerForRun(runDir);
    }
  }
  if (failures.length > 0) {
    throw new Error(`cleanup completed with failures:\n${failures.join("\n")}`);
  }
}

export async function report(
  runId: string,
  artifactsRoot = defaultArtifactsRoot,
): Promise<BenchmarkResultV2> {
  const runDir = await resolveExistingRunDirectory(artifactsRoot, runId);
  const existing = await readJson<BenchmarkResultV2>(
    join(runDir, "result.json"),
  );
  const trials = await Promise.all(existing.trials.map(async (trial) => {
    if (trial.lane !== "replay" || !trial.tracePath) return trial;
    try {
      const traceProcess = await readJson<{
        stdout: string;
        wallTimeMs?: number;
      }>(trial.tracePath);
      const trace = parseTrace(traceProcess.stdout);
      return {
        ...trial,
        runtime: replayRuntimeUsage(
          trace,
          typeof traceProcess.wallTimeMs === "number"
            ? traceProcess.wallTimeMs
            : traceUsage(trace).wallTimeMs,
        ),
      };
    } catch {
      return trial;
    }
  }));
  const currentTrialError = trialIntegrityError(trials);
  const runError = existing.error ?? currentTrialError;
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
    existing.environment,
    existing.summary.combinedResult === true,
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
): Promise<BenchmarkResultV2> {
  if (runIds.length < 2) {
    throw new Error("combine requires at least two batch run IDs");
  }
  for (const runId of runIds) validateRunId(runId, "source run ID");
  if (requestedRunId !== undefined) {
    validateRunId(requestedRunId, "combined run ID");
  }
  const sourceRunDirectories = await Promise.all(
    runIds.map((runId) =>
      resolveExistingRunDirectory(artifactsRoot, runId, "source run ID")
    ),
  );
  const sources = await Promise.all(
    sourceRunDirectories.map((runDir) =>
      readJson<BenchmarkResultV2>(join(runDir, "result.json"))
    ),
  );
  const first = sources[0]!;
  const missingCalendar = sources.filter((source) =>
    !source.environment.calendar_id
  );
  if (missingCalendar.length > 0) {
    throw new Error(
      `combined runs require CORI_BENCH_CALENDAR_ID evidence; missing from ${
        missingCalendar.map((source) => source.runId).join(", ")
      }`,
    );
  }
  const calendarIds = sources.map((source) =>
    source.environment.calendar_id!
  );
  if (new Set(calendarIds).size > 1) {
    throw new Error(
      "combined runs must use the same CORI_BENCH_CALENDAR_ID",
    );
  }
  for (const source of sources) {
    assertResultCoriIdentity(source);
    assertCompleteInstrumentIdentity(source);
    if (
      source.status !== "succeeded"
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
      JSON.stringify(stableInstrumentIdentity(source.environment)) !==
        JSON.stringify(stableInstrumentIdentity(first.environment))
    ) {
      throw new Error(
        "combined runs must use the same complete benchmark instrument identity",
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
  const expectedPerLane = profilePairs(first.profile);
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
  const trialDesignFailure = exactTrialDesignError(
    first.profile,
    first.seed,
    TASKS.map((task) => task.id),
    trials,
  );
  if (trialDesignFailure) {
    throw new Error(
      `combined runs have an invalid paired trial design: ${trialDesignFailure}`,
    );
  }
  const runId = requestedRunId ??
    `workflow-capture-combined-${
      new Date().toISOString().replace(/[:.]/gu, "-")
    }-${first.seed}`;
  validateRunId(runId, "combined run ID");
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
      first.environment,
      true,
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

export function assertResultCoriIdentity(
  result: Pick<BenchmarkResultV2, "runId" | "environment">,
): void {
  const environment = result.environment;
  const selected: CoriIdentity = {
    path: environment.cori ?? "",
    version: environment.cori_version ?? "",
    sha256: environment.cori_sha256 ?? "",
  };
  const author: CoriIdentity = {
    path: environment.author_cori_path ?? "",
    version: environment.author_cori_version ?? "",
    sha256: environment.author_cori_sha256 ?? "",
  };
  if (
    !selected.path || !selected.version || !selected.sha256 ||
    !author.path || !author.version || !author.sha256
  ) {
    throw new Error(
      `run ${result.runId} is missing selected or author-side Cori identity evidence`,
    );
  }
  if (
    selected.version !== author.version ||
    selected.sha256 !== author.sha256
  ) {
    throw new Error(
      `run ${result.runId} selected and author-side Cori identities do not match`,
    );
  }
}

export function assertCompleteInstrumentIdentity(
  result: Pick<BenchmarkResultV2, "runId" | "environment">,
): void {
  const identity = stableInstrumentIdentity(result.environment);
  const missing = Object.entries(identity)
    .filter(([, value]) => typeof value !== "string" || value.length === 0)
    .map(([key]) => key);
  if (missing.length > 0) {
    throw new Error(
      `run ${result.runId} is missing complete benchmark instrument identity: ${
        missing.join(", ")
      }`,
    );
  }
}

function stableInstrumentIdentity(
  environment: BenchmarkEnvironment,
): Record<string, string | null> {
  return {
    cori_version: environment.cori_version,
    cori_sha256: environment.cori_sha256,
    author_cori_version: environment.author_cori_version,
    author_cori_sha256: environment.author_cori_sha256,
    harness_version: environment.harness_version,
    harness_sha256: environment.harness_sha256,
    gws_version: environment.gws_version,
    gws_sha256: environment.gws_sha256,
    temporal_version: environment.temporal_version,
    temporal_sha256: environment.temporal_sha256,
    deno_version: environment.deno_version,
    deno_sha256: environment.deno_sha256,
    node_version: environment.node_version,
    node_sha256: environment.node_sha256,
    subject_isolation: environment.subject_isolation,
    subject_isolation_sha256: environment.subject_isolation_sha256,
    benchmark_source_sha256: environment.benchmark_source_sha256,
    capture_skill_sha256: environment.capture_skill_sha256,
    workspace_account_sha256: environment.workspace_account_sha256,
    author_model: environment.author_model,
    llm_model: environment.llm_model,
    os: environment.os,
    arch: environment.arch,
    timezone: environment.timezone,
  };
}

function summarize(
  runId: string,
  options: RunOptions,
  startedAt: string,
  capture: BenchmarkResultV2["capture"],
  trials: readonly TrialResult[],
  runError?: string,
  existingEnvironment?: BenchmarkEnvironment,
  combinedResult = false,
): BenchmarkResultV2 {
  const direct = trials.filter((trial) => trial.lane === "direct");
  const replay = trials.filter((trial) => trial.lane === "replay");
  const directScore = mean(direct.map((trial) => trial.grade.score));
  const replayScore = mean(replay.map((trial) => trial.grade.score));
  const scorePairs = pairedScores(trials);
  const taskScorePairs = pairedTaskScores(scorePairs);
  const inferenceEligible = inferentiallyEligible(options.profile, capture, trials, {
    isolationMechanism:
      existingEnvironment?.subject_isolation ?? subjectIsolationMechanism,
    harness: options.harness,
    authorModel:
      existingEnvironment?.author_model ?? authorModelIdentity(options.harness),
    combinedResult,
    seed: options.seed,
  });
  const paired = inferenceEligible
    ? pairedDifferenceCi95(
      taskScorePairs.map((pair) => pair.direct),
      taskScorePairs.map((pair) => pair.replay),
      options.seed,
    )
    : null;
  const directTime = completeMean(
    direct.map((trial) => trial.harness?.wallTimeMs ?? null),
  );
  const replayTime = completeMean(
    replay.map((trial) => trial.runtime?.wallTimeMs ?? null),
  );
  const designTokens = sumNullable(capture.tasks.flatMap((task) =>
    [task.outcomes.author, task.outcomes.capture].map((phase) =>
      phase.inputTokens !== null && phase.inputTokens !== undefined &&
        phase.outputTokens !== null && phase.outputTokens !== undefined
        ? phase.inputTokens + phase.outputTokens
        : null
    )
  ));
  const safetyViolations = trials.reduce(
    (sum, trial) => sum + trial.grade.safetyViolations.length,
    0,
  );
  const designWallTime = capture.tasks.length > 0
    ? capture.tasks.reduce((sum, task) =>
      sum + task.outcomes.author.wallTimeMs +
      task.outcomes.capture.wallTimeMs + task.outcomes.check.wallTimeMs, 0)
    : null;
  const selectedTaskIds = capture.tasks.map((task) => task.taskId);
  const directSuiteTime = suiteWallTime(
    trials,
    "direct",
    selectedTaskIds,
  );
  const replaySuiteTime = suiteWallTime(
    trials,
    "replay",
    selectedTaskIds,
  );
  const runtimeInputTokens = sumNullable(
    replay.map((trial) => trial.runtime?.inputTokens ?? null),
  );
  const runtimeOutputTokens = sumNullable(
    replay.map((trial) => trial.runtime?.outputTokens ?? null),
  );
  const result: BenchmarkResultV2 = {
    version: 2,
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
      cori_version: coriBinaryVersion,
      cori_sha256: coriBinarySha256,
      author_cori_path: authorCoriIdentity?.path ?? null,
      author_cori_version: authorCoriIdentity?.version ?? null,
      author_cori_sha256: authorCoriIdentity?.sha256 ?? null,
      harness_path: harnessIdentity?.path ?? null,
      harness_version: harnessIdentity?.version ?? null,
      harness_sha256: harnessIdentity?.sha256 ?? null,
      gws: process.env.GWS_BIN ?? "gws",
      gws_path: gwsIdentity?.path ?? null,
      gws_version: gwsIdentity?.version ?? null,
      gws_sha256: gwsIdentity?.sha256 ?? null,
      temporal_path: temporalIdentity?.path ?? null,
      temporal_version: temporalIdentity?.version ?? null,
      temporal_sha256: temporalIdentity?.sha256 ?? null,
      deno_path: denoIdentity?.path ?? null,
      deno_version: denoIdentity?.version ?? null,
      deno_sha256: denoIdentity?.sha256 ?? null,
      node_path: nodeIdentity?.path ?? process.execPath,
      node_version: process.version,
      node_sha256: nodeIdentity?.sha256 ?? null,
      subject_isolation: subjectIsolationMechanism,
      subject_isolation_path: subjectIsolationIdentity?.path ?? null,
      subject_isolation_sha256: subjectIsolationIdentity?.sha256 ?? null,
      benchmark_source_sha256: benchmarkSourceSha256,
      capture_skill_sha256: captureSkillSha256,
      workspace_account_sha256: workspaceAccountSha256,
      calendar_id: configuredBenchmarkCalendarId() ?? null,
      author_model: authorModelIdentity(options.harness),
      llm_model: process.env.CORI_BENCH_LLM_MODEL ?? null,
      os: process.platform,
      arch: process.arch,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    },
    capture,
    phaseTimingsMs: {
      author: sumPhaseTime(capture.tasks, "author"),
      capture: sumPhaseTime(capture.tasks, "capture"),
      check: sumPhaseTime(capture.tasks, "check"),
      replay: sumPhaseTime(capture.tasks, "replay"),
    },
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
      designWallTimeMs: designWallTime,
      directSuiteWallTimeMs: directSuiteTime,
      replaySuiteWallTimeMs: replaySuiteTime,
      breakEvenRepetitions:
        designWallTime !== null && directSuiteTime !== null &&
          replaySuiteTime !== null
          ? breakEvenRepetitions(
            designWallTime,
            directSuiteTime,
            replaySuiteTime,
          )
          : null,
    },
    summary: {
      directScore,
      replayScore,
      pairedSampleSize: taskScorePairs.length,
      combinedResult,
      inferenceEligible,
      pairedDifferenceCi95: paired,
      reuseAdvantageDemonstrated: reuseAdvantage(
        safetyViolations,
        paired,
        designWallTime,
        directSuiteTime,
        replaySuiteTime,
        taskScorePairs.length,
      ),
    },
    ...(runError ? { error: runError } : {}),
  };
  return result;
}

interface PairedScores {
  key: string;
  taskId: string;
  direct: number;
  replay: number;
}

function pairedScores(trials: readonly TrialResult[]): PairedScores[] {
  const direct = new Map<string, number>();
  const replay = new Map<string, number>();
  for (const trial of trials) {
    const key = `${trial.taskId}\u0000${trial.seed}`;
    (trial.lane === "direct" ? direct : replay).set(key, trial.grade.score);
  }
  return [...direct.keys()]
    .filter((key) => replay.has(key))
    .sort()
    .map((key) => ({
      key,
      taskId: key.split("\u0000", 1)[0]!,
      direct: direct.get(key)!,
      replay: replay.get(key)!,
    }));
}

interface PairedTaskScores {
  taskId: string;
  direct: number;
  replay: number;
}

/**
 * Collapse repeated seeds within each task before inference. Tasks—not the
 * three regenerated fixtures within a task—are the independent units in the
 * benchmark design.
 */
function pairedTaskScores(pairs: readonly PairedScores[]): PairedTaskScores[] {
  const grouped = new Map<string, { direct: number[]; replay: number[] }>();
  for (const pair of pairs) {
    const values = grouped.get(pair.taskId) ?? { direct: [], replay: [] };
    values.direct.push(pair.direct);
    values.replay.push(pair.replay);
    grouped.set(pair.taskId, values);
  }
  return [...grouped.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([taskId, values]) => ({
      taskId,
      direct: mean(values.direct)!,
      replay: mean(values.replay)!,
    }));
}

export function inferentiallyEligible(
  profile: BenchmarkProfile,
  capture: BenchmarkResultV2["capture"],
  trials: readonly TrialResult[],
  evidence: {
    isolationMechanism: string | null;
    harness: HarnessName;
    authorModel: string | null;
    combinedResult: boolean;
    seed: number;
  },
): boolean {
  const {
    isolationMechanism,
    harness,
    authorModel,
    combinedResult,
    seed,
  } = evidence;
  if (
    profile !== "publication" ||
    !combinedResult ||
    harness !== "codex" ||
    !authorModel?.trim() ||
    !isolationMechanism ||
    isolationMechanism === "advisory_temp_workspace"
  ) return false;
  const taskIds = capture.tasks.map((task) => task.taskId);
  if (
    taskIds.length !== TASKS.length ||
    new Set(taskIds).size !== TASKS.length ||
    TASKS.some((task) => !taskIds.includes(task.id)) ||
    !capture.tasks.every(captureReady) ||
    trials.some((trial) => trial.grade.safetyViolations.length > 0)
  ) return false;
  if (
    exactTrialDesignError(
      profile,
      seed,
      TASKS.map((task) => task.id),
      trials,
    )
  ) return false;
  return pairedTaskScores(pairedScores(trials)).length >= MIN_PAIRED_TASKS;
}

function exactTrialDesignError(
  profile: BenchmarkProfile,
  seed: number,
  taskIds: readonly string[],
  trials: readonly TrialResult[],
): string | undefined {
  const expectedSeeds = Array.from(
    { length: profilePairs(profile) },
    (_value, index) => seed + index + 1,
  );
  const expectedKeys = new Set(
    taskIds.flatMap((taskId) =>
      (["direct", "replay"] as const).flatMap((lane) =>
        expectedSeeds.map((trialSeed) =>
          trialDesignKey(taskId, lane, trialSeed)
        )
      )
    ),
  );
  const actualKeys = trials.map((trial) =>
    trialDesignKey(trial.taskId, trial.lane, trial.seed)
  );
  const seen = new Set<string>();
  const duplicates = actualKeys.filter((key) => {
    if (seen.has(key)) return true;
    seen.add(key);
    return false;
  });
  if (duplicates.length > 0) {
    return `duplicate task/lane/seed trials: ${
      [...new Set(duplicates)].join(", ")
    }`;
  }
  const unexpected = actualKeys.filter((key) => !expectedKeys.has(key));
  if (unexpected.length > 0) {
    return `unexpected task/lane/seed trials: ${
      [...new Set(unexpected)].join(", ")
    }`;
  }
  const missing = [...expectedKeys].filter((key) => !seen.has(key));
  if (missing.length > 0) {
    return `missing task/lane/seed trials: ${missing.join(", ")}`;
  }
  if (actualKeys.length !== expectedKeys.size) {
    return `found ${actualKeys.length} trials; expected ${expectedKeys.size}`;
  }
  return undefined;
}

function trialDesignKey(
  taskId: string,
  lane: TrialResult["lane"],
  seed: number,
): string {
  return `${taskId}/${lane}/${seed}`;
}

function suiteWallTime(
  trials: readonly TrialResult[],
  lane: TrialResult["lane"],
  taskIds: readonly string[],
): number | null {
  if (taskIds.length === 0) return null;
  let total = 0;
  for (const taskId of taskIds) {
    const selected = trials.filter((trial) =>
      trial.taskId === taskId && trial.lane === lane
    );
    const taskMean = completeMean(selected.map((trial) =>
      lane === "direct"
        ? trial.harness?.wallTimeMs ?? null
        : trial.runtime?.wallTimeMs ?? null
    ));
    if (taskMean === null) return null;
    total += taskMean;
  }
  return total;
}

function completeMean(values: readonly (number | null)[]): number | null {
  if (values.length === 0 || values.some((value) => value === null)) return null;
  return mean(values as readonly number[]);
}

function authorModelIdentity(harness: HarnessName): string | null {
  if (harness === "codex") return codexModel();
  return null;
}

async function writeArtifacts(
  runDir: string,
  result: BenchmarkResultV2,
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
  task,
  seed,
  runId,
  runDir,
  agentRoot,
  gws,
  driver,
  adapter,
  registry,
  onProgress,
}: CaptureWorkflowArgs): Promise<CapturedTaskWorkflow> {
  const artifactStem = task.id;
  const authorTimer = startPhase();
  const authorScenario = await provision(
    driver,
    buildScenario(task.id, seed, "author", runId),
    registry,
    runDir,
  );
  const authorWorkspace = join(agentRoot, "authors", artifactStem);
  await prepareDirectWorkspace(authorWorkspace, task.id, authorScenario);
  await onProgress(
    "author_direct",
    "agent is completing the single author fixture",
  );
  const beforeAuthor = driver.baselineSnapshot(authorScenario);
  await writeJson(
    artifactPath(runDir, "snapshots", "authors", `${artifactStem}-before.json`),
    beforeAuthor,
  );
  const authorTranscriptPath = artifactPath(
    runDir,
    "transcripts",
    "authors",
    `${artifactStem}-direct.json`,
  );
  const directAuthor = await adapter.start(
    renderedTaskPrompt(task.id, authorScenario, "direct"),
    authorWorkspace,
    harnessEvidenceOptions(
      authorTranscriptPath,
      "author_direct",
      "agent is working on the author fixture",
      onProgress,
      phaseTimeoutMs("author"),
    ),
  );
  await writeJson(authorTranscriptPath, directAuthor);
  const afterAuthor = await driver.snapshot(
    authorScenario,
    { settleTaggedOutputs: true },
  );
  await writeJson(
    artifactPath(runDir, "snapshots", "authors", `${artifactStem}-after.json`),
    afterAuthor,
  );
  const authorGrade = hardGateHarnessTrial(
    gradeExternalState(
      authorScenario,
      beforeAuthor,
      afterAuthor,
    ),
    directAuthor,
  );
  await writeJson(
    artifactPath(runDir, "author-grades", `${artifactStem}.json`),
    authorGrade,
  );
  await writeJson(
    artifactPath(runDir, "author-grades", `${task.id}.json`),
    authorGrade,
  );
  const authorFailure = authorGrade.safetyViolations.length > 0
    ? `author task violated benchmark safety: ${
      authorGrade.safetyViolations.join("; ")
    }`
    : directAuthor.timedOut
    ? "author harness timed out"
    : directAuthor.exitCode !== 0
    ? `author harness exited ${directAuthor.exitCode}`
    : !authorGrade.passed
    ? `author task scored ${authorGrade.score}; at least 90 is required before capture`
    : undefined;
  const authorOutcome = finishPhase(
    authorTimer,
    authorFailure ? "failed" : "succeeded",
    authorFailure,
    sessionUsage(directAuthor),
  );
  if (authorFailure) {
    const captureOutcome = skippedOutcome("author task did not pass");
    const checkOutcome = skippedOutcome("author task did not pass");
    const replayOutcome = skippedOutcome("author task did not pass");
    return {
      capture: {
        taskId: task.id,
        authorGrade,
        outcomes: {
          author: authorOutcome,
          capture: captureOutcome,
          check: checkOutcome,
          replay: replayOutcome,
        },
        previewPresented: false,
        previewDidNotWrite: false,
        skillCheckObserved: false,
        skillCheckSucceeded: false,
        benchmarkCheckSucceeded: false,
        runtimeModelDataflowVerified: null,
        checkPassed: false,
        policy: null,
        workflowHash: null,
        workflowPath: null,
        error: authorFailure,
      },
      workflowDir: null,
      authorWorkspace,
    };
  }

  await prepareCaptureWorkspace(authorWorkspace);
  const workflowDir = join(authorWorkspace, "captured-workflow");
  const persistedWorkflowDir = artifactPath(
    runDir,
    "generated-workflows",
    task.id,
  );
  const captureTimer = startPhase();
  const absentBeforePreview = !(await containsWorkflowManifest(authorWorkspace)) &&
    !(await containsFiles(workflowDir));
  const workspaceHashBeforePreview = await hashDirectory(authorWorkspace);
  const gwsAuditBeforeCapture = gws.auditEvidence();
  let previewDidNotWrite = false;
  let previewPresented = false;
  let captureDidNotMutateWorkspace = false;
  let preview: HarnessSession | undefined;
  let approval: HarnessSession | undefined;
  if (directAuthor.sessionId) {
    const previewTranscriptPath = artifactPath(
      runDir,
      "transcripts",
      "authors",
      `${artifactStem}-capture-preview.json`,
    );
    const approvalTranscriptPath = artifactPath(
      runDir,
      "transcripts",
      "authors",
      `${artifactStem}-capture-approval.json`,
    );
    await onProgress("capture_preview", "requesting the skill's read-only tree and manifest preview");
    ({ preview, approval } = await captureConversationTurns(
      adapter,
      directAuthor.sessionId,
      authorWorkspace,
      {
        preview: harnessEvidenceOptions(
          previewTranscriptPath,
          "capture_preview",
          "skill is preparing the preview",
          onProgress,
          phaseTimeoutMs("capture_preview"),
        ),
        approval: harnessEvidenceOptions(
          approvalTranscriptPath,
          "capture_approval",
          "skill is writing and checking the approved workflow",
          onProgress,
          phaseTimeoutMs("capture_approval"),
        ),
        afterPreview: async (session) => {
          await writeJson(previewTranscriptPath, session);
          previewPresented = transcriptHasWorkflowPreview(session);
          previewDidNotWrite = previewHadNoSideEffects({
            cleanStart: absentBeforePreview,
            workspaceHashBefore: workspaceHashBeforePreview,
            workspaceHashAfter: await hashDirectory(authorWorkspace),
            auditBefore: gwsAuditBeforeCapture,
            auditAfter: gws.auditEvidence(),
            session,
          });
          await onProgress(
            "capture_approval",
            previewDidNotWrite
              ? "preview recorded without writes; replying yes"
              : "preview gate failed; recording the required yes turn without repairing",
          );
        },
      },
    ));
    await writeJson(approvalTranscriptPath, approval);
    captureDidNotMutateWorkspace =
      captureAuditHasNoMutations(
        gwsAuditBeforeCapture,
        gws.auditEvidence(),
      ) &&
      transcriptWorkspaceToolViolations(approval).length === 0;
  }

  const workflowExists = await exists(workflowDir);
  const captureFailures = [
    ...(!directAuthor.sessionId ? ["author harness did not return a resumable session"] : []),
    ...(!previewPresented ? ["skill did not present a complete workflow tree and manifest preview"] : []),
    ...(!previewDidNotWrite
      ? [
        "preview gate detected local or Workspace side effects, or Cori execution, before approval",
      ]
      : []),
    ...(!approval ? ["approval turn was not recorded"] : []),
    ...(approval && approval.exitCode !== 0
      ? [`approval harness exited ${approval.exitCode}`]
      : []),
    ...(approval && !captureDidNotMutateWorkspace
      ? [
        "capture turns executed or obscured mutating Google Workspace commands",
      ]
      : []),
    ...(!workflowExists ? ["approved skill session did not create captured-workflow"] : []),
  ];
  const captureOutcome = finishPhase(
    captureTimer,
    captureFailures.length > 0 ? "failed" : "succeeded",
    captureFailures.join("; ") || undefined,
    combinedSessionUsage([preview, approval]),
  );

  let policy: TaskCapture["policy"] = null;
  let checkPassed = false;
  const skillCheckObserved = approval
    ? transcriptExecutedCoriCheck(approval)
    : false;
  const skillCheckSucceeded = approval
    ? transcriptSuccessfulCoriCheck(approval)
    : false;
  let benchmarkCheckSucceeded = false;
  let runtimeModelDataflowVerified: boolean | null =
    task.requiresRuntimeModel === true ? false : null;
  const checkTimer = startPhase();
  let checkError: string | undefined = !skillCheckObserved
    ? "skill approval turn did not run cori check"
    : !skillCheckSucceeded
    ? "skill approval turn attempted cori check but did not complete with `Result: ✓ ready`"
    : undefined;
  if (workflowExists) {
    policy = await inspectWorkflowPolicy(
      workflowDir,
      [
        authorScenario.runTag,
        ...authorScenario.resources.map((resource) => resource.id),
      ],
      task.parameters.map((parameter) => parameter.name),
      task.requiresRuntimeModel === true,
    );
    await writeJson(
      artifactPath(runDir, "policy", `${task.id}.json`),
      policy,
    );
    await writeJson(
      artifactPath(runDir, "workflow-hashes", `${task.id}.json`),
      { captured: policy.workflowHash },
    );
    await copyWorkflow(
      workflowDir,
      persistedWorkflowDir,
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
    benchmarkCheckSucceeded = checked.code === 0 &&
      isCanonicalCoriReadyOutput(`${checked.stdout}\n${checked.stderr}`);
    if (task.requiresRuntimeModel === true) {
      runtimeModelDataflowVerified = await workflowHasRuntimeModelDataflow(
        workflowDir,
      );
    }
    checkPassed = policy.ok && skillCheckSucceeded &&
      benchmarkCheckSucceeded &&
      runtimeModelDataflowVerified !== false;
    checkError = [
      checkError,
      formatWorkflowCheckFailure(checked, policy),
      runtimeModelDataflowVerified === false
        ? "hybrid workflow has no static LLM-result dataflow to a later CLI or MCP side effect"
        : undefined,
    ].filter(Boolean).join("; ") || undefined;
  } else {
    checkError = [
      checkError,
      "approved skill session did not create captured-workflow",
    ].filter(Boolean).join("; ");
  }
  const checkOutcome = finishPhase(
    checkTimer,
    checkPassed ? "succeeded" : "failed",
    checkError,
  );

  const errorParts = [captureOutcome.error, checkOutcome.error]
    .flatMap((value) => value?.split("; ") ?? [])
    .map((value) => value.trim())
    .filter(Boolean);
  const error = [...new Set(errorParts)].join("; ") || undefined;

  return {
    capture: {
      taskId: task.id,
      authorGrade,
      outcomes: {
        author: authorOutcome,
        capture: captureOutcome,
        check: checkOutcome,
        replay: skippedOutcome(
          checkPassed && previewDidNotWrite && previewPresented
            ? "held-out replay not started"
            : "capture or check failed",
        ),
      },
      previewPresented,
      previewDidNotWrite,
      skillCheckObserved,
      skillCheckSucceeded,
      benchmarkCheckSucceeded,
      runtimeModelDataflowVerified,
      checkPassed,
      policy,
      workflowHash: policy?.workflowHash ?? null,
      workflowPath: workflowExists ? persistedWorkflowDir : null,
      ...(error ? { error } : {}),
    },
    workflowDir: checkPassed && previewPresented && previewDidNotWrite
      ? workflowDir
      : null,
    authorWorkspace,
  };
}

export function captureReady(capture: TaskCapture): boolean {
  const requiresRuntimeModel =
    taskById(capture.taskId).requiresRuntimeModel === true;
  return capture.authorGrade.passed &&
    capture.authorGrade.safetyViolations.length === 0 &&
    capture.outcomes.author.status === "succeeded" &&
    capture.previewPresented && capture.previewDidNotWrite &&
    capture.skillCheckObserved && capture.skillCheckSucceeded &&
    capture.benchmarkCheckSucceeded && capture.checkPassed &&
    (!requiresRuntimeModel || capture.runtimeModelDataflowVerified === true) &&
    capture.outcomes.capture.status === "succeeded" &&
    capture.outcomes.check.status === "succeeded" &&
    capture.policy?.ok === true && capture.workflowHash !== null;
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
  } else if (
    !isCanonicalCoriReadyOutput(`${checked.stdout}\n${checked.stderr}`)
  ) {
    failures.push(
      "benchmark absolute-binary cori check did not report `Result: ✓ ready`",
    );
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
): BenchmarkResultV2["capture"] {
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
): Promise<void> {
  await cp(
    join(repoRoot, "skills", "cori-save-workflow"),
    join(workspace, ".agents", "skills", "cori-save-workflow"),
    { recursive: true },
  );
}

interface StaticWorkflowStep {
  kind: "cli" | "mcp" | "code" | "llm";
  inputKeys: ReadonlySet<string>;
  outputKeys: ReadonlySet<string>;
  callbackKeys: ReadonlySet<string>;
}

/**
 * Prove a minimal static lineage for hybrid workflows. A model output must be
 * destructured by executable step code and, directly or through code-step
 * outputs, reach a later CLI/MCP side effect. Merely adding an unused LLM step
 * therefore cannot satisfy the benchmark's runtime-understanding gate.
 */
export async function workflowHasRuntimeModelDataflow(
  workflowDir: string,
): Promise<boolean> {
  const stepsDir = join(workflowDir, "steps");
  const entries = (await readdir(stepsDir))
    .filter((entry) => entry.endsWith(".ts"))
    .sort();
  const steps = await Promise.all(entries.map(async (entry) =>
    parseStaticWorkflowStep(await readFile(join(stepsDir, entry), "utf8"))
  ));
  for (let modelIndex = 0; modelIndex < steps.length; modelIndex += 1) {
    const model = steps[modelIndex];
    if (!model || model.kind !== "llm" || model.outputKeys.size === 0) continue;
    const tainted = new Set(model.outputKeys);
    for (let index = modelIndex + 1; index < steps.length; index += 1) {
      const step = steps[index];
      if (!step) continue;
      const consumesModelOutput = [...tainted].some((key) =>
        step.inputKeys.has(key) && step.callbackKeys.has(key)
      );
      if (!consumesModelOutput) continue;
      if (step.kind === "cli" || step.kind === "mcp") return true;
      for (const key of step.outputKeys) tainted.add(key);
    }
  }
  return false;
}

function parseStaticWorkflowStep(source: string): StaticWorkflowStep {
  const kindMatch = /step\.(cli|mcpTool|mcp_tool|code|llm)\s*\(/u.exec(source);
  const rawKind = kindMatch?.[1];
  const kind: StaticWorkflowStep["kind"] = rawKind === "mcpTool" ||
      rawKind === "mcp_tool"
    ? "mcp"
    : rawKind === "cli" || rawKind === "code" || rawKind === "llm"
    ? rawKind
    : "code";
  const callbackKeys = new Set<string>();
  const callbackPattern =
    /\b(?:command|run|prompt|request)\s*:\s*\(\s*\{([\s\S]*?)\}\s*\)/gu;
  for (const match of source.matchAll(callbackPattern)) {
    for (const key of destructuredKeys(match[1] ?? "")) {
      callbackKeys.add(key);
    }
  }
  return {
    kind,
    inputKeys: zodObjectKeys(source, "Input"),
    outputKeys: zodObjectKeys(source, "Output"),
    callbackKeys,
  };
}

function destructuredKeys(value: string): string[] {
  return value.split(",").flatMap((part) => {
    const match = /^\s*([A-Za-z_$][\w$]*)/u.exec(part);
    return match?.[1] ? [match[1]] : [];
  });
}

function zodObjectKeys(source: string, variable: "Input" | "Output"): Set<string> {
  const marker = new RegExp(
    `\\b(?:const|let)\\s+${variable}\\s*=\\s*z\\.object\\s*\\(`,
    "u",
  ).exec(source);
  if (!marker) return new Set();
  const opening = source.indexOf("{", marker.index + marker[0].length);
  if (opening < 0) return new Set();
  return topLevelObjectKeys(source, opening);
}

function topLevelObjectKeys(source: string, opening: number): Set<string> {
  const keys = new Set<string>();
  let depth = 1;
  let index = opening + 1;
  let quote: string | null = null;
  let escaped = false;
  while (index < source.length && depth > 0) {
    const character = source[index]!;
    const next = source[index + 1];
    if (quote) {
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === quote) quote = null;
      index += 1;
      continue;
    }
    if (character === "'" || character === '"' || character === "`") {
      quote = character;
      index += 1;
      continue;
    }
    if (character === "/" && next === "/") {
      index = source.indexOf("\n", index + 2);
      if (index < 0) break;
      continue;
    }
    if (character === "/" && next === "*") {
      const end = source.indexOf("*/", index + 2);
      index = end < 0 ? source.length : end + 2;
      continue;
    }
    if (character === "{") {
      depth += 1;
      index += 1;
      continue;
    }
    if (character === "}") {
      depth -= 1;
      index += 1;
      continue;
    }
    if (depth === 1 && /[A-Za-z_$]/u.test(character)) {
      const match = /^[A-Za-z_$][\w$]*/u.exec(source.slice(index));
      const key = match?.[0];
      if (key) {
        let cursor = index + key.length;
        while (/\s/u.test(source[cursor] ?? "")) cursor += 1;
        if (source[cursor] === ":") keys.add(key);
        index = cursor + 1;
        continue;
      }
    }
    index += 1;
  }
  return keys;
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
    task.rerunContract
      ? "This task runs repeatedly against the same resources. Some of the state you find was left by an earlier run; honour the re-run rules stated above so that running twice is safe."
      : "The registered resources are freshly provisioned for this scenario and its run tag is unique. Do not add stale-state cleanup or cross-run already-exists guards unless this task explicitly requires one.",
    "",
    ...(task.requiresRuntimeModel
      ? [
        // The source data is regenerated every run, so the workflow this
        // session captures has to keep working on text it has never seen.
        "The source content differs every time this job runs: wording, volume, language, layout, and values are all new each day. Solve the case in front of you, but assume the next run will look different.",
        `An LLM model class is available to workflows in this environment: ${modelForHybridTasks()}.`,
        "",
      ]
      : []),
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

export function modelForHybridTasks(): string {
  return process.env.CORI_BENCH_LLM_MODEL ?? "";
}

export function captureRequestPrompt(): string {
  return "Save this as a Cori workflow under ./captured-workflow.";
}

export const missingRuntimeModelFailure =
  "the captured workflow produced no successful non-empty llm result, but this task's source data is regenerated every run";

/**
 * A task on the hybrid track claims its answers cannot be derived from
 * literals. If a replay produces no `llm` activity, either the claim or the
 * fixture is wrong, and the run must say so rather than bank the score.
 */
export function traceRanRuntimeModel(trace: unknown): boolean {
  if (!trace || typeof trace !== "object" || Array.isArray(trace)) return false;
  const activities = (trace as { activities?: unknown }).activities;
  if (!Array.isArray(activities)) return false;
  return activities.some((activity) =>
    activity && typeof activity === "object" && !Array.isArray(activity) &&
    (activity as { kind?: unknown }).kind === "llm" &&
    ["ok", "succeeded"].includes(
      String((activity as { status?: unknown }).status),
    ) &&
    hasNonEmptyModelOutput((activity as { output?: unknown }).output)
  );
}

function hasNonEmptyModelOutput(output: unknown): boolean {
  if (Array.isArray(output)) return output.length > 0;
  if (output && typeof output === "object") {
    return Object.keys(output as Record<string, unknown>).length > 0;
  }
  return typeof output === "string" ? output.trim().length > 0 : output != null;
}

export function approvalPrompt(): string {
  return "yes";
}

export async function captureConversationTurns(
  adapter: HarnessAdapter,
  authorSessionId: string,
  cwd: string,
  options: {
    preview?: HarnessExecutionOptions;
    approval?: HarnessExecutionOptions;
    afterPreview?: (preview: HarnessSession) => void | Promise<void>;
  } = {},
): Promise<{ preview: HarnessSession; approval: HarnessSession }> {
  const preview = await adapter.resume(
    authorSessionId,
    captureRequestPrompt(),
    cwd,
    options.preview,
  );
  await options.afterPreview?.(preview);
  const approval = await adapter.resume(
    preview.sessionId ?? authorSessionId,
    approvalPrompt(),
    cwd,
    options.approval,
  );
  return { preview, approval };
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

export function profilePairs(profile: BenchmarkProfile): number {
  return profile === "publication" ? 3 : 1;
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
  if (count > requested.length) {
    throw new Error(
      `batch count ${count} exceeds the ${requested.length} selected tasks`,
    );
  }
  const baseSize = Math.floor(requested.length / count);
  const remainder = requested.length % count;
  const zeroBasedIndex = index - 1;
  const offset = zeroBasedIndex * baseSize +
    Math.min(zeroBasedIndex, remainder);
  const size = baseSize + (zeroBasedIndex < remainder ? 1 : 0);
  return requested.slice(offset, offset + size);
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
  return resolve(process.env.CORI_BENCH_CORI ?? workspaceCoriBinary());
}

async function prepareCoriWorkflowCli(): Promise<void> {
  coriPreparation ??= (async () => {
    const override = process.env.CORI_BENCH_CORI?.trim();
    if (override) {
      process.env.CORI_BENCH_CORI = resolve(override);
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
    if (!(await exists(binary))) {
      throw new Error(`selected Cori executable does not exist: ${binary}`);
    }
    const versionResult = await runProcess(binary, ["--version"]);
    if (versionResult.code !== 0 || !versionResult.stdout.trim()) {
      throw new Error(
        `${binary} --version failed: ${
          versionResult.stderr || versionResult.stdout || "no output"
        }`,
      );
    }
    coriBinaryVersion = versionResult.stdout.trim();
    coriBinarySha256 = await sha256(await readFile(binary));
    pinExecutableDirectoryOnPath(binary);
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

export async function createBenchmarkHarnessEnvironment(
  _runDir: string,
  binary: string,
  source: NodeJS.ProcessEnv = process.env,
): Promise<NodeJS.ProcessEnv> {
  const selected = resolve(binary);
  const environment: NodeJS.ProcessEnv = {
    ...source,
    CORI_BENCH_CORI: selected,
  };
  const directory = dirname(selected);
  const entries = (source.PATH ?? "").split(delimiter).filter((entry) => {
    if (!entry) return false;
    const absolute = resolve(entry);
    return absolute !== directory && !pathWithin(absolute, repoRoot);
  });
  environment.PATH = [
    directory,
    ...entries,
  ].join(delimiter);
  for (
    const variable of [
      "PWD",
      "OLDPWD",
      "INIT_CWD",
      "NODE_PATH",
      "CARGO_MANIFEST_DIR",
      "npm_config_local_prefix",
      "npm_package_json",
    ]
  ) {
    delete environment[variable];
  }
  delete environment.ZDOTDIR;
  delete environment.CORI_BENCH_ZSH;
  return environment;
}

/**
 * Stage the measured subject outside the checkout. The direct agent sees only
 * task-local artifacts, plus a byte-identical Cori executable copied into the
 * subject's bin directory.
 */
export async function createBenchmarkSubject(
  binary: string,
): Promise<BenchmarkSubject> {
  const source = resolve(binary);
  const metadata = await stat(source);
  if (!metadata.isFile()) {
    throw new Error(`selected Cori executable is not a file: ${source}`);
  }
  const root = await mkdtemp(join(tmpdir(), "cori-benchmark-subject-"));
  const binRoot = join(root, "bin");
  const agentRoot = join(root, "workspaces");
  await mkdir(binRoot, { recursive: true });
  await mkdir(agentRoot, { recursive: true });
  const coriName = process.platform === "win32" ? "cori.exe" : "cori";
  const staged = join(binRoot, coriName);
  await copyFile(source, staged);
  await chmod(staged, metadata.mode);
  return { root, agentRoot, coriBinary: staged };
}

/**
 * Build an OS-enforced read boundary around measured harness subprocesses.
 * Network, user credentials, and installed tools remain available; the Cori
 * checkout (and therefore benchmark answers/reference workflows) does not.
 */
export async function createHarnessSandbox(
  subject: BenchmarkSubject,
): Promise<HarnessSandbox | null> {
  if (process.platform === "darwin") {
    const executable = "/usr/bin/sandbox-exec";
    if (!(await exists(executable))) return null;
    const profile = join(subject.root, ".subject.sb");
    await writeFile(
      profile,
      [
        "(version 1)",
        "(allow default)",
        `(deny file-read* (subpath ${sandboxLiteral(repoRoot)}))`,
        `(deny file-read* (literal ${sandboxLiteral(profile)}))`,
        "",
      ].join("\n"),
      "utf8",
    );
    return {
      file: executable,
      args: ["-f", profile],
      mechanism: "macos_sandbox_exec_repo_read_deny",
    };
  }
  if (process.platform === "linux") {
    try {
      const executable = await resolveExecutablePath("bwrap");
      return {
        file: executable,
        args: [
          "--dev-bind",
          "/",
          "/",
          "--tmpfs",
          repoRoot,
          "--share-net",
          "--",
        ],
        mechanism: "linux_bwrap_repo_mask",
      };
    } catch {
      return null;
    }
  }
  return null;
}

export async function auditHarnessSandbox(
  sandbox: HarnessSandbox,
  subject: BenchmarkSubject,
): Promise<void> {
  const allowed = join(subject.agentRoot, "isolation-canary.txt");
  const denied = join(repoRoot, "package.json");
  await writeFile(allowed, "allowed\n", "utf8");
  const script = [
    'const fs = require("node:fs");',
    "let allowed = false;",
    "let denied = false;",
    "try { allowed = fs.readFileSync(process.argv[1], \"utf8\").includes(\"allowed\"); } catch {}",
    "try { fs.readFileSync(process.argv[2], \"utf8\"); } catch { denied = true; }",
    "process.stdout.write(JSON.stringify({ allowed, denied }));",
  ].join("");
  const audit = await runProcess(
    sandbox.file,
    [...sandbox.args, process.execPath, "-e", script, allowed, denied],
    subject.agentRoot,
  );
  let observed: { allowed?: unknown; denied?: unknown } = {};
  try {
    observed = JSON.parse(audit.stdout) as typeof observed;
  } catch {
    // The diagnostic below includes the raw process output.
  }
  if (
    audit.code !== 0 || observed.allowed !== true || observed.denied !== true
  ) {
    throw new Error(
      `subject isolation audit failed for ${sandbox.mechanism}: ${
        compactDiagnostic(`${audit.stdout}\n${audit.stderr}`) ||
        `exit ${audit.code}`
      }`,
    );
  }
}

function sandboxLiteral(value: string): string {
  return JSON.stringify(resolve(value));
}

function pathWithin(path: string, parent: string): boolean {
  const absolute = resolve(path);
  const root = resolve(parent);
  return absolute === root ||
    absolute.startsWith(`${root}${process.platform === "win32" ? "\\" : "/"}`);
}

export function isCoriWorkflowCliHelp(value: string): boolean {
  return /<PATH>/u.test(value) && /--update\b/u.test(value) &&
    /--yes\b/u.test(value);
}

export function isCanonicalCoriReadyOutput(value: string): boolean {
  return /(?:^|\n)Result:\s*✓ ready(?:\n|$)/u.test(value);
}

export function validateCoriExecutableProbe(
  selected: CoriIdentity,
  probe: CoriExecutableProbe,
): void {
  if (resolve(probe.path) !== resolve(selected.path)) {
    throw new Error(
      `author Cori path mismatch: expected ${selected.path}, found ${probe.path}`,
    );
  }
  if (!isCoriWorkflowCliHelp(probe.help)) {
    throw new Error(
      "author Cori help mismatch: `cori check --help` is not the expected workflow CLI surface",
    );
  }
  if (probe.version !== selected.version) {
    throw new Error(
      `author Cori version mismatch: expected ${selected.version}, found ${probe.version}`,
    );
  }
  if (probe.sha256 !== selected.sha256) {
    throw new Error(
      `author Cori digest mismatch: expected ${selected.sha256}, found ${probe.sha256}`,
    );
  }
}

export async function probeHarnessCoriEnvironment(
  environment: NodeJS.ProcessEnv,
  selected: CoriIdentity,
  runner: ProcessRunner = runProcess,
): Promise<CoriIdentity> {
  const observedPath = await resolveExecutablePath("cori", environment);
  const helpResult = await runner(
    observedPath,
    ["check", "--help"],
    dirname(observedPath),
    environment,
  );
  const help = successfulProbeOutput("cori check --help", helpResult, false);
  const versionResult = await runner(
    observedPath,
    ["--version"],
    dirname(observedPath),
    environment,
  );
  const observed: CoriExecutableProbe = {
    path: resolve(observedPath),
    help,
    version: successfulProbeOutput("cori --version", versionResult),
    sha256: await sha256(await readFile(resolve(observedPath))),
  };
  validateCoriExecutableProbe(selected, observed);
  return {
    path: observed.path,
    version: observed.version,
    sha256: observed.sha256,
  };
}

function successfulProbeOutput(
  command: string,
  result: ProcessResult,
  trim = true,
): string {
  const output = `${result.stdout}${result.stderr}`;
  if (result.code !== 0 || !output.trim()) {
    throw new Error(
      `author executable probe \`${command}\` failed (${result.code}): ${
        compactDiagnostic(output) || "no output"
      }`,
    );
  }
  return trim ? output.trim() : output;
}

function selectedCoriIdentity(): CoriIdentity {
  if (!coriBinaryVersion || !coriBinarySha256) {
    throw new Error("selected Cori executable identity was not prepared");
  }
  return {
    path: coriBinary(),
    version: coriBinaryVersion,
    sha256: coriBinarySha256,
  };
}

async function collectInstrumentIdentities(
  adapter: HarnessAdapter,
  gws: GwsClient,
  environment: NodeJS.ProcessEnv,
): Promise<void> {
  harnessIdentity = await adapter.identity();
  const gwsExecutable = await executableFileIdentity(
    gws.underlyingBinary(),
    environment,
  );
  const gwsVersion = await gws.version();
  if (gwsVersion !== "gws 0.22.5") {
    throw new Error(
      `expected gws 0.22.5, found ${gwsVersion}; update the benchmark lock deliberately`,
    );
  }
  gwsIdentity = { ...gwsExecutable, version: gwsVersion };
  temporalIdentity = await versionedExecutableIdentity(
    "temporal",
    environment,
  );
  denoIdentity = await versionedExecutableIdentity(
    process.env.CORI_DENO ?? "deno",
    environment,
  );
  nodeIdentity = await executableFileIdentity(process.execPath, environment);
  benchmarkSourceSha256 = await hashDirectory(join(packageRoot, "src"));
  captureSkillSha256 = await hashDirectory(
    join(repoRoot, "skills", "cori-save-workflow"),
  );
}

async function versionedExecutableIdentity(
  command: string,
  environment: NodeJS.ProcessEnv,
): Promise<ExecutableFileIdentity & { version: string }> {
  const identity = await executableFileIdentity(command, environment);
  const result = await runProcess(identity.path, ["--version"], undefined, environment);
  if (result.code !== 0 || !result.stdout.trim()) {
    throw new Error(
      `${identity.path} --version failed: ${
        compactDiagnostic(`${result.stdout}\n${result.stderr}`) || "no output"
      }`,
    );
  }
  return { ...identity, version: result.stdout.trim() };
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
  const started = performance.now();
  const result = await runProcess(coriBinary(), args, cwd);
  return {
    ...result,
    wallTimeMs: Math.round(performance.now() - started),
  };
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

interface PhaseTimer {
  startedAt: string;
  startedMs: number;
}

function startPhase(): PhaseTimer {
  return { startedAt: new Date().toISOString(), startedMs: performance.now() };
}

function finishPhase(
  timer: PhaseTimer,
  status: PhaseOutcome["status"],
  error?: string,
  details: Partial<PhaseOutcome> = {},
): PhaseOutcome {
  const finishedAt = new Date().toISOString();
  return {
    status,
    startedAt: timer.startedAt,
    finishedAt,
    wallTimeMs: Math.round(performance.now() - timer.startedMs),
    ...details,
    ...(error ? { error } : {}),
  };
}

function skippedOutcome(error: string): PhaseOutcome {
  const now = new Date().toISOString();
  return {
    status: "skipped",
    startedAt: now,
    finishedAt: now,
    wallTimeMs: 0,
    error,
  };
}

function sessionUsage(session: HarnessSession): Partial<PhaseOutcome> {
  return {
    inputTokens: session.usage.inputTokens,
    outputTokens: session.usage.outputTokens,
    toolCalls: session.usage.toolCalls,
  };
}

function combinedSessionUsage(
  sessions: readonly (HarnessSession | undefined)[],
): Partial<PhaseOutcome> {
  const present = sessions.filter((session): session is HarnessSession =>
    session !== undefined
  );
  return {
    inputTokens: sumNullable(present.map((session) =>
      session.usage.inputTokens
    )),
    outputTokens: sumNullable(present.map((session) =>
      session.usage.outputTokens
    )),
    toolCalls: sumNullable(present.map((session) => session.usage.toolCalls)),
  };
}

function harnessEvidenceOptions(
  path: string,
  phase: string,
  detail: string,
  onProgress: (phase: string, detail: string) => Promise<void>,
  timeoutMs: number,
): HarnessExecutionOptions {
  let lastProgressDetail: string | undefined;
  return {
    timeoutMs,
    onProgress: async (partial) => {
      await writeJson(path, partial);
      const progressDetail =
        `${detail} (${formatElapsed(partial.wallTimeMs)} elapsed)`;
      if (progressDetail !== lastProgressDetail) {
        lastProgressDetail = progressDetail;
        await onProgress(phase, progressDetail);
      }
    },
  };
}

function formatElapsed(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1_000);
  return seconds < 60
    ? `${seconds}s`
    : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

export function phaseTimeoutMs(phase: string): number {
  const key = `CORI_BENCH_${phase.toUpperCase()}_TIMEOUT_MS`;
  const defaults: Record<string, number> = {
    author: 15 * 60_000,
    direct: 15 * 60_000,
    capture_preview: 15 * 60_000,
    capture_approval: 20 * 60_000,
  };
  const fallback = defaults[phase] ?? 10 * 60_000;
  const configured = Number(process.env[key] ?? fallback);
  if (!Number.isSafeInteger(configured) || configured <= 0) {
    throw new Error(`${key} must be a positive integer`);
  }
  return configured;
}

function sumPhaseTime(
  tasks: readonly TaskCapture[],
  phase: keyof TaskCapture["outcomes"],
): number {
  return tasks.reduce((sum, task) => sum + task.outcomes[phase].wallTimeMs, 0);
}

function failedTaskWorkflow(
  task: TaskSpec,
  error: string,
  agentRoot: string,
): CapturedTaskWorkflow {
  const author = finishPhase(startPhase(), "failed", error);
  return {
    capture: {
      taskId: task.id,
      authorGrade: emptyGrade(task, error),
      outcomes: {
        author,
        capture: skippedOutcome("author phase failed"),
        check: skippedOutcome("author phase failed"),
        replay: skippedOutcome("author phase failed"),
      },
      previewPresented: false,
      previewDidNotWrite: false,
      skillCheckObserved: false,
      skillCheckSucceeded: false,
      benchmarkCheckSucceeded: false,
      runtimeModelDataflowVerified: null,
      checkPassed: false,
      policy: null,
      workflowHash: null,
      workflowPath: null,
      error,
    },
    workflowDir: null,
    authorWorkspace: join(agentRoot, "authors", task.id),
  };
}

function emptyGrade(task: TaskSpec, note: string): Grade {
  return {
    score: 0,
    passed: false,
    safetyViolations: [],
    items: task.rubric.map((item) => ({
      id: item.id,
      earned: 0,
      max: item.points,
      note,
    })),
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function sumNullable(values: readonly (number | null)[]): number | null {
  if (values.length === 0 || values.some((value) => value === null)) return null;
  return (values as readonly number[]).reduce((sum, value) => sum + value, 0);
}

interface AuditEvidence {
  complete: boolean;
  events: readonly {
    argv: readonly string[];
    cwd: string;
    at: string;
    pid: number;
  }[];
}

export function previewHadNoSideEffects(options: {
  cleanStart: boolean;
  workspaceHashBefore: string;
  workspaceHashAfter: string;
  auditBefore: AuditEvidence;
  auditAfter: AuditEvidence;
  session: Pick<HarnessSession, "transcript">;
}): boolean {
  return options.cleanStart &&
    options.workspaceHashBefore === options.workspaceHashAfter &&
    auditEvidenceUnchanged(options.auditBefore, options.auditAfter) &&
    !transcriptExecutedCoriRun(options.session) &&
    !transcriptExecutedCoriCheck(options.session) &&
    transcriptWorkspaceToolViolations(options.session).length === 0;
}

function auditEvidenceUnchanged(
  before: AuditEvidence,
  after: AuditEvidence,
): boolean {
  return before.complete && after.complete &&
    JSON.stringify(before.events) === JSON.stringify(after.events);
}

export function captureAuditHasNoMutations(
  before: AuditEvidence,
  after: AuditEvidence,
): boolean {
  if (
    !before.complete ||
    !after.complete ||
    after.events.length < before.events.length ||
    JSON.stringify(after.events.slice(0, before.events.length)) !==
      JSON.stringify(before.events)
  ) return false;
  return after.events.slice(before.events.length).every((event) =>
    captureAuditCommandIsReadOnly(event.argv)
  );
}

function captureAuditCommandIsReadOnly(argv: readonly string[]): boolean {
  if (argv.length === 1 && ["--help", "--version"].includes(argv[0]!)) {
    return true;
  }
  const flag = argv.findIndex((arg) => arg.startsWith("--"));
  const path = argv.slice(0, flag < 0 ? argv.length : flag)
    .map((part) => part.toLowerCase());
  if (path[0] === "schema") return true;
  if (path[0] === "auth" && path[1] === "status") return true;
  return [
    "batchget",
    "download",
    "export",
    "get",
    "getprofile",
    "instances",
    "list",
  ].includes(path.at(-1) ?? "");
}

export function transcriptExecutedCoriRun(
  session: { transcript: readonly unknown[] },
): boolean {
  return session.transcript.some((event) => hasCoriCommand(event, "run"));
}

export function transcriptExecutedCoriCheck(
  session: { transcript: readonly unknown[] },
): boolean {
  return session.transcript.some((event) => hasCoriCommand(event, "check"));
}

export function transcriptSuccessfulCoriCheck(
  session: { transcript: readonly unknown[] },
): boolean {
  return session.transcript.some((event) =>
    hasSuccessfulCompletedCoriCheck(event)
  );
}

function hasSuccessfulCompletedCoriCheck(
  value: unknown,
  completed = false,
): boolean {
  if (Array.isArray(value)) {
    return value.some((entry) =>
      hasSuccessfulCompletedCoriCheck(entry, completed)
    );
  }
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  const isCompleted = completed || record.type === "item.completed" ||
    record.status === "completed";
  if (
    isCompleted && record.type === "command_execution" &&
    typeof record.command === "string" &&
    hasCoriCommand(record, "check")
  ) {
    const exitCode = record.exit_code ?? record.exitCode;
    const output = [
      record.aggregated_output,
      record.stdout,
      record.stderr,
      record.output,
    ].filter((entry): entry is string => typeof entry === "string").join("\n");
    if (
      (exitCode === undefined || exitCode === 0) &&
      isCanonicalCoriReadyOutput(output)
    ) return true;
  }
  return Object.values(record).some((nested) =>
    typeof nested !== "string" &&
    hasSuccessfulCompletedCoriCheck(nested, isCompleted)
  );
}

function hasCoriCommand(value: unknown, verb: "run" | "check"): boolean {
  if (Array.isArray(value)) {
    return value.some((entry) => hasCoriCommand(entry, verb));
  }
  if (!value || typeof value !== "object") return false;
  for (const [key, nested] of Object.entries(value)) {
    if (
      key === "command" && typeof nested === "string" &&
      new RegExp(`(?:^|[\\s\"'])[^\\s\"']*cori[\"']?\\s+${verb}\\b`, "iu")
        .test(nested)
    ) return true;
    if (typeof nested !== "string" && hasCoriCommand(nested, verb)) return true;
  }
  return false;
}

export function transcriptHasWorkflowPreview(
  session: { transcript: readonly unknown[] },
): boolean {
  const text = session.transcript.map((event) => assistantText(event)).join("\n");
  return /captured-workflow\/?/iu.test(text) &&
    /manifest\.md/iu.test(text) &&
    /---[\s\S]*\bid\s*:/iu.test(text) &&
    /\n#\s+[^\n]+/u.test(text) &&
    /\n##\s+(?:Goal|Steps|Verification)/iu.test(text);
}

function assistantText(value: unknown, assistant = false): string {
  if (typeof value === "string") return assistant ? value : "";
  if (Array.isArray(value)) {
    return value.map((entry) => assistantText(entry, assistant)).join("\n");
  }
  if (!value || typeof value !== "object") return "";
  const record = value as Record<string, unknown>;
  const marker = [record.role, record.type, record.kind]
    .filter((entry): entry is string => typeof entry === "string")
    .join(" ");
  const nestedAssistant = assistant ||
    /(?:^|[_\s.-])(?:assistant|agent_message)(?:$|[_\s.-])/iu.test(marker);
  return Object.entries(record)
    .filter(([key]) => !["role", "type", "kind"].includes(key))
    .map(([, nested]) => assistantText(nested, nestedAssistant))
    .join("\n");
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

async function containsFiles(directory: string): Promise<boolean> {
  try {
    return (await readdir(directory)).length > 0;
  } catch {
    return false;
  }
}

export function hardGate(
  grade: TrialResult["grade"],
  traceOk: boolean,
  hashUnchanged: boolean,
  missingRuntimeModel = false,
): TrialResult["grade"] {
  const safetyViolations = [...grade.safetyViolations];
  if (!traceOk) {
    safetyViolations.push(replayTraceFailure);
  }
  if (!hashUnchanged) {
    safetyViolations.push(replayMutationFailure);
  }
  if (missingRuntimeModel) {
    safetyViolations.push(missingRuntimeModelFailure);
  }
  return safetyViolations.length === grade.safetyViolations.length
    ? grade
    : { ...grade, score: 0, passed: false, safetyViolations };
}

export function hardGateHarnessTrial(
  grade: TrialResult["grade"],
  session: HarnessSession,
): TrialResult["grade"] {
  const violations = [...grade.safetyViolations];
  if (session.timedOut) {
    violations.push("measured agent timed out before a clean task completion");
  } else if (session.exitCode !== 0) {
    violations.push(`measured agent exited ${session.exitCode}`);
  }
  violations.push(...transcriptWorkspaceToolViolations(session));
  return violations.length === grade.safetyViolations.length
    ? grade
    : {
      ...grade,
      score: 0,
      passed: false,
      safetyViolations: [...new Set(violations)],
    };
}

/**
 * Workspace writes are only auditable when they go through the pinned `gws`
 * executable. Reject common ways an unrestricted shell agent could bypass the
 * proxy and mutate Google APIs directly.
 */
export function transcriptWorkspaceToolViolations(
  session: Pick<HarnessSession, "transcript">,
): string[] {
  const violations: string[] = [];
  for (const command of transcriptCommands(session.transcript)) {
    if (/(?:^|[\s;&|])(curl|wget)(?:\s|$)/iu.test(command)) {
      violations.push(
        "measured agent attempted network side effects with curl/wget instead of gws",
      );
    }
    if (
      /(?:googleapis\.com|googleapiclient|google\.auth|from\s+google\s+import|require\(["']googleapis["']\)|from\s+["']googleapis["'])/iu
        .test(command)
    ) {
      violations.push(
        "measured agent attempted a direct Google API client/script instead of gws",
      );
    }
  }
  return [...new Set(violations)];
}

function transcriptCommands(values: readonly unknown[]): string[] {
  const commands: string[] = [];
  const visit = (value: unknown): void => {
    if (Array.isArray(value)) {
      value.forEach(visit);
      return;
    }
    if (!value || typeof value !== "object") return;
    for (const [key, nested] of Object.entries(value)) {
      if (
        ["command", "cmd", "script"].includes(key) &&
        typeof nested === "string"
      ) {
        commands.push(nested);
      } else {
        visit(nested);
      }
    }
  };
  values.forEach(visit);
  return commands;
}

export function trialIntegrityError(
  trials: readonly TrialResult[],
): string | undefined {
  const failures = trials.flatMap((trial) => {
    const safetyFailures = trial.grade.safetyViolations.map((violation) =>
      `${trial.taskId} seed ${trial.seed} ${trial.lane}: ${violation}`
    );
    return safetyFailures;
  });
  return failures.length === 0
    ? undefined
    : `${failures.length} benchmark safety or replay-integrity failure(s):\n${
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

type TrialRuntime = NonNullable<TrialResult["runtime"]>;

export function traceUsage(trace: unknown): TrialRuntime {
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

export function replayRuntimeUsage(
  trace: unknown,
  processWallTimeMs: number | null,
): TrialRuntime {
  return {
    ...traceUsage(trace),
    wallTimeMs: processWallTimeMs,
  };
}

async function sha256(value: string | Uint8Array): Promise<string> {
  const { createHash } = await import("node:crypto");
  return createHash("sha256").update(value).digest("hex");
}

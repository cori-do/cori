import { join, resolve } from "node:path";

import { scorecard } from "./artifacts.js";
import { cleanup, combineRuns, parseBatch, preflight, report, runBenchmark, selectTasks, validate } from "./runner.js";
import type { BenchmarkResultV1 } from "./types.js";
import type { BenchmarkProfile, HarnessName } from "./types.js";
import { writeBenchmarkViewer } from "./viewer.js";

const [command, ...args] = process.argv.slice(2);

try {
  if (command === "validate") {
    await validate();
    console.log("benchmark validation passed");
  } else if (command === "preflight") {
    console.log(JSON.stringify(await preflight(value(args, "--artifacts") ?? undefined), null, 2));
  } else if (command === "plan") {
    const profile = requiredEnum(value(args, "--profile") ?? "full", ["smoke", "full", "publication"] as const, "--profile");
    const taskIds = value(args, "--task")?.split(",").filter(Boolean);
    const batch = parseBatch(value(args, "--batch"));
    const tasks = selectTasks({ profile, harness: "codex", seed: Number(value(args, "--seed") ?? "42"), taskIds, batch });
    console.log(JSON.stringify({ profile, batch: batch ?? null, tasks: tasks.map((task) => ({ id: task.id, runtimeTrack: task.runtimeTrack })) }, null, 2));
  } else if (command === "run") {
    const profile = requiredEnum(value(args, "--profile") ?? "smoke", ["smoke", "full", "publication"] as const, "--profile");
    const harness = requiredEnum(value(args, "--harness") ?? "codex", ["codex", "claude", "gemini"] as const, "--harness");
    const seed = Number(value(args, "--seed") ?? "1");
    if (!Number.isSafeInteger(seed)) throw new Error("--seed must be an integer");
    const taskIds = value(args, "--task")?.split(",").filter(Boolean);
    const artifactsRoot = value(args, "--artifacts");
    const result = await runBenchmark({
      profile,
      harness,
      seed,
      taskIds,
      batch: parseBatch(value(args, "--batch")),
      artifactsRoot,
      runId: value(args, "--run-id"),
      onProgress: (progress) => console.error(`[${progress.updatedAt}] ${progress.phase}${progress.taskId ? ` ${progress.taskNumber}/${progress.totalTasks} ${progress.taskId}` : ""}: ${progress.detail} (direct ${progress.completedDirectTrials}/${progress.plannedTrialsPerLane}, replay ${progress.completedReplayTrials}/${progress.plannedTrialsPerLane})`),
    });
    printResult(result, args, artifactsRoot);
  } else if (command === "cleanup") {
    const runId = required(value(args, "--run-id"), "--run-id");
    await cleanup(runId, value(args, "--artifacts") ?? undefined);
  } else if (command === "report") {
    const runId = required(value(args, "--run-id"), "--run-id");
    const artifactsRoot = value(args, "--artifacts");
    const result = await report(runId, artifactsRoot ?? undefined);
    printResult(result, args, artifactsRoot);
  } else if (command === "view") {
    const runId = required(value(args, "--run-id"), "--run-id");
    const viewer = await writeBenchmarkViewer(
      runId,
      value(args, "--artifacts") ?? "artifacts",
    );
    if (args.includes("--json")) console.log(JSON.stringify({ runId, viewer }, null, 2));
    else console.log(`Benchmark review page: ${viewer}`);
  } else if (command === "combine") {
    const runIds = required(value(args, "--run-ids"), "--run-ids").split(",").filter(Boolean);
    const artifactsRoot = value(args, "--artifacts");
    const result = await combineRuns(runIds, artifactsRoot ?? undefined, value(args, "--run-id"));
    printResult(result, args, artifactsRoot);
  } else {
    throw new Error("usage: benchmark validate | preflight | plan --profile full --batch INDEX/COUNT | run --profile smoke|full|publication [--task id[,id]] [--batch INDEX/COUNT] --harness codex|claude|gemini --seed N [--json] | combine --run-ids id1,id2,... [--run-id ID] [--json] | cleanup --run-id ID | report --run-id ID [--json] | view --run-id ID [--artifacts path] [--json]");
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}

function value(args: readonly string[], flag: string): string | undefined {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

function required(value: string | undefined, flag: string): string {
  if (!value) throw new Error(`${flag} is required`);
  return value;
}

function requiredEnum<T extends string>(value: string, values: readonly T[], flag: string): T {
  if ((values as readonly string[]).includes(value)) return value as T;
  throw new Error(`${flag} must be one of ${values.join(", ")}`);
}

function printResult(
  result: BenchmarkResultV1,
  args: readonly string[],
  artifactsRoot: string | undefined,
): void {
  if (args.includes("--json")) {
    console.log(JSON.stringify({
      runId: result.runId,
      status: result.status,
      capture: result.capture,
      scorecard: result.summary,
      trialFindings: result.trials
        .filter((trial) =>
          trial.grade.score < 100 ||
          trial.grade.safetyViolations.length > 0
        )
        .map((trial) => ({
          taskId: trial.taskId,
          seed: trial.seed,
          lane: trial.lane,
          score: trial.grade.score,
          harnessExitCode: trial.harness?.exitCode ?? null,
          safetyViolations: trial.grade.safetyViolations,
          incompleteItems: trial.grade.items
            .filter((item) => item.earned < item.max)
            .map((item) => item.id),
        })),
    }, null, 2));
    return;
  }
  console.log(scorecard(result));
  console.log(
    `\nArtifacts: ${
      resolve(artifactsRoot ?? "artifacts", result.runId)
    }`,
  );
  console.log(`Shareable report: ${
    resolve(artifactsRoot ?? "artifacts", join(result.runId, "scorecard.md"))
  }`);
}

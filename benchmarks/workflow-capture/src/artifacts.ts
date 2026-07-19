import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import type { BenchmarkResultV1 } from "./types.js";

/** Benchmark pricing in USD per one million tokens. */
export const INPUT_TOKEN_PRICE_PER_MILLION_USD = 2.5;
export const OUTPUT_TOKEN_PRICE_PER_MILLION_USD = 15;

export async function writeJson(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, "utf8");
    await rename(temporary, path);
  } finally {
    await rm(temporary, { force: true });
  }
}

export async function readJson<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(path, "utf8")) as T;
}

export function scorecard(result: BenchmarkResultV1): string {
  const direct = result.trials.filter((trial) => trial.lane === "direct");
  const replay = result.trials.filter((trial) => trial.lane === "replay");
  const directStats = laneStats(direct);
  const replayStats = laneStats(replay);
  const safetyViolations = result.trials.reduce((sum, trial) => sum + trial.grade.safetyViolations.length, 0);
  const taskCaptures = result.capture.tasks ?? [];
  const capturePassed = taskCaptures.filter((capture) => capture.previewDidNotWrite && capture.checkPassed && capture.policy?.ok).length;
  const qualified = taskCaptures.filter((capture) =>
    capture.qualificationPassed === true
  ).length;
  const captureAttempts = taskCaptures.reduce(
    (sum, capture) => sum + (capture.attempts?.length ?? 1),
    0,
  );
  const retriedCaptures = taskCaptures.filter((capture) =>
    (capture.attempts?.length ?? 1) > 1
  ).length;
  const captureRows = captureAttemptRows(taskCaptures);
  const pairedRows = pairedTrialRows(result);
  const directSpread = scoreSpread(directStats);
  const replaySpread = scoreSpread(replayStats);
  const observedComparison = directStats.count === 0 || replayStats.count === 0
    ? "No complete direct/replay comparison was recorded."
    : `Observed score spread: direct agents ${directSpread} points (${
      scoreRange(directStats)
    }); unchanged Cori replays ${replaySpread} points (${
      scoreRange(replayStats)
    }).`;
  const stabilityObservation = directStats.count > 1 &&
      replayStats.count > 1 &&
      replaySpread < directSpread
    ? "In this run, direct agent execution was more variable than replaying the captured Cori workflow."
    : "Direct-agent variability is comparative; every unchanged Cori replay is required to score 100.";
  return [
    `# Cori workflow-capture benchmark: ${result.runId}`,
    "",
    `Run status: **${result.status === "succeeded" ? "completed" : "failed"}**`,
    result.error ? `Run failure: ${result.error}` : "Cori replays scored 100; direct-agent scores remain comparative measurements.",
    "",
    "## Executive summary",
    "",
    observedComparison,
    stabilityObservation,
    "",
    "| Lane | Mean score | Score range | 100-point trials | Trials ≥90 | Mean wall time | Total tokens | Total price (USD) |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    comparisonRow(
      "Direct agent",
      directStats,
      result.metrics.designTokens,
      lanePriceUsd(direct),
    ),
    comparisonRow(
      "Cori replay",
      replayStats,
      result.metrics.runtimeTokens,
      lanePriceUsd(replay),
    ),
    "",
    "## Paired trial detail",
    "",
    "| Task | Seed | Direct agent | Cori replay | Replay − agent | Direct findings | Replay findings |",
    "| --- | ---: | ---: | ---: | ---: | --- | --- |",
    ...(pairedRows.length > 0
      ? pairedRows
      : ["| _No paired trials_ |  |  |  |  |  |  |"]),
    "",
    "## Design-time capture attempts",
    "",
    "| Task | Attempts | Selected | Author scores | Attempt outcomes |",
    "| --- | ---: | ---: | --- | --- |",
    ...(captureRows.length > 0
      ? captureRows
      : ["| _No capture attempts_ |  |  |  |  |"]),
    "",
    "## Capture, safety, and reuse",
    "",
    "| Metric | Result |",
    "| --- | --- |",
    `| Capture preview gate | ${result.capture.previewDidNotWrite ? "pass" : "fail"} |`,
    `| Capture/check | ${result.capture.checkPassed ? "pass" : "fail"} |`,
    `| Task captures validated | ${capturePassed}/${taskCaptures.length} |`,
    `| Functional qualifications | ${qualified}/${taskCaptures.length} |`,
    `| Capture attempts | ${captureAttempts} across ${taskCaptures.length} task(s); ${retriedCaptures} retried |`,
    `| Safety/replay-integrity violations | ${safetyViolations} |`,
    `| Replay − direct 95% CI | ${
      result.summary.pairedDifferenceCi95?.map(formatScore).join(" to ") ??
        "n/a"
    } |`,
    `| Reuse advantage | ${
      result.summary.reuseAdvantageDemonstrated
        ? "demonstrated"
        : "not demonstrated"
    } |`,
    `| Break-even repetitions | ${
      result.metrics.breakEvenRepetitions ?? "n/a"
    } |`,
    "",
    "## How to read this",
    "",
    "- Direct-agent score variation is expected and remains a comparative measurement.",
    "- Every unchanged Cori replay must score 100; a sub-100 replay fails the run.",
    "- Benchmark infrastructure, capture gates, safety, and replay integrity remain hard requirements.",
    `- Token prices use $${INPUT_TOKEN_PRICE_PER_MILLION_USD.toFixed(2)} per 1M input tokens and $${OUTPUT_TOKEN_PRICE_PER_MILLION_USD.toFixed(2)} per 1M output tokens.`,
    "",
    "Missing vendor usage fields are reported as `null`, never as zero.",
  ].join("\n");
}

interface LaneStats {
  count: number;
  mean: number | null;
  min: number | null;
  max: number | null;
  exact: number;
  threshold: number;
  meanWallTimeMs: number | null;
}

function laneStats(trials: BenchmarkResultV1["trials"]): LaneStats {
  const scores = trials.map((trial) => trial.grade.score);
  const wallTimes = trials.flatMap((trial) => {
    const value = trial.harness?.wallTimeMs ?? trial.runtime?.wallTimeMs;
    return value === null || value === undefined ? [] : [value];
  });
  return {
    count: trials.length,
    mean: average(scores),
    min: scores.length > 0 ? Math.min(...scores) : null,
    max: scores.length > 0 ? Math.max(...scores) : null,
    exact: scores.filter((score) => score === 100).length,
    threshold: trials.filter((trial) => trial.grade.passed).length,
    meanWallTimeMs: average(wallTimes),
  };
}

function comparisonRow(
  label: string,
  stats: LaneStats,
  tokens: number | null,
  priceUsd: number | null,
): string {
  return `| ${label} | ${formatScore(stats.mean)} | ${
    scoreRange(stats)
  } | ${stats.exact}/${stats.count} | ${stats.threshold}/${stats.count} | ${
    formatDuration(stats.meanWallTimeMs)
  } | ${formatTokens(tokens)} | ${formatPrice(priceUsd)} |`;
}

function lanePriceUsd(
  trials: readonly BenchmarkResultV1["trials"][number][],
): number | null {
  if (trials.length === 0) return null;
  const prices = trials.map(trialPriceUsd);
  return prices.every((price): price is number => price !== null)
    ? prices.reduce((sum, price) => sum + price, 0)
    : null;
}

export function trialPriceUsd(
  trial: BenchmarkResultV1["trials"][number],
): number | null {
  const usage = trial.lane === "direct"
    ? trial.harness?.usage
    : trial.runtime;
  const inputTokens = usage?.inputTokens;
  const outputTokens = usage?.outputTokens;
  if (inputTokens === null || inputTokens === undefined ||
      outputTokens === null || outputTokens === undefined) return null;
  return inputTokens * INPUT_TOKEN_PRICE_PER_MILLION_USD / 1_000_000 +
    outputTokens * OUTPUT_TOKEN_PRICE_PER_MILLION_USD / 1_000_000;
}

function pairedTrialRows(result: BenchmarkResultV1): string[] {
  const replayByPair = new Map(
    result.trials
      .filter((trial) => trial.lane === "replay")
      .map((trial) => [`${trial.taskId}:${trial.seed}`, trial] as const),
  );
  return result.trials
    .filter((trial) => trial.lane === "direct")
    .map((direct) => {
      const replay = replayByPair.get(`${direct.taskId}:${direct.seed}`);
      const delta = replay
        ? replay.grade.score - direct.grade.score
        : null;
      return `| ${escapeCell(direct.taskId)} | ${direct.seed} | ${
        formatScore(direct.grade.score)
      } | ${formatScore(replay?.grade.score ?? null)} | ${
        delta === null ? "n/a" : `${delta >= 0 ? "+" : ""}${formatScore(delta)}`
      } | ${escapeCell(trialFindings(direct))} | ${
        escapeCell(replay ? trialFindings(replay) : "not recorded")
      } |`;
    });
}

function trialFindings(trial: BenchmarkResultV1["trials"][number]): string {
  const findings = trial.grade.items
    .filter((item) => item.earned < item.max)
    .map((item) => item.id);
  const safety = trial.grade.safetyViolations.map((violation) =>
    `safety: ${violation}`
  );
  return [...findings, ...safety].join(", ") || "none";
}

function captureAttemptRows(
  captures: BenchmarkResultV1["capture"]["tasks"],
): string[] {
  return captures.map((capture) => {
    const attempts = capture.attempts ?? [{
      attempt: 1,
      seed: 0,
      authorGrade: capture.authorGrade,
      ready: capture.checkPassed &&
        capture.policy?.ok === true &&
        capture.qualificationPassed !== false,
      ...(capture.error ? { error: capture.error } : {}),
    }];
    const scores = attempts.map((attempt) =>
      `#${attempt.attempt}: ${formatScore(attempt.authorGrade.score)}`
    ).join("; ");
    const outcomes = attempts.map((attempt) => {
      if (attempt.ready) return `#${attempt.attempt}: ready`;
      const incomplete = attempt.authorGrade.items
        .filter((item) => item.earned < item.max)
        .map((item) => item.id);
      const detail = incomplete.join(", ") ||
        attempt.error?.replaceAll("\n", " ").slice(0, 120) ||
        "not replayable";
      return `#${attempt.attempt}: ${detail}`;
    }).join("; ");
    const selected = capture.selectedAttempt ??
      (attempts.length === 1 && attempts[0]!.ready ? 1 : "none");
    return `| ${escapeCell(capture.taskId)} | ${attempts.length} | ${
      selected
    } | ${escapeCell(scores)} | ${escapeCell(outcomes)} |`;
  });
}

function average(values: readonly number[]): number | null {
  return values.length === 0
    ? null
    : values.reduce((sum, value) => sum + value, 0) / values.length;
}

function scoreRange(stats: LaneStats): string {
  return stats.min === null || stats.max === null
    ? "n/a"
    : `${formatScore(stats.min)}–${formatScore(stats.max)}`;
}

function scoreSpread(stats: LaneStats): number {
  return stats.min === null || stats.max === null ? 0 : stats.max - stats.min;
}

function formatScore(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return "n/a";
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function formatDuration(value: number | null): string {
  return value === null ? "n/a" : `${(value / 1_000).toFixed(1)}s`;
}

function formatTokens(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "n/a"
    : Math.round(value).toLocaleString("en-US");
}

function formatPrice(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "n/a"
    : `$${value.toFixed(4)}`;
}

function escapeCell(value: string): string {
  return value.replaceAll("|", "\\|").replaceAll("\n", " ");
}

export function normalizedCsv(result: BenchmarkResultV1): string {
  const header = "task_id,seed,lane,score,passed,safety_violations,wall_time_ms,tool_calls,input_tokens,output_tokens,runtime_input_tokens,runtime_output_tokens,runtime_cost_eur,price_usd";
  const rows = result.trials.map((trial) => [
    trial.taskId,
    trial.seed,
    trial.lane,
    trial.grade.score,
    trial.grade.passed,
    trial.grade.safetyViolations.length,
    trial.harness?.wallTimeMs ?? trial.runtime?.wallTimeMs ?? "",
    trial.harness?.usage.toolCalls ?? "",
    trial.harness?.usage.inputTokens ?? "",
    trial.harness?.usage.outputTokens ?? "",
    trial.runtime?.inputTokens ?? "",
    trial.runtime?.outputTokens ?? "",
    trial.runtime?.costEur ?? "",
    trialPriceUsd(trial) ?? "",
  ].join(","));
  return `${header}\n${rows.join("\n")}\n`;
}

export function artifactPath(runDir: string, ...parts: readonly string[]): string {
  return join(runDir, ...parts);
}

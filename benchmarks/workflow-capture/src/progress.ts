import type { BenchmarkProgress } from "./runner.js";

export function benchmarkProgressText(progress: BenchmarkProgress): string {
  const task = progress.taskId
    ? ` ${progress.taskNumber}/${progress.totalTasks} ${progress.taskId}`
    : "";
  const counts =
    ` (direct ${progress.completedDirectTrials}/${progress.plannedTrialsPerLane}, replay ${progress.completedReplayTrials}/${progress.plannedTrialsPerLane})`;
  const heading = `[${progress.updatedAt}] ${progress.phase}${task}`;
  const details = progress.detail
    .replaceAll("\r", "")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  if (details.length <= 1) {
    return `${heading}: ${details[0] ?? ""}${counts}`;
  }
  return [
    `${heading}${counts}:`,
    ...details.map((line) => `  ${line}`),
  ].join("\n");
}

export function benchmarkProgressOutput(
  progress: BenchmarkProgress,
  terminal: boolean,
): string {
  return benchmarkDiagnosticOutput(benchmarkProgressText(progress), terminal);
}

export function benchmarkDiagnosticOutput(
  diagnostic: string,
  terminal: boolean,
): string {
  const lines = diagnostic.replaceAll("\r", "").replace(/\n+$/u, "").split("\n");
  if (!terminal) return `${lines.join("\n")}\n`;
  return `${lines.map((line) => `\r\u001b[2K${line}`).join("\r\n")}\r\n`;
}

export function writeBenchmarkProgress(progress: BenchmarkProgress): void {
  process.stderr.write(
    benchmarkProgressOutput(progress, process.stderr.isTTY === true),
  );
}

export function writeBenchmarkDiagnostic(diagnostic: string): void {
  process.stderr.write(
    benchmarkDiagnosticOutput(diagnostic, process.stderr.isTTY === true),
  );
}

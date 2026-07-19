import { SeededRandom } from "./scenario.js";

export function mean(values: readonly number[]): number | null {
  return values.length === 0 ? null : values.reduce((sum, value) => sum + value, 0) / values.length;
}

/** Percentile bootstrap CI for replay minus direct scores; deterministic by seed. */
export function pairedDifferenceCi95(
  direct: readonly number[],
  replay: readonly number[],
  seed = 1,
  samples = 10_000,
): readonly [number, number] | null {
  if (direct.length === 0 || direct.length !== replay.length) return null;
  const deltas = direct.map((value, index) => replay[index]! - value);
  const random = new SeededRandom(seed);
  const means: number[] = [];
  for (let sample = 0; sample < samples; sample += 1) {
    let total = 0;
    for (let index = 0; index < deltas.length; index += 1) total += deltas[random.int(0, deltas.length - 1)]!;
    means.push(total / deltas.length);
  }
  means.sort((a, b) => a - b);
  return [quantile(means, 0.025), quantile(means, 0.975)];
}

export function breakEvenRepetitions(designCost: number, directPerRun: number, replayPerRun: number): number | null {
  const savings = directPerRun - replayPerRun;
  if (!Number.isFinite(designCost) || !Number.isFinite(savings) || savings <= 0) return null;
  return Math.ceil(designCost / savings);
}

export function reuseAdvantage(
  safetyViolations: number,
  pairedCi: readonly [number, number] | null,
  designCost: number | null,
  directPerRun: number | null,
  replayPerRun: number | null,
): boolean {
  if (safetyViolations > 0 || !pairedCi || pairedCi[0] < -5) return false;
  if (designCost === null || directPerRun === null || replayPerRun === null) return false;
  return 5 * replayPerRun + designCost < 5 * directPerRun;
}

function quantile(sorted: readonly number[], p: number): number {
  const at = (sorted.length - 1) * p;
  const low = Math.floor(at);
  const high = Math.ceil(at);
  return sorted[low]! + (sorted[high]! - sorted[low]!) * (at - low);
}

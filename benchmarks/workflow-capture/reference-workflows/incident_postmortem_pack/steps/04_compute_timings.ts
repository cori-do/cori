import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  values: z.array(z.array(z.string())),
  factors: z.array(z.object({
    factor_id: z.string(),
    summary: z.string(),
    confirmed_by: z.string(),
  })),
});
const Output = z.object({
  timings: z.array(z.object({ metric: z.string(), minutes: z.number().int() })),
  incident_summary: z.string(),
});

export default step.code({
  description: "Compute the response durations from the recorded timestamps",
  input: Input,
  output: Output,
  run: ({ values, factors }) => {
    const byMetric = new Map(values.slice(1).map((row) => [row[0] ?? "", row[1] ?? ""]));
    const started = Date.parse(byMetric.get("started_at") ?? "");
    const minutesFromStart = (key: string) => {
      const instant = Date.parse(byMetric.get(key) ?? "");
      if (!Number.isFinite(started) || !Number.isFinite(instant)) {
        throw new Error(`incident metrics are missing a usable ${key}`);
      }
      return Math.round((instant - started) / 60_000);
    };
    const timings = [
      { metric: "time_to_detect", minutes: minutesFromStart("detected_at") },
      { metric: "time_to_mitigate", minutes: minutesFromStart("mitigated_at") },
      { metric: "time_to_resolve", minutes: minutesFromStart("resolved_at") },
    ];
    return {
      timings,
      incident_summary: [
        ...timings.map((timing) => `${timing.metric}: ${timing.minutes} minutes`),
        `confirmed contributing factors: ${factors.length}`,
      ].join("\n"),
    };
  },
});

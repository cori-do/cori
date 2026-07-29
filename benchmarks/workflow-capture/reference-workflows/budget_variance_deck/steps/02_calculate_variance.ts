import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({
  values: z.array(z.array(z.string())),
  run_tag: z.string(),
  period: z.string(),
});
const Output = z.object({
  deck_title: z.string(),
  executive_summary: z.string(),
  unfavourable_summary: z.string(),
  variance_detail: z.string(),
  budget_draft_summary: z.string(),
});
export default step.code({
  description: "Calculate signed budget variance summaries",
  input: Input,
  output: Output,
  run: ({ values, run_tag, period }) => {
    const lines = values.slice(1).filter((row) => row[5] === period).map((row) => {
      const type = row[1] ?? "";
      const category = row[2] ?? "";
      const budget = Number(row[3] ?? "");
      const actual = Number(row[4] ?? "");
      const variance = actual - budget;
      const percent = budget === 0
        ? null
        : Math.round((variance / budget) * 1_000) / 10;
      const unfavourable = percent !== null &&
        (type === "expense" ? percent > 10 : percent < -10);
      return { type, category, budget, actual, variance, percent, unfavourable };
    });
    const unfavourable = lines.filter((line) => line.unfavourable);
    const format = (line: typeof lines[number]) =>
      `${line.category}: budget ${line.budget}, actual ${line.actual}, variance ${line.variance}, ${
        line.percent === null ? "N/A" : `${line.percent}%`
      }`;
    return {
      deck_title: `${run_tag} Budget Variance Deck`,
      executive_summary: `Executive Summary\n${run_tag}\nUnfavourable lines: ${unfavourable.length}`,
      unfavourable_summary: `Unfavourable Variances\n${
        unfavourable.map(format).join("\n") || "None"
      }`,
      variance_detail: `Detail\n${lines.map(format).join("\n")}`,
      budget_draft_summary: `${run_tag}\nUnfavourable variances: ${unfavourable.length}`,
    };
  },
});

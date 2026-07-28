import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  findings_spreadsheet_id: z.string(),
  run_tag: z.string(),
  factors: z.array(z.object({ factor_id: z.string(), summary: z.string(), confirmed_by: z.string() })),
  timings: z.array(z.object({ metric: z.string(), minutes: z.number().int() })),
});
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the contributing factors and response timings",
  input: Input,
  output: Output,
  command: ({ findings_spreadsheet_id, run_tag, factors, timings }) => [
    "gws", "sheets", "spreadsheets", "values", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: findings_spreadsheet_id }),
    "--json", JSON.stringify({
      valueInputOption: "RAW",
      data: [
        {
          range: "'Contributing Factors'!A1",
          values: [
            ["factor_id", "summary", "confirmed_by", "run_tag"],
            ...[...factors]
              .sort((left, right) => left.factor_id.localeCompare(right.factor_id))
              .map((factor) => [factor.factor_id, factor.summary, factor.confirmed_by, run_tag]),
          ],
        },
        {
          range: "Timings!A1",
          values: [
            ["metric", "minutes", "run_tag"],
            ...timings.map((timing) => [timing.metric, String(timing.minutes), run_tag]),
          ],
        },
      ],
    }),
  ],
  parse: () => ({ updated: 2 }),
});

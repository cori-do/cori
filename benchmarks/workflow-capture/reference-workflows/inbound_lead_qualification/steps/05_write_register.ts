import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  lead_spreadsheet_id: z.string(),
  run_tag: z.string(),
  as_of: z.string(),
  rows: z.array(z.array(z.string())),
});
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the ranked qualified leads",
  input: Input,
  output: Output,
  command: ({ lead_spreadsheet_id, run_tag, as_of, rows }) => [
    "gws", "sheets", "spreadsheets", "values", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: lead_spreadsheet_id }),
    "--json", JSON.stringify({
      valueInputOption: "RAW",
      data: [{
        range: "'Qualified Leads'!A1",
        values: [
          ["message_id", "sender", "company", "seat_count", "timeline_days", "security_review", "score", "band", "run_tag", "as_of"],
          ...rows.map((row) => [...row, run_tag, as_of]),
        ],
      }],
    }),
  ],
  parse: () => ({ updated: 1 }),
});

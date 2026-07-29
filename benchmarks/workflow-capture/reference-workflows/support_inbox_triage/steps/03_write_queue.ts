import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  queue_spreadsheet_id: z.string(),
  run_tag: z.string(),
  as_of: z.string(),
  rows: z.array(z.array(z.string())),
});
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the sorted triage queue",
  input: Input,
  output: Output,
  command: ({ queue_spreadsheet_id, run_tag, as_of, rows }) => [
    "gws", "sheets", "spreadsheets", "values", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: queue_spreadsheet_id }),
    "--json", JSON.stringify({
      valueInputOption: "RAW",
      data: [{
        range: "'Triage Queue'!A1",
        values: [
          ["message_id", "received_at", "sender", "subject", "category", "priority", "status", "run_tag", "as_of"],
          ...rows.map((row) => [...row, run_tag, as_of]),
        ],
      }],
    }),
  ],
  parse: () => ({ updated: 1 }),
});

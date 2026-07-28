import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  register_spreadsheet_id: z.string(),
  run_tag: z.string(),
  as_of: z.string(),
  rows: z.array(z.array(z.string())),
});
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the invoice register",
  input: Input,
  output: Output,
  command: ({ register_spreadsheet_id, run_tag, as_of, rows }) => [
    "gws", "sheets", "spreadsheets", "values", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: register_spreadsheet_id }),
    "--json", JSON.stringify({
      valueInputOption: "RAW",
      data: [{
        range: "'Invoice Register'!A1",
        values: [
          ["document_id", "vendor", "invoice_number", "currency", "net", "tax", "gross", "due_date", "status", "run_tag", "as_of"],
          ...rows.map((row) => [...row, run_tag, as_of]),
        ],
      }],
    }),
  ],
  parse: () => ({ updated: 1 }),
});

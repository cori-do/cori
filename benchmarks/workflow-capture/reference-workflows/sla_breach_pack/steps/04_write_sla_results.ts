import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ case_spreadsheet_id: z.string(), rows: z.array(z.array(z.string())) });
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the SLA breach result rows",
  input: Input,
  output: Output,
  command: ({ case_spreadsheet_id, rows }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: case_spreadsheet_id }), "--json", JSON.stringify({
    valueInputOption: "RAW",
    data: [{
      range: "'SLA Results'!A1",
      values: [
        ["case_id", "status", "priority", "opened_at", "sla_deadline", "breached", "due_within_two_hours", "run_tag"],
        ...rows,
      ],
    }],
  })],
  parse: () => ({ updated: 1 }),
});

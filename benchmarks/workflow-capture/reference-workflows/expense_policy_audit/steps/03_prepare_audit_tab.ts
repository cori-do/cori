import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ expense_spreadsheet_id: z.string() });
const Output = z.object({ audit_tab_created: z.boolean() });

export default step.cli({
  description: "Create the expense audit tab",
  input: Input,
  output: Output,
  command: ({ expense_spreadsheet_id }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: expense_spreadsheet_id }),
    "--json", JSON.stringify({
      requests: [{ addSheet: { properties: { title: "Audit" } } }],
    }),
  ],
  parse: () => ({ audit_tab_created: true }),
});

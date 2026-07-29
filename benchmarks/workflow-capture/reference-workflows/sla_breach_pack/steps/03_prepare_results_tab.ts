import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ case_spreadsheet_id: z.string() });
const Output = z.object({ sla_results_tab_created: z.boolean() });

export default step.cli({
  description: "Create the SLA results tab",
  input: Input,
  output: Output,
  command: ({ case_spreadsheet_id }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: case_spreadsheet_id }),
    "--json", JSON.stringify({
      requests: [{ addSheet: { properties: { title: "SLA Results" } } }],
    }),
  ],
  parse: () => ({ sla_results_tab_created: true }),
});

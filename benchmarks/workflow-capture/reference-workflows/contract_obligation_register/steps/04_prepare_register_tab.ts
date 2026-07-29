import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ register_spreadsheet_id: z.string() });
const Output = z.object({ obligations_tab_created: z.boolean() });

export default step.cli({
  description: "Create the Obligations register tab",
  input: Input,
  output: Output,
  command: ({ register_spreadsheet_id }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: register_spreadsheet_id }),
    "--json", JSON.stringify({
      requests: [{ addSheet: { properties: { title: "Obligations" } } }],
    }),
  ],
  parse: () => ({ obligations_tab_created: true }),
});

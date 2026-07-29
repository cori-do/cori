import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ findings_spreadsheet_id: z.string() });
const Output = z.object({ findings_tabs_created: z.boolean() });

export default step.cli({
  description: "Create the postmortem findings tabs",
  input: Input,
  output: Output,
  command: ({ findings_spreadsheet_id }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: findings_spreadsheet_id }),
    "--json", JSON.stringify({
      requests: [
        { addSheet: { properties: { title: "Contributing Factors" } } },
        { addSheet: { properties: { title: "Timings" } } },
      ],
    }),
  ],
  parse: () => ({ findings_tabs_created: true }),
});

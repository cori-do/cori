import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ project_spreadsheet_id: z.string() });
const Output = z.object({ weekly_review_tab_created: z.boolean() });

export default step.cli({
  description: "Create the weekly review tab",
  input: Input,
  output: Output,
  command: ({ project_spreadsheet_id }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: project_spreadsheet_id }),
    "--json", JSON.stringify({
      requests: [{ addSheet: { properties: { title: "Weekly Review" } } }],
    }),
  ],
  parse: () => ({ weekly_review_tab_created: true }),
});

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  spreadsheet_id: z.string(),
  target_tab: z.string(),
});

const Output = z.object({
  created: z.boolean(),
  tab: z.string(),
});

export default step.cli({
  description: "Create the target FR tab if it does not exist (idempotent)",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, target_tab }) => [
    "gws",
    "sheets",
    "spreadsheets",
    "batchUpdate",
    "--params",
    JSON.stringify({
      spreadsheetId: spreadsheet_id,
      requests: [{ addSheet: { properties: { title: target_tab } } }],
      ignoreExistingSheet: true,
    }),
  ],
  parse: (_stdout, { exitCode }) => ({
    created: exitCode === 0,
    tab: "(injected by worker)",
  }),
});

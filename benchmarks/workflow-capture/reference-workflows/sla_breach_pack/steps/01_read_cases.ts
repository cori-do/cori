import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ case_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });

export default step.cli({
  description: "Read source support cases from Google Sheets",
  input: Input,
  output: Output,
  command: ({ case_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: case_spreadsheet_id, range: "Cases" })],
  parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }),
});

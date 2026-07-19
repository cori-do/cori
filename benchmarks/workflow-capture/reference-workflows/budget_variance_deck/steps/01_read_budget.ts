import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ budget_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });
export default step.cli({ description: "Read budget and actual values", input: Input, output: Output, command: ({ budget_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: budget_spreadsheet_id, range: "Budget" })], parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }) });

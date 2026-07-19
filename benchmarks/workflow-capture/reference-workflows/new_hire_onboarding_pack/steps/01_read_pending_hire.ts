import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ new_hire_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });
export default step.cli({ description: "Read pending new hire row", input: Input, output: Output, command: ({ new_hire_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: new_hire_spreadsheet_id, range: "New Hires" })], parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }) });

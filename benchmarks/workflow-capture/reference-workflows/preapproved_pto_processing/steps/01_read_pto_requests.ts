import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ pto_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });
export default step.cli({ description: "Read approved PTO request and holidays", input: Input, output: Output, command: ({ pto_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: pto_spreadsheet_id, range: "PTO" })], parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }) });

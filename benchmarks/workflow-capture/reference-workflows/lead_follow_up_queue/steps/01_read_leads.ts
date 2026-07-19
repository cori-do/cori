import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ lead_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });
export default step.cli({ description: "Read active sales leads", input: Input, output: Output, command: ({ lead_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: lead_spreadsheet_id, range: "Leads" })], parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }) });

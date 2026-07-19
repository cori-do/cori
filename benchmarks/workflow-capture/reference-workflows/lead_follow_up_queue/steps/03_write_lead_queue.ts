import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ lead_spreadsheet_id: z.string(), rows: z.array(z.array(z.string())) });
const Output = z.object({ updated: z.number() });
export default step.cli({ description: "Batch-write ranked lead queue", input: Input, output: Output, command: ({ lead_spreadsheet_id, rows }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: lead_spreadsheet_id }), "--json", JSON.stringify({ valueInputOption: "RAW", data: [{ range: "Follow-up Queue!A1", values: rows }] })], parse: () => ({ updated: 1 }) });

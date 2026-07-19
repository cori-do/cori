import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ expense_spreadsheet_id: z.string(), rows: z.array(z.array(z.string())) });
const Output = z.object({ updated: z.number() });
export default step.cli({ description: "Batch-write expense audit findings", input: Input, output: Output, command: ({ expense_spreadsheet_id, rows }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: expense_spreadsheet_id }), "--json", JSON.stringify({ valueInputOption: "RAW", data: [{ range: "Audit!A1", values: rows }] })], parse: () => ({ updated: 1 }) });

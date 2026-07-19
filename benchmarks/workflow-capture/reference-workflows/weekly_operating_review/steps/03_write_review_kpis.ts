import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ project_spreadsheet_id: z.string(), rows: z.array(z.array(z.string())) });
const Output = z.object({ updated: z.number() });
export default step.cli({ description: "Batch-write weekly RAG and KPIs", input: Input, output: Output, command: ({ project_spreadsheet_id, rows }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: project_spreadsheet_id }), "--json", JSON.stringify({ valueInputOption: "RAW", data: [{ range: "Weekly Review!A1", values: rows }] })], parse: () => ({ updated: 1 }) });

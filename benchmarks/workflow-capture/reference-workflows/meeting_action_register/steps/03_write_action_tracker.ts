import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ action_tracker_spreadsheet_id: z.string(), run_tag: z.string(), actions: z.array(z.object({ action: z.string(), owner: z.string(), due_date: z.string(), source_section: z.string() })) });
const Output = z.object({ updated: z.number() });
export default step.cli({ description: "Batch-write deduplicated action tracker", input: Input, output: Output, command: ({ action_tracker_spreadsheet_id, run_tag, actions }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: action_tracker_spreadsheet_id }), "--json", JSON.stringify({ valueInputOption: "RAW", data: [{ range: "Actions!A1", values: [["action", "owner", "due_date", "source_section", "benchmark_tag"], ...actions.map((item) => [item.action, item.owner, item.due_date, item.source_section, run_tag])] }] })], parse: () => ({ updated: 1 }) });

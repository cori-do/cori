import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ queue_spreadsheet_id: z.string(), run_tag: z.string(), classifications: z.array(z.object({ id: z.string(), category: z.string(), priority: z.string() })) });
const Output = z.object({ updated: z.number() });

export default step.cli({
  description: "Batch-write the sorted support queue",
  input: Input,
  output: Output,
  command: ({ queue_spreadsheet_id, run_tag, classifications }) => ["gws", "sheets", "spreadsheets", "values", "batchUpdate", "--params", JSON.stringify({ spreadsheetId: queue_spreadsheet_id }), "--json", JSON.stringify({ valueInputOption: "RAW", data: [{ range: "Queue!A1", values: [["message_id", "category", "priority", "run_tag"], ...classifications.map((item) => [item.id, item.category, item.priority, run_tag])] }] })],
  parse: () => ({ updated: 1 }),
});

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  report_document_id: z.string(),
  breached_count: z.number().int(),
  warning_count: z.number().int(),
  run_tag: z.string(),
});
const Output = z.object({ report_updated: z.boolean() });

export default step.cli({
  description: "Fill the copied SLA report with computed totals",
  input: Input,
  output: Output,
  command: ({ report_document_id, breached_count, warning_count, run_tag }) => [
    "gws", "docs", "documents", "batchUpdate",
    "--params", JSON.stringify({ documentId: report_document_id }),
    "--json", JSON.stringify({
      requests: [
        {
          replaceAllText: {
            containsText: { text: "{{BREACHED_COUNT}}", matchCase: true },
            replaceText: String(breached_count),
          },
        },
        {
          replaceAllText: {
            containsText: { text: "{{WARNING_COUNT}}", matchCase: true },
            replaceText: String(warning_count),
          },
        },
        {
          replaceAllText: {
            containsText: { text: "{{RUN_TAG}}", matchCase: true },
            replaceText: run_tag,
          },
        },
      ],
    }),
  ],
  parse: () => ({ report_updated: true }),
});

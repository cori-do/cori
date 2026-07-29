import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  report_document_id: z.string(),
  exception_count: z.number().int(),
  reason_codes: z.string(),
  run_tag: z.string(),
});
const Output = z.object({ report_updated: z.boolean() });

export default step.cli({
  description: "Fill the copied exception report",
  input: Input,
  output: Output,
  command: ({ report_document_id, exception_count, reason_codes, run_tag }) => [
    "gws", "docs", "documents", "batchUpdate",
    "--params", JSON.stringify({ documentId: report_document_id }),
    "--json", JSON.stringify({
      requests: [
        {
          replaceAllText: {
            containsText: { text: "{{EXCEPTION_COUNT}}", matchCase: true },
            replaceText: String(exception_count),
          },
        },
        {
          replaceAllText: {
            containsText: { text: "{{REASONS}}", matchCase: true },
            replaceText: reason_codes || "none",
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

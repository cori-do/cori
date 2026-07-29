import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  report_document_id: z.string(),
  week_ending: z.string(),
  red_count: z.number().int(),
  amber_count: z.number().int(),
  green_count: z.number().int(),
  escalations: z.string(),
  run_tag: z.string(),
});
const Output = z.object({ report_updated: z.boolean() });

export default step.cli({
  description: "Fill the copied weekly operating review",
  input: Input,
  output: Output,
  command: ({
    report_document_id,
    week_ending,
    red_count,
    amber_count,
    green_count,
    escalations,
    run_tag,
  }) => [
    "gws", "docs", "documents", "batchUpdate",
    "--params", JSON.stringify({ documentId: report_document_id }),
    "--json", JSON.stringify({
      requests: [
        ["{{WEEK_ENDING}}", week_ending],
        ["{{GREEN_COUNT}}", String(green_count)],
        ["{{AMBER_COUNT}}", String(amber_count)],
        ["{{RED_COUNT}}", String(red_count)],
        ["{{ESCALATIONS}}", escalations || "none"],
        ["{{RUN_TAG}}", run_tag],
      ].map(([text, replaceText]) => ({
        replaceAllText: {
          containsText: { text, matchCase: true },
          replaceText,
        },
      })),
    }),
  ],
  parse: () => ({ report_updated: true }),
});

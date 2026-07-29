import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ report_template_id: z.string(), run_tag: z.string() });
const Output = z.object({ report_document_id: z.string() });

export default step.cli({
  description: "Copy the supplied SLA report template",
  input: Input,
  output: Output,
  command: ({ report_template_id, run_tag }) => [
    "gws", "drive", "files", "copy",
    "--params", JSON.stringify({ fileId: report_template_id }),
    "--json", JSON.stringify({
      name: `SLA Breach Pack ${run_tag}`,
      description: run_tag,
    }),
    "--format", "json",
  ],
  parse: (stdout) => ({
    report_document_id: (JSON.parse(stdout) as { id?: string }).id ?? "",
  }),
});

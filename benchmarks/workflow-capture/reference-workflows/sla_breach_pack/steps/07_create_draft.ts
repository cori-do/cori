import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ run_tag: z.string(), sla_draft_summary: z.string() });
const Output = z.object({ draft_id: z.string() });

export default step.cli({
  description: "Create the SLA summary draft for the support lead",
  input: Input,
  output: Output,
  command: ({ run_tag, sla_draft_summary }) => {
    const raw = btoa([
      "To: support-lead@example.test",
      `Subject: [${run_tag}] SLA breach pack`,
      "",
      sla_draft_summary,
    ].join("\r\n")).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
    return [
      "gws", "gmail", "users", "drafts", "create",
      "--params", JSON.stringify({ userId: "me" }),
      "--json", JSON.stringify({ message: { raw } }),
      "--format", "json",
    ];
  },
  parse: (stdout) => ({
    draft_id: (JSON.parse(stdout) as { id?: string }).id ?? "",
  }),
});

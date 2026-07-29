import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ run_tag: z.string(), review_draft_summary: z.string() });
const Output = z.object({ draft_id: z.string() });

export default step.cli({
  description: "Create the weekly review summary draft for leadership",
  input: Input,
  output: Output,
  command: ({ run_tag, review_draft_summary }) => {
    const raw = btoa([
      "To: leadership@example.test",
      `Subject: [${run_tag}] Weekly operating review`,
      "",
      review_draft_summary,
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

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ run_tag: z.string(), summary: z.string() });
const Output = z.object({ draft_id: z.string() });

export default step.cli({
  description: "Draft the accounts payable exception list",
  input: Input,
  output: Output,
  command: ({ run_tag, summary }) => [
    "gws", "gmail", "users", "drafts", "create",
    "--params", JSON.stringify({ userId: "me" }),
    "--json", JSON.stringify({
      message: {
        raw: btoa([
          "To: ap-lead@example.test",
          `Subject: [${run_tag}] Invoice intake exceptions`,
          "",
          summary,
        ].join("\r\n")).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, ""),
      },
    }),
  ],
  parse: (stdout) => ({ draft_id: (JSON.parse(stdout) as { id?: string }).id ?? "" }),
});

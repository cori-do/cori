import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ top_sender: z.string(), run_tag: z.string(), summary: z.string() });
const Output = z.object({ draft_id: z.string() });

export default step.cli({
  description: "Draft the reply to the highest-scoring lead",
  input: Input,
  output: Output,
  command: ({ top_sender, run_tag, summary }) => [
    "gws", "gmail", "users", "drafts", "create",
    "--params", JSON.stringify({ userId: "me" }),
    "--json", JSON.stringify({
      message: {
        raw: btoa([
          `To: ${top_sender}`,
          `Subject: [${run_tag}] Following up on your enquiry`,
          "",
          summary,
        ].join("\r\n")).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, ""),
      },
    }),
  ],
  parse: (stdout) => ({ draft_id: (JSON.parse(stdout) as { id?: string }).id ?? "" }),
});

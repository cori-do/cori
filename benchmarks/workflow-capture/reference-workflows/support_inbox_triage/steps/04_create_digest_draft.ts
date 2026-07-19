import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ run_tag: z.string(), classifications: z.array(z.unknown()) });
const Output = z.object({ draft_id: z.string() });

export default step.cli({
  description: "Create one internal support digest draft",
  input: Input,
  output: Output,
  command: ({ run_tag, classifications }) => {
    const raw = btoa(`To: support-lead@example.test\r\nSubject: [${run_tag}] Support digest\r\n\r\n${JSON.stringify(classifications)}`).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
    return ["gws", "gmail", "users", "drafts", "create", "--params", JSON.stringify({ userId: "me" }), "--json", JSON.stringify({ message: { raw } })];
  },
  parse: (stdout) => ({ draft_id: (JSON.parse(stdout) as { id: string }).id }),
});

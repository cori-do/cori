import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ gmail_query: z.string() });
const Output = z.object({ message_refs: z.array(z.object({ id: z.string() })) });

export default step.cli({
  description: "List the inbound enquiries that arrived overnight",
  input: Input,
  output: Output,
  command: ({ gmail_query }) => ["gws", "gmail", "users", "messages", "list", "--params", JSON.stringify({ userId: "me", q: gmail_query })],
  parse: (stdout) => ({ message_refs: (JSON.parse(stdout) as { messages?: { id: string }[] }).messages ?? [] }),
});

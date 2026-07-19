import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ gmail_query: z.string() });
const Output = z.object({ messages: z.array(z.unknown()) });

export default step.cli({
  description: "Read synthetic unread support messages",
  input: Input,
  output: Output,
  command: ({ gmail_query }) => ["gws", "gmail", "users", "messages", "list", "--params", JSON.stringify({ userId: "me", q: gmail_query })],
  parse: (stdout) => ({ messages: (JSON.parse(stdout) as { messages?: unknown[] }).messages ?? [] }),
});

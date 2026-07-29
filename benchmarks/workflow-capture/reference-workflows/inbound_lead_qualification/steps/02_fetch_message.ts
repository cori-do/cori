import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ message_id: z.string() });
const Output = z.object({ message: z.unknown() });

export default step.cli({
  description: "Read one enquiry in full",
  input: Input,
  output: Output,
  command: ({ message_id }) => ["gws", "gmail", "users", "messages", "get", "--params", JSON.stringify({ userId: "me", id: message_id, format: "full" })],
  parse: (stdout) => ({ message: JSON.parse(stdout) as unknown }),
});

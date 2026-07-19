import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ messages: z.array(z.unknown()) });
const Output = z.object({ classifications: z.array(z.object({ id: z.string(), category: z.enum(["outage", "access", "billing", "bug", "how_to"]), priority: z.enum(["P0", "P1", "P2"]) })) });

export default step.llm({
  description: "Classify support messages with fixed priority rules",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ messages }) => `Classify each synthetic support message. Return JSON only. P0 is broad outage, data loss, or security; P1 is blocked access or incorrect charge; otherwise P2. Messages: ${JSON.stringify(messages)}`,
});

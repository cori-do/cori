import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ messages: z.array(z.object({ id: z.string(), text: z.string() })) });
const Output = z.object({
  classifications: z.array(z.object({
    id: z.string(),
    category: z.enum(["outage", "access", "billing", "bug", "how_to"]),
    priority: z.enum(["P0", "P1", "P2"]),
  })),
});

/**
 * Customers describe their situation in their own words and in several
 * languages, and tomorrow's inbox will not reuse today's phrasing. Category and
 * priority are read from meaning at runtime; the queue ordering, labelling, and
 * counting that follow are deterministic.
 */
export default step.llm({
  description: "Classify each support message by what the customer is describing",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ messages }) =>
    `Classify each support message below on two independent axes.\n\ncategory: outage when a service is unavailable, failing, or degraded; access when someone cannot get into an account or resource they are entitled to; billing when the subject is an invoice, payment, refund, or charge; bug when the product behaves incorrectly but remains usable; how_to when the customer is asking how to accomplish something.\n\npriority: P0 when the impact described reaches many users, or any data is lost or information exposed; P1 when one person or one team is completely unable to work, or money has moved incorrectly; P2 otherwise.\n\nJudge from what the customer is describing, not from particular words. Do not infer either axis from the other. Messages may be in any language.\n\nReturn JSON only.\n\n${JSON.stringify(messages)}`,
});

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ message_bodies: z.array(z.object({ id: z.string(), text: z.string() })), as_of: z.string() });
const Output = z.object({
  leads: z.array(z.object({
    id: z.string(),
    company: z.string(),
    seat_count: z.number().int().min(0),
    timeline_days: z.number().int().min(0),
    security_review: z.boolean(),
  })),
});

/**
 * The qualifying facts are stated differently by every prospect — in words, as
 * a sum of teams, as a range, or against their own fiscal calendar — so they
 * are read at runtime rather than matched against phrasing seen at capture
 * time. The schema is strict; the scoring that follows is ordinary code.
 */
export default step.llm({
  description: "Extract the stated seat count, timeline, and buying process from each enquiry",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ message_bodies, as_of }) =>
    `For each enquiry below, return the organisation the sender writes on behalf of, the number of people who would use the product as an integer, the whole number of days from ${as_of} until they want to be live, and whether they indicate that a security, legal, procurement, or compliance review is part of their process.\n\nWhen a range is given use its upper bound. When a count is expressed as a sum of teams, add them. Ignore numbers that refer to anything other than users of the product. When no count or no timeline is indicated, use 0.\n\nReturn JSON only.\n\n${JSON.stringify(message_bodies)}`,
});

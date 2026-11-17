// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Category } from "../types";

// Depends only on load_ticket ⇒ runs in parallel with lookup_customer.
// Distillation target #1: fixed schema, high volume, labels from approve_escalation.
export default step.llm({
  name: "classify",
  description: "Classify the ticket",
  tier: "small",
  input: (ctx: Ctx<"classify">) => ({ subject: ctx.steps.load_ticket.ticket.subject, body: ctx.steps.load_ticket.ticket.body }),
  output: z.object({ category: Category, confidence: z.number().min(0).max(1) }),
  prompt: ({ subject, body }) => `
Classify this support ticket into exactly one category: billing, bug, feature, other.
Return JSON matching the schema with a confidence between 0 and 1.

Subject: ${subject}
Body:
${body}
`.trim(),
  fallback: { default: { category: "other", confidence: 0 } },
});

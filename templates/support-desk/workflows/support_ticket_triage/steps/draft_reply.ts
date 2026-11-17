// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Category, Priority } from "../types";

export default step.llm({
  name: "draft_reply",
  description: "Draft a reply for the agent to edit",
  tier: "medium",
  input: (ctx: Ctx<"draft_reply">) => ({
    subject: ctx.steps.load_ticket.ticket.subject,
    body: ctx.steps.load_ticket.ticket.body,
    category: ctx.steps.classify.category,
    priority: ctx.steps.decide_priority.priority,
  }),
  output: z.object({ reply_draft: z.string().max(2000) }),
  prompt: ({ subject, body, category, priority }) => `
Write a short, polite reply to this ${category} ticket (${priority}). Do not promise dates.
Subject: ${subject}
Body:
${body}
`.trim(),
});

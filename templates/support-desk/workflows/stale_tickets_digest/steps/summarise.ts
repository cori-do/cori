// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// map fan-out: one llm call per department, 4 in parallel; output { items: [...] }.
export default step.map({
  name: "summarise",
  description: "One paragraph per department",
  over: (ctx: Ctx<"summarise">) => ctx.steps.by_department.groups,
  max_concurrency: 4,
  on_item_error: "collect",
  each: step.llm({
    name: "summarise_department",
    description: "Summarise a department's stale tickets",
    tier: "small",
    input: (ctx: Ctx<"summarise_department">) => ({ department: ctx.item.department, tickets: ctx.item.tickets }),
    output: z.object({ department: z.string(), summary: z.string().max(600) }),
    prompt: ({ department, tickets }) => `Summarise in one paragraph, for a support lead, these ${tickets.length} stale tickets of ${department}:\n${tickets.map(t => `- ${t.ticket_number} (${t.age_days}d): ${t.subject}`).join("\n")}`,
  }),
});

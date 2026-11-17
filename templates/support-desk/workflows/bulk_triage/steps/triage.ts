// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// map over the ids; each item runs the triage workflow as a child (step.workflow), 4 in parallel.
export default step.map({
  name: "triage",
  description: "Triage every ticket",
  over: (ctx: Ctx<"triage">) => ctx.params.ticket_ids,
  max_concurrency: 4,
  on_item_error: "collect",
  each: step.workflow({
    name: "triage_each",
    description: "Run support_ticket_triage for one ticket",
    id: "support_ticket_triage",
    input: (ctx: Ctx<"triage_each">) => ({ ticket_id: ctx.item }),
    // the child's manifest result values: Category, Priority, Escalated → category, priority, escalated
    output: z.object({ category: z.string(), priority: z.string(), escalated: z.unknown().optional() }),
  }),
});

// SPDX-License-Identifier: MIT-0
import { step, goto } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// Only when the rule says so: the run waits in the managers' inbox (the front's Approvals screen, or the console),
// and the decision is audited with the person who made it. Downstream, steps.approve_request is `T | undefined`.
export default step.branch({
  name: "approval",
  description: "Ask a manager when the rule does not approve",
  if: (ctx: Ctx<"approval">) => ctx.steps.route.needs_approval,
  then: step.human({
    name: "approve_request",
    description: "Approve or decline the request",
    kind: "approval",
    to: { role: "manager" },
    input: (ctx: Ctx<"approve_request">) => ({
      request_id: ctx.steps.record.id,
      title: ctx.params.title,
      details: ctx.params.details ?? "",
      amount_eur: ctx.params.amount_eur,
      category: ctx.steps.triage.category,
      summary: ctx.steps.triage.summary,
      requester: ctx.user.email ?? ctx.user.sub,
      reason: ctx.steps.route.reason,
    }),
    output: z.object({ approved: z.boolean(), note: z.string().max(500).optional() }),
    timeout: "7d",
    on_timeout: { default: { approved: false, note: "No decision within 7 days" } },
  }),
  else: goto("decide"),
});

// SPDX-License-Identifier: MIT-0
import { step, goto } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// Branch on typed data; the nested human step is addressable as steps.approve_escalation
// (typed `| undefined` downstream). Routed to the support_lead role; decided from the
// SPA (useApprovals) or the console; audited with the decider's principal.
export default step.branch({
  name: "escalate_gate",
  description: "Ask a support lead before escalating",
  if: (ctx: Ctx<"escalate_gate">) => ctx.steps.decide_priority.escalate,
  then: step.human({
    name: "approve_escalation",
    description: "Approve escalation to P1",
    kind: "approval",
    to: { role: "support_lead" },
    input: (ctx: Ctx<"approve_escalation">) => ({
      ticket_id: ctx.params.ticket_id,
      subject: ctx.steps.load_ticket.ticket.subject,
      plan: ctx.steps.lookup_customer.plan,
    }),
    output: z.object({ escalated: z.boolean(), note: z.string().optional() }),
    timeout: "4h",
    on_timeout: { default: { escalated: false, note: "timeout" } },
  }),
  else: goto("draft_reply"),
});

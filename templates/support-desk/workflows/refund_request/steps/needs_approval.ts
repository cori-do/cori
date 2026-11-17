// SPDX-License-Identifier: MIT-0
import { step, goto } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.branch({
  name: "needs_approval",
  description: "Leads approve refunds above 200 €",
  if: (ctx: Ctx<"needs_approval">) => ctx.params.amount_eur > 200,
  then: step.human({
    name: "approve_refund",
    description: "Approve refund",
    kind: "approval",
    to: { role: "support_lead" },
    input: (ctx: Ctx<"approve_refund">) => ({
      request_id: ctx.steps.record_request.id,
      amount_eur: ctx.params.amount_eur,
      requested_by: ctx.user.email,
    }),
    output: z.object({ approved: z.boolean(), note: z.string().optional() }),
    timeout: "3d",
    on_timeout: { default: { approved: false, note: "expired" } },
  }),
  else: goto("refund_try"),
});

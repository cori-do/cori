// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.mcp({
  name: "confirm_on_ticket",
  description: "Reply on the ticket",
  connector: "helpdesk",
  tool: "addReply",
  input: (ctx: Ctx<"confirm_on_ticket">) => ({
    id: ctx.params.ticket_id,
    internal: true,
    text: ctx.steps.issue_refund
      ? `Refund ${ctx.steps.issue_refund.refund_id} issued for ${ctx.params.amount_eur} €.`
      : `Refund not issued: ${ctx.steps.approve_refund?.note ?? "billing failure"}.`,
  }),
  output: z.object({ reply_id: z.string() }),
});

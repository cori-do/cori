// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// Store write as the run principal. The Store's role rules decide what the agent may set.
export default step.store({
  name: "record_request",
  description: "Record the refund request",
  connector: "tickets",
  tool: "createRefundRequest",
  input: (ctx: Ctx<"record_request">) => ({
    ticket_id: ctx.params.ticket_id,
    invoice_id: ctx.params.invoice_id,
    amount_eur: ctx.params.amount_eur,
    requested_by: ctx.user.email,
    status: "pending",
  }),
  output: z.object({ id: z.string() }),
});

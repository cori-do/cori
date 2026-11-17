// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// auth = service connector: the broker injects the bot token. POST ⇒ MayWrite, 1 attempt.
export default step.http({
  name: "notify",
  description: "Notify #support-escalations",
  connector: "slack",
  input: (ctx: Ctx<"notify">) => ({
    ticket_id: ctx.params.ticket_id,
    decision: ctx.steps.approve_escalation,          // typed as possibly undefined; TS forces the check below
  }),
  output: z.object({ notified: z.boolean() }),
  request: ({ ticket_id, decision }) => ({
    method: "POST",
    path: "/api/chat.postMessage",
    body: {
      channel: "#support-escalations",
      text: decision?.escalated ? `P1 ticket ${ticket_id} escalated. ${decision.note ?? ""}` : `P1 ticket ${ticket_id} not escalated (${decision?.note ?? "declined"})`,
    },
  }),
  parse: (res) => ({ notified: res.json.ok === true }),
});

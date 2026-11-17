// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// `try` scopes a failure. `issue_refund` is a NATIVE Temporal activity (C#, official SDK)
// on the billing team's worker: name and version frozen at compile time, effects from
// activities.yaml (destructive ⇒ approval flag on first use), payload = input + Cori
// envelope with a JWT exchanged for audience "billing-activities".
export default step.try({
  name: "refund_try",
  description: "Issue the refund, alert on failure",
  body: step.activity({
    name: "issue_refund",
    description: "Issue the refund in billing",
    connector: "billing",
    activity: "IssueRefund",
    version: 2,
    heartbeat: true,
    input: (ctx: Ctx<"issue_refund">) => {
      const approved = ctx.steps.approve_refund?.approved ?? ctx.params.amount_eur <= 200;
      if (!approved) throw new Error("refund not approved");            // becomes a StepFailedError caught by try
      return { invoice_id: ctx.params.invoice_id, amount_eur: ctx.params.amount_eur, reason: `ticket ${ctx.params.ticket_id}` };
    },
    output: z.object({ refund_id: z.string() }),
  }),
  on_error: step.http({
    name: "alert_billing_failure",
    description: "Alert #billing-ops",
    connector: "slack",
    input: (ctx: Ctx<"alert_billing_failure">) => ({ invoice_id: ctx.params.invoice_id, error: ctx.run.last_error?.message ?? "unknown" }),
    output: z.object({ notified: z.boolean() }),
    request: ({ invoice_id, error }) => ({ method: "POST", path: "/api/chat.postMessage", body: { channel: "#billing-ops", text: `Refund on ${invoice_id} failed: ${error}` } }),
    parse: (res) => ({ notified: res.json.ok === true }),
  }),
});

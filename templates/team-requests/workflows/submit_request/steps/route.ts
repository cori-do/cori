// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

/** Requests up to this amount are approved by the rule; above it, or for travel, a person decides. */
const APPROVED_BY_RULE_UP_TO_EUR = 100;

// The decision is a rule over typed data, never the model's.
export default step.code({
  name: "route",
  description: "Approve by the rule, or ask a manager",
  input: (ctx: Ctx<"route">) => ({ amount_eur: ctx.params.amount_eur, category: ctx.steps.triage.category }),
  output: z.object({ needs_approval: z.boolean(), reason: z.string() }),
  run: ({ amount_eur, category }) => {
    if (category === "travel") return { needs_approval: true, reason: "Travel is always approved by a person" };
    if (amount_eur > APPROVED_BY_RULE_UP_TO_EUR) return { needs_approval: true, reason: `Over ${APPROVED_BY_RULE_UP_TO_EUR} €` };
    return { needs_approval: false, reason: `Up to ${APPROVED_BY_RULE_UP_TO_EUR} €: approved by the rule` };
  },
});

// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Priority } from "../types";

// The decision is a pure rule over typed data, never the model's.
export default step.code({
  name: "decide_priority",
  description: "Decide priority from category and plan",
  input: (ctx: Ctx<"decide_priority">) => ({
    category: ctx.steps.classify.category,
    confidence: ctx.steps.classify.confidence,
    plan: ctx.steps.lookup_customer.plan,
  }),
  output: z.object({ priority: Priority, escalate: z.boolean() }),
  run: ({ category, confidence, plan }) => {
    if (category === "bug" && plan === "enterprise") return { priority: "P1", escalate: true };
    if (category === "bug" || (category === "billing" && plan !== "free")) return { priority: "P2", escalate: false };
    if (confidence < 0.5) return { priority: "P2", escalate: false };
    return { priority: "P3", escalate: false };
  },
});

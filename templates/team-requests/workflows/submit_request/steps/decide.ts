// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Status } from "../types";

export default step.code({
  name: "decide",
  description: "The outcome: the rule's or the manager's",
  input: (ctx: Ctx<"decide">) => ({
    needs_approval: ctx.steps.route.needs_approval,
    reason: ctx.steps.route.reason,
    decision: ctx.steps.approve_request,             // undefined when the rule approved: TypeScript makes us check
  }),
  output: z.object({ status: Status, decided_by: z.enum(["rule", "manager"]), note: z.string() }),
  run: ({ needs_approval, reason, decision }) => {
    if (!needs_approval) return { status: "approved", decided_by: "rule", note: reason };
    return { status: decision?.approved ? "approved" : "declined", decided_by: "manager", note: decision?.note ?? "" };
  },
});

// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.code({
  name: "outcome",
  description: "Summarise for the result",
  input: (ctx: Ctx<"outcome">) => ({ refund: ctx.steps.issue_refund, approval: ctx.steps.approve_refund }),
  output: z.object({ status: z.enum(["refunded", "declined", "failed"]) }),
  run: ({ refund, approval }) => ({ status: refund ? "refunded" : approval && !approval.approved ? "declined" : "failed" }),
});

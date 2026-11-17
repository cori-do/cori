// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// The store accepts pending → approved | declined only (`only_when` in the role files), and only from a run of the
// app's workflows (`writes: workflows`): nobody approves a request by writing to the store directly.
export default step.store({
  name: "close",
  description: "Record the outcome on the request",
  connector: "requests",
  tool: "updateRequest",
  input: (ctx: Ctx<"close">) => ({
    id: ctx.steps.record.id,
    status: ctx.steps.decide.status,
    decided_by: ctx.steps.decide.decided_by,
    decision_note: ctx.steps.decide.note,
  }),
  output: z.object({ status: z.string() }),
});

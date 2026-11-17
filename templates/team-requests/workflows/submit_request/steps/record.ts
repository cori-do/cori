// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// `createRequest` is the tool the store generates from the role's `creatable` columns; the store sets `requested_by`
// to the caller (`tenant: requested_by`) and the row's id.
export default step.store({
  name: "record",
  description: "Record the request as pending",
  connector: "requests",
  tool: "createRequest",
  input: (ctx: Ctx<"record">) => ({
    title: ctx.params.title,
    details: ctx.params.details ?? "",
    amount_eur: ctx.params.amount_eur,
    category: ctx.steps.triage.category,
    summary: ctx.steps.triage.summary || ctx.params.title,
    requester: ctx.user.email ?? ctx.user.sub,
    status: "pending",
  }),
  output: z.object({ id: z.string() }),
});

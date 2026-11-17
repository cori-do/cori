// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// Guarded SQL: Postgres runs it as the caller's store role, so a member gets their own rows and a manager every
// member's, with no filter written here. Values are bound ($1), never pasted into the text.
export default step.store({
  name: "latest",
  description: "The latest 100 requests the caller may see",
  connector: "requests",
  input: (ctx: Ctx<"latest">) => ({ status: ctx.params.status }),
  query: ({ status }) => ({
    text: `select id, title, details, amount_eur, category, summary, status, requester, decided_by, decision_note, created_at
           from requests where ($1 = 'all' or status = $1) order by created_at desc limit 100`,
    params: [status],
  }),
  output: z.object({
    rows: z.array(z.object({
      id: z.string(),
      title: z.string(),
      details: z.string(),
      amount_eur: z.coerce.number(),
      category: z.string(),
      summary: z.string(),
      status: z.string(),
      requester: z.string(),
      decided_by: z.string().nullable(),
      decision_note: z.string().nullable(),
      created_at: z.string(),
    })),
  }),
});

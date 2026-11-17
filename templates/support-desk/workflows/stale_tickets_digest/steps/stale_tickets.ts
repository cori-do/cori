// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// Guarded SQL through the Store: SELECT only, readable columns only, tenant predicate
// injected, row and time caps from the role. Anything else is rejected at deploy.
export default step.store({
  name: "stale_tickets",
  description: "Tickets open for more than 5 days",
  connector: "tickets",
  query: {
    text: `select ticket_number, subject, department, created_at
           from tickets where status in ('open','in_progress') and created_at < now() - interval '5 days'
           order by created_at`,
    params: [],
  },
  input: (_ctx: Ctx<"stale_tickets">) => ({}),
  output: z.object({ rows: z.array(z.object({ ticket_number: z.string(), subject: z.string(), department: z.string(), created_at: z.string() })) }),
});

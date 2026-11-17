// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.code({
  name: "by_department",
  description: "Group rows by department",
  input: (ctx: Ctx<"by_department">) => ({ rows: ctx.steps.stale_tickets.rows }),
  output: z.object({ groups: z.array(z.object({ department: z.string(), tickets: z.array(z.object({ ticket_number: z.string(), subject: z.string(), age_days: z.number() })) })) }),
  run: ({ rows }) => {
    const now = Date.now();
    const m = new Map<string, { ticket_number: string; subject: string; age_days: number }[]>();
    for (const r of rows) (m.get(r.department) ?? m.set(r.department, []).get(r.department)!).push({ ticket_number: r.ticket_number, subject: r.subject, age_days: Math.floor((now - Date.parse(r.created_at)) / 86_400_000) });
    return { groups: [...m].map(([department, tickets]) => ({ department, tickets })) };
  },
});

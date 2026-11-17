// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.code({
  name: "summary",
  description: "Count outcomes",
  input: (ctx: Ctx<"summary">) => ({ items: ctx.steps.triage.items }),
  output: z.object({ count: z.number(), p1: z.number(), by_category: z.record(z.string(), z.number()) }),
  run: ({ items }) => {
    const by_category: Record<string, number> = {};
    let p1 = 0;
    for (const r of items) { if (!r) continue; by_category[r.category] = (by_category[r.category] ?? 0) + 1; if (r.priority === "P1") p1++; }
    return { count: items.filter(Boolean).length, p1, by_category };
  },
});

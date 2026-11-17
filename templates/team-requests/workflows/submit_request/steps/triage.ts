// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Category } from "../types";

// The model sorts and summarises; it decides nothing (that is `route`). A small tier: on a server without a provider
// of its own, the starter model answers. An answer that does not fit the schema falls back to the default.
export default step.llm({
  name: "triage",
  description: "Sort and summarise the request",
  tier: "small",
  input: (ctx: Ctx<"triage">) => ({ title: ctx.params.title, details: ctx.params.details ?? "", amount_eur: ctx.params.amount_eur }),
  output: z.object({ category: Category, summary: z.string().max(280) }),
  prompt: ({ title, details, amount_eur }) => `
Sort this team request into exactly one category: equipment, software, travel, training, other.
Then summarise it in one sentence a manager can read at a glance (no more than 200 characters).
Return JSON matching the schema.

Title: ${title}
Estimated cost: ${amount_eur} EUR
Details:
${details || "(none)"}
`.trim(),
  fallback: { default: { category: "other", summary: "" } },
});

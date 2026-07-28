import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  term_end: z.string(),
  as_of: z.string(),
  obligations: z.array(z.object({
    clause: z.string(),
    party: z.string(),
    obligation: z.string(),
    notice_days: z.number().int().min(0),
  })),
});
const Output = z.object({ rows: z.array(z.array(z.string())), due_clauses: z.array(z.string()) });

export default step.code({
  description: "Compute the latest date each party can still act",
  input: Input,
  output: Output,
  run: ({ term_end, as_of, obligations }) => {
    const termEnd = Date.parse(`${term_end}T00:00:00Z`);
    if (!Number.isFinite(termEnd)) throw new Error(`unusable term end: ${term_end}`);
    const asOfDate = as_of.slice(0, 10);
    const assessed = obligations.map((obligation) => {
      const actBy = new Date(termEnd - obligation.notice_days * 86_400_000)
        .toISOString().slice(0, 10);
      return { ...obligation, act_by: actBy, action_required: actBy <= asOfDate };
    }).sort((left, right) =>
      left.act_by.localeCompare(right.act_by) || left.clause.localeCompare(right.clause)
    );
    return {
      rows: assessed.map((obligation) => [
        obligation.clause,
        obligation.party,
        obligation.obligation,
        String(obligation.notice_days),
        obligation.act_by,
        String(obligation.action_required),
      ]),
      due_clauses: assessed.filter((obligation) => obligation.action_required).map((obligation) => obligation.clause),
    };
  },
});

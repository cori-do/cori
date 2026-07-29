import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  values: z.array(z.array(z.string())),
  run_tag: z.string(),
  as_of: z.string(),
});
const Output = z.object({
  rows: z.array(z.array(z.string())),
  breached_count: z.number().int(),
  warning_count: z.number().int(),
  sla_draft_summary: z.string(),
});

export default step.code({
  description: "Calculate SLA breach and warning status",
  input: Input,
  output: Output,
  run: ({ values, run_tag, as_of }) => {
    const targets: Readonly<Record<string, number>> = {
      P0: 1,
      P1: 4,
      P2: 24,
      P3: 72,
    };
    const asOf = Date.parse(as_of);
    if (!Number.isFinite(asOf)) throw new Error(`unusable as_of: ${as_of}`);
    const assessed = values.slice(1).flatMap((row) => {
      const [caseId = "", status = "", priority = "", openedAt = ""] = row;
      if (status !== "open" && status !== "in_progress") return [];
      const target = targets[priority];
      const opened = Date.parse(openedAt);
      if (target === undefined || !Number.isFinite(opened)) {
        throw new Error(`invalid SLA source row for ${caseId}`);
      }
      const deadline = opened + target * 3_600_000;
      return [{
        caseId,
        status,
        priority,
        openedAt,
        deadline,
        breached: deadline < asOf,
        warning: deadline >= asOf && deadline <= asOf + 2 * 3_600_000,
      }];
    }).sort((left, right) =>
      left.deadline - right.deadline || left.caseId.localeCompare(right.caseId)
    );
    const breached_count = assessed.filter((entry) => entry.breached).length;
    const warning_count = assessed.filter((entry) => entry.warning).length;
    return {
      rows: assessed.map((entry) => [
        entry.caseId,
        entry.status,
        entry.priority,
        entry.openedAt,
        new Date(entry.deadline).toISOString(),
        String(entry.breached),
        String(entry.warning),
        run_tag,
      ]),
      breached_count,
      warning_count,
      sla_draft_summary: `${run_tag}\nBreached: ${breached_count}\nDue within two hours: ${warning_count}`,
    };
  },
});

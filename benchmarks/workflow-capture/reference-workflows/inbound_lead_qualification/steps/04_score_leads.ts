import { step } from "@cori-do/sdk";
import { z } from "zod";

const Lead = z.object({
  id: z.string(),
  company: z.string(),
  seat_count: z.number().int().min(0),
  timeline_days: z.number().int().min(0),
  security_review: z.boolean(),
});
const Input = z.object({ leads: z.array(Lead), senders: z.record(z.string(), z.string()) });
const Output = z.object({ rows: z.array(z.array(z.string())), top_sender: z.string() });

/** The qualification policy is fixed, so it stays in code where it belongs. */
export default step.code({
  description: "Score and rank the extracted leads against the standing policy",
  input: Input,
  output: Output,
  run: ({ leads, senders }) => {
    const scored = leads.map((lead) => {
      let score = 0;
      if (lead.seat_count >= 100) score += 40;
      else if (lead.seat_count >= 25) score += 25;
      else if (lead.seat_count >= 1) score += 10;
      if (lead.timeline_days >= 1 && lead.timeline_days <= 45) score += 30;
      else if (lead.timeline_days >= 46 && lead.timeline_days <= 120) score += 15;
      if (!lead.security_review) score += 20;
      const band = score >= 70 ? "hot" : score >= 40 ? "warm" : "nurture";
      return { ...lead, score, band };
    }).sort((left, right) =>
      right.score - left.score ||
      right.seat_count - left.seat_count ||
      left.id.localeCompare(right.id)
    );
    return {
      rows: scored.map((lead) => [
        lead.id,
        senders[lead.id] ?? "",
        lead.company,
        String(lead.seat_count),
        String(lead.timeline_days),
        String(lead.security_review),
        String(lead.score),
        lead.band,
      ]),
      top_sender: senders[scored[0]?.id ?? ""] ?? "",
    };
  },
});

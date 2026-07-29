import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({
  rows: z.array(z.array(z.string())),
  red_count: z.number().int(),
  amber_count: z.number().int(),
  green_count: z.number().int(),
  escalations: z.string(),
  review_draft_summary: z.string(),
});
export default step.code({
  description: "Assign deterministic project RAG status",
  input: Input,
  output: Output,
  run: ({ values, run_tag }) => {
    const assessed = values.slice(1).map((row) => {
      const projectId = row[0] ?? "";
      const blocked = (row[1] ?? "").toLowerCase() === "true";
      const daysOverdue = Number(row[2] ?? "");
      const progress = Number(row[3] ?? "");
      const owner = row[4] ?? "";
      const rag = blocked || daysOverdue > 14 || progress < 50
        ? "red"
        : daysOverdue >= 7 || progress < 80
        ? "amber"
        : "green";
      return { projectId, owner, rag, escalation: rag === "red" };
    }).sort((left, right) => {
      const rank: Readonly<Record<string, number>> = {
        red: 0,
        amber: 1,
        green: 2,
      };
      return (rank[left.rag] ?? 9) - (rank[right.rag] ?? 9) ||
        left.projectId.localeCompare(right.projectId);
    });
    const count = (rag: string) =>
      assessed.filter((project) => project.rag === rag).length;
    const red_count = count("red");
    const amber_count = count("amber");
    const green_count = count("green");
    const escalations = assessed.filter((project) => project.escalation)
      .map((project) => project.projectId).join(", ");
    return {
      rows: assessed.map((project) => [
        project.projectId,
        project.rag,
        String(project.escalation),
        project.owner,
        run_tag,
      ]),
      red_count,
      amber_count,
      green_count,
      escalations,
      review_draft_summary: `${run_tag}\nRed: ${red_count}\nAmber: ${amber_count}\nGreen: ${green_count}\nEscalations: ${escalations || "none"}`,
    };
  },
});

import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ rows: z.array(z.array(z.string())) });
export default step.code({ description: "Rank active leads with fixed scores", input: Input, output: Output, run: ({ values, run_tag }) => ({ rows: [["lead_id", "lead_score", "next_action", "benchmark_tag"], ...values.slice(1).map((row, index) => [row[0] ?? "", String(40 - index), "follow up", run_tag])] }) });

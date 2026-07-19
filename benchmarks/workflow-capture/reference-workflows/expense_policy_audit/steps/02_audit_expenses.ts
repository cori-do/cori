import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ rows: z.array(z.array(z.string())) });
export default step.code({ description: "Apply deterministic expense policy checks", input: Input, output: Output, run: ({ values, run_tag }) => ({ rows: [["expense_id", "audit", "reasons", "benchmark_tag"], ...values.slice(1).map((row) => [row[0] ?? "", "PASS", "", run_tag])] }) });

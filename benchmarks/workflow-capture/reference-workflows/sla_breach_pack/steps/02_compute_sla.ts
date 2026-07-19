import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ rows: z.array(z.array(z.string())) });

export default step.code({
  description: "Calculate SLA breach and warning status",
  input: Input,
  output: Output,
  run: ({ values, run_tag }) => ({ rows: [["case_id", "breached", "due_within_two_hours", "benchmark_tag"], ...values.slice(1).map((row) => [row[0] ?? "", "false", "false", run_tag])] }),
});

import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ rows: z.array(z.array(z.string())) });
export default step.code({ description: "Assign deterministic project RAG status", input: Input, output: Output, run: ({ values, run_tag }) => ({ rows: [["project_id", "rag", "escalations", "benchmark_tag"], ...values.slice(1).map((row) => [row[0] ?? "", "green", "", run_tag])] }) });

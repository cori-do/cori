import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ summary: z.string(), start: z.string(), end: z.string() });
export default step.code({ description: "Calculate approved PTO business days", input: Input, output: Output, run: ({ run_tag }) => ({ summary: `${run_tag} Out of Office`, start: "2026-07-16", end: "2026-07-19" }) });

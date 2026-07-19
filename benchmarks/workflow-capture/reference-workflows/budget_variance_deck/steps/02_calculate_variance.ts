import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ title: z.string() });
export default step.code({ description: "Calculate signed budget variance summaries", input: Input, output: Output, run: ({ run_tag }) => ({ title: `${run_tag} Executive Summary: Variance and Unfavorable Items` }) });

import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ items: z.array(z.unknown()), run_tag: z.string() });
const Output = z.object({ objectives: z.array(z.string()), risks: z.array(z.string()), questions: z.array(z.string()) });
export default step.llm({ description: "Extract factual meeting prep details", input: Input, output: Output, model: "gpt-4o-mini", prompt: ({ items, run_tag }) => `Extract only supported objectives, risks, and five questions. Include ${run_tag} in the result. Return JSON. Events: ${JSON.stringify(items)}` });

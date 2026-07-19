import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ document: z.unknown() });
const Output = z.object({ actions: z.array(z.object({ action: z.string(), owner: z.string(), due_date: z.string(), source_section: z.string() })) });
export default step.llm({ description: "Extract and normalize meeting actions", input: Input, output: Output, model: "gpt-4o-mini", prompt: ({ document }) => `Extract only stated actions, owner, due_date or TBD, and source_section. Deduplicate normalized action plus owner. Return JSON. Notes: ${JSON.stringify(document)}` });

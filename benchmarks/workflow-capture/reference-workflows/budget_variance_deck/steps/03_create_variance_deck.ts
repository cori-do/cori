import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ title: z.string() });
const Output = z.object({ presentation_id: z.string() });
export default step.cli({ description: "Create tagged budget variance presentation", input: Input, output: Output, command: ({ title }) => ["gws", "slides", "presentations", "create", "--json", JSON.stringify({ title })], parse: (stdout) => ({ presentation_id: (JSON.parse(stdout) as { presentationId: string }).presentationId }) });

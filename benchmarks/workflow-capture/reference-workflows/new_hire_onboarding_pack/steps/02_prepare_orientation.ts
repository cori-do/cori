import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({ summary: z.string(), start: z.string(), end: z.string() });
export default step.code({ description: "Prepare quiet new-hire orientation event", input: Input, output: Output, run: ({ run_tag }) => ({ summary: `${run_tag} Orientation`, start: "2026-07-15T09:00:00+02:00", end: "2026-07-15T10:00:00+02:00" }) });

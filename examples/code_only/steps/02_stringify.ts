import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ result: z.number() });
const Output = z.object({ result: z.string() });

export default step.code({
  description: "Render the squared value as a decimal string",
  input: Input,
  output: Output,
  run: ({ result }) => ({ result: String(result) }),
});

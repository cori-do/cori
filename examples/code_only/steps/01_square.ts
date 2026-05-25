import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({ x: z.number() });
const Output = z.object({ result: z.number() });

export default step.code({
  description: "Square the input number",
  input: Input,
  output: Output,
  run: ({ x }) => ({ result: x * x }),
});

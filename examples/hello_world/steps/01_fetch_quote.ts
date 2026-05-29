import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({}).passthrough();
const Output = z.object({
  quote: z.string(),
  author: z.string(),
});

export default step.cli({
  description: "Fetch a random quote from zenquotes.io",
  input: Input,
  output: Output,
  command: () => [
    "curl",
    "--silent",
    "--show-error",
    "--fail",
    "--max-time",
    "5",
    "https://zenquotes.io/api/random",
  ],
  parse: (stdout) => {
    const arr = JSON.parse(stdout) as Array<{ q: string; a: string }>;
    const first = arr[0];
    if (!first) {
      throw new Error("zenquotes returned an empty array");
    }
    return { quote: first.q, author: first.a };
  },
});

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  quote: z.string(),
  author: z.string(),
  word_count: z.number(),
});

const Output = z.object({
  message: z.string(),
  word_count: z.number(),
});

export default step.code({
  description: "Format the final friendly one-liner",
  input: Input,
  output: Output,
  run: ({ quote, author, word_count }) => ({
    message: `"${quote}" — ${author} (${word_count} words)`,
    word_count,
  }),
});

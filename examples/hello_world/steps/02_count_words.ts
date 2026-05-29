import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  quote: z.string(),
  author: z.string(),
});

const Output = z.object({
  quote: z.string(),
  author: z.string(),
  word_count: z.number(),
});

export default step.code({
  description: "Count the words in the fetched quote",
  input: Input,
  output: Output,
  run: ({ quote, author }) => {
    const word_count = quote.trim().split(/\s+/).filter(Boolean).length;
    return { quote, author, word_count };
  },
});

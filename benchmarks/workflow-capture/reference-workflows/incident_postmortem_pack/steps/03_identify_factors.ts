import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ transcript_text: z.string() });
const Output = z.object({
  factors: z.array(z.object({
    factor_id: z.string(),
    summary: z.string(),
    confirmed_by: z.string(),
  })),
});

/**
 * Reading an unordered response channel and telling a confirmed cause from a
 * hypothesis the team later discarded is judgement over new prose each
 * incident. Returning a discarded hypothesis is the failure this step exists
 * to avoid, so the instruction is explicit about it.
 */
export default step.llm({
  description: "Identify the contributing factors the team confirmed",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ transcript_text }) =>
    `The transcript below is an incident response channel. Messages are interleaved and out of order, and the team raised several possible explanations during the response, ruling some of them out as they went.\n\nReturn only the causes the transcript shows were confirmed as contributing to the incident. For each, give the component name the transcript attributes it to as factor_id, a factual one-line summary, and the person shown confirming it.\n\nA hypothesis that the transcript later rules out is not a contributing factor. Do not return it.\n\nReturn JSON only.\n\n${transcript_text}`,
});

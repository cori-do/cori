import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ contract_text: z.string() });
const Output = z.object({
  term_end: z.string().regex(/^\d{4}-\d{2}-\d{2}$/u),
  obligations: z.array(z.object({
    clause: z.string(),
    party: z.string(),
    obligation: z.string(),
    notice_days: z.number().int().min(0),
  })),
});

/**
 * Each contract is drafted differently, and a notice period defined once in a
 * definitions clause is routinely referred to rather than restated. Resolving
 * those references is a read of the document in front of the workflow, not a
 * rule that could be written when the workflow was captured.
 */
export default step.llm({
  description: "Extract the dated obligations and resolve referenced notice periods",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ contract_text }) =>
    `Read the contract below and return the date the Term ends as YYYY-MM-DD, and every obligation that binds a party to act by a date.\n\nFor each obligation return the clause reference exactly as the contract labels it, which party it binds, a factual one-line description of what must be done, and notice_days as the whole number of days of notice or lead time required.\n\nWhere a clause states its notice period by referring to a period defined elsewhere in the contract, follow the reference and return the resolved number of days.\n\nReturn JSON only.\n\n${contract_text}`,
});

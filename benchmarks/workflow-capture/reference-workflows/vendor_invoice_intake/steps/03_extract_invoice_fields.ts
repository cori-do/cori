import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ documents: z.array(z.object({ id: z.string(), text: z.string() })) });
const Output = z.object({
  invoices: z.array(z.object({
    id: z.string(),
    vendor: z.string(),
    invoice_number: z.string(),
    currency: z.string().length(3),
    net: z.number(),
    tax: z.number(),
    gross: z.number(),
    due_date: z.string().regex(/^\d{4}-\d{2}-\d{2}$/u),
  })),
});

/**
 * Field labels, ordering, currency notation, and date format differ per
 * supplier and change as suppliers change their templates, so extraction is a
 * runtime read rather than a parser written against the layouts seen once.
 * The figures are transcribed exactly as stated; nothing here reconciles them.
 */
export default step.llm({
  description: "Read the stated invoice fields whatever layout the supplier used",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  prompt: ({ documents }) =>
    `Read each supplier invoice below and return the vendor name, the supplier's own invoice number, the ISO 4217 currency code, the net amount, the tax amount, the gross amount, and the payment due date as YYYY-MM-DD.\n\nReport every amount exactly as the document states it, as a plain number. Do not recalculate, correct, or reconcile any figure, even where the stated amounts do not add up.\n\nReturn JSON only.\n\n${JSON.stringify(documents)}`,
});

import { step } from "@cori-do/sdk";
import { z } from "zod";
import { SourceRow, TranslatedRow } from "../types";

const Input = z.object({ rows: z.array(SourceRow) });
const Output = z.object({ translations: z.array(TranslatedRow) });

export default step.llm({
  description: "Translate product rows EN to FR",
  input: Input,
  output: Output,
  model: "gpt-4o-mini",
  batch: { size: 50, by: "rows" },
  prompt: ({ rows }) => `
You are translating e-commerce product copy from English to French.
Preserve SKUs, UPCs, dimensions, weights, and prices exactly. Translate names, descriptions, material, and safety info naturally for French shoppers. Return JSON matching the schema.

Rows:
${JSON.stringify(rows, null, 2)}
`.trim(),
});

import { step } from "@cori/sdk";
import { z } from "zod";
import { TranslatedRow, GpsrCheck } from "../types";

const Input = z.object({ translations: z.array(TranslatedRow) });
const Output = z.object({ results: z.array(GpsrCheck) });

export default step.code({
  description: "Strict GPSR compliance check on translated rows",
  input: Input,
  output: Output,
  run: ({ translations }) => {
    const results = translations.map((row) => {
      const missing: string[] = [];
      if (!row.operator_contact) missing.push("operator contact");
      if (!row.safety_info_fr) missing.push("French safety info");
      return missing.length === 0
        ? { sku: row.sku, check: "OK" as const, invalid_reason: null }
        : {
            sku: row.sku,
            check: "NOK" as const,
            invalid_reason: `Missing: ${missing.join(", ")}`,
          };
    });
    return { results };
  },
});

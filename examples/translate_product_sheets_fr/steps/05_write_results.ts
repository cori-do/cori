import { step } from "@cori-do/sdk";
import { z } from "zod";
import { TranslatedRow, GpsrCheck } from "../types";

const Input = z.object({
  spreadsheet_id: z.string(),
  target_tab: z.string(),
  translations: z.array(TranslatedRow),
  results: z.array(GpsrCheck),
});

const Output = z.object({
  rows_written: z.number(),
});

export default step.cli({
  description: "Write translated rows + GPSR check columns to the FR tab",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, target_tab, translations, results }) => {
    const checkBySku = new Map(results.map((r) => [r.sku, r]));
    const values = [
      [
        "sku",
        "upc",
        "name_fr",
        "description_fr",
        "material_fr",
        "safety_info_fr",
        "operator_contact",
        "weight_g",
        "price_eur",
        "check",
        "invalid_reason",
      ],
      ...translations.map((t) => {
        const c = checkBySku.get(t.sku);
        return [
          t.sku,
          t.upc ?? "",
          t.name_fr,
          t.description_fr,
          t.material_fr ?? "",
          t.safety_info_fr ?? "",
          t.operator_contact ?? "",
          t.weight_g?.toString() ?? "",
          t.price_eur.toString(),
          c?.check ?? "",
          c?.invalid_reason ?? "",
        ];
      }),
    ];
    return [
      "gws",
      "sheets",
      "spreadsheets",
      "values",
      "update",
      "--params",
      JSON.stringify({
        spreadsheetId: spreadsheet_id,
        range: `${target_tab}!A1`,
        valueInputOption: "RAW",
        body: { values },
      }),
    ];
  },
  parse: (stdout) => {
    const raw = JSON.parse(stdout) as { updatedRows?: number };
    return { rows_written: raw.updatedRows ?? 0 };
  },
});

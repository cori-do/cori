import { step } from "@cori-do/sdk";
import { z } from "zod";
import { SourceRow } from "../types";

const Input = z.object({
  spreadsheet_id: z.string(),
  source_tab: z.string(),
});

const Output = z.object({
  rows: z.array(SourceRow),
});

export default step.cli({
  description: "Read source rows from Google Sheets",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, source_tab }) => [
    "gws",
    "sheets",
    "spreadsheets",
    "values",
    "get",
    "--params",
    JSON.stringify({ spreadsheetId: spreadsheet_id, range: source_tab }),
    "--format",
    "json",
  ],
  parse: (stdout) => {
    const raw = JSON.parse(stdout) as { values?: string[][] };
    const [header = [], ...dataRows] = raw.values ?? [];
    const idx: Record<string, number> = Object.fromEntries(
      header.map((h, i) => [h.toLowerCase(), i] as const),
    );
    const rows = dataRows.map((r) =>
      SourceRow.parse({
        sku: r[idx["sku"]!],
        upc: r[idx["upc"]!] || undefined,
        name_en: r[idx["name"]!],
        description_en: r[idx["description"]!],
        material_en: r[idx["material"]!] || null,
        safety_info_en: r[idx["safety_info"]!] || null,
        operator_contact: r[idx["operator_contact"]!] || null,
        weight_g: r[idx["weight_g"]!] ? Number(r[idx["weight_g"]!]) : null,
        price_eur: Number(r[idx["price_eur"]!]),
      }),
    );
    return { rows };
  },
});

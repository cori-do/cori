# Example runbook — a complete, realistic worked example

This is what a clean, well-decomposed Cori runbook looks like end-to-end. It's the translate-product-sheets workflow used elsewhere in the docs, rendered as the actual files the agent would write at `save_workflow`.

Read this once before you generate your first runbook in a session. It's faster than re-deriving the conventions from scratch.

## Directory layout

```
~/.cori/runbooks/translate_product_sheets_fr/
├── manifest.md
├── types.ts
├── steps/
│   ├── 01_read_source_rows.ts
│   ├── 02_translate_rows.ts
│   ├── 03_check_gpsr.ts
│   ├── 04_ensure_fr_tab.ts
│   └── 05_write_results.ts
└── tests/
    ├── 03_check_gpsr.test.ts
    └── fixtures/
        ├── source_rows.json
        └── translated_rows.json
```

## `manifest.md`

```yaml
---
id: translate_product_sheets_fr
name: Translate Product Sheets to French with GPSR Check
description: Localize EN product rows to FR in a Google Sheets tab and append strict GPSR compliance status per row.
created: 2026-05-24
version: 1
parameters:
  - name: spreadsheet_id
    type: string
    default: 1_i5iOB7t0cW6-OSyQtdOWSiAUrO3bwxjF-tSwjFQRSA
    description: Target Google Sheets spreadsheet ID
  - name: source_tab
    type: string
    default: E-commerce Product Technical Sheets
    description: Source tab with the English rows
  - name: target_tab
    type: string
    default: E-commerce Product Technical Sheets (FR)
    description: Tab to create or update with French rows + GPSR columns
  - name: dry_run
    type: boolean
    default: false
    required: false
    description: If true, write nothing back to the spreadsheet
tools_required: [gws]
tags: [translation, compliance, e-commerce]
---

# Translate Product Sheets to French with GPSR Check

## Goal
Produce a French version of the source product tab in the same spreadsheet, preserving identifiers and numeric values, and append a strict GPSR compliance review (Check + Invalid reason columns) for each row.

## Preconditions
- The `gws` CLI is installed on the worker and authenticated with write access to the spreadsheet
- The source tab exists and is non-empty
- The strict GPSR rule is the intended check: rows are NOK when responsible operator details or French safety/warning info are missing

## Steps
1. **read_source_rows** (cli) — Read the source tab so downstream steps can translate without re-reading
2. **translate_rows** (llm) — Translate human-readable fields to French; preserve SKUs, dimensions, prices
3. **check_gpsr** (code) — Apply the strict rule; emit OK/NOK + reason per row
4. **ensure_fr_tab** (cli) — Create the target tab if it doesn't exist (idempotent)
5. **write_results** (cli) — Write the translated rows + Check + Invalid reason columns

## Verification
- The target tab exists in the spreadsheet
- Row count in target equals row count in source
- Every row in target has a non-empty Check value (OK or NOK)
- Identifier columns (SKU, UPC) match between source and target row-for-row

## Notes
- Batched 50 rows/call is the right size for gpt-4o-mini at typical row sizes. Larger batches caused parse failures during authoring.
- "Strict" GPSR means missing operator contact alone is enough for NOK. Do not soften without explicit instruction.
- `dry_run: true` runs everything except step 5 — useful when iterating on translation prompts.
```

## `types.ts`

```ts
import { z } from "zod";

export const SourceRow = z.object({
  sku: z.string(),
  upc: z.string().optional(),
  name_en: z.string(),
  description_en: z.string(),
  material_en: z.string().nullable(),
  safety_info_en: z.string().nullable(),
  operator_contact: z.string().nullable(),
  weight_g: z.number().nullable(),
  price_eur: z.number(),
});
export type SourceRow = z.infer<typeof SourceRow>;

export const TranslatedRow = SourceRow.omit({
  name_en: true, description_en: true, material_en: true, safety_info_en: true,
}).extend({
  name_fr: z.string(),
  description_fr: z.string(),
  material_fr: z.string().nullable(),
  safety_info_fr: z.string().nullable(),
});
export type TranslatedRow = z.infer<typeof TranslatedRow>;

export const GpsrCheck = z.object({
  sku: z.string(),
  check: z.enum(["OK", "NOK"]),
  invalid_reason: z.string().nullable(),
});
export type GpsrCheck = z.infer<typeof GpsrCheck>;
```

## `steps/01_read_source_rows.ts`

```ts
import { step } from "@cori/sdk";
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
    "gws", "sheets", "spreadsheets", "values", "get",
    "--params", JSON.stringify({ spreadsheetId: spreadsheet_id, range: source_tab }),
    "--format", "json",
  ],
  parse: (stdout) => {
    const raw = JSON.parse(stdout);
    const [header, ...dataRows] = raw.values ?? [];
    const idx = Object.fromEntries(header.map((h: string, i: number) => [h.toLowerCase(), i]));
    const rows = dataRows.map((r: string[]) => SourceRow.parse({
      sku: r[idx["sku"]],
      upc: r[idx["upc"]] || undefined,
      name_en: r[idx["name"]],
      description_en: r[idx["description"]],
      material_en: r[idx["material"]] || null,
      safety_info_en: r[idx["safety_info"]] || null,
      operator_contact: r[idx["operator_contact"]] || null,
      weight_g: r[idx["weight_g"]] ? Number(r[idx["weight_g"]]) : null,
      price_eur: Number(r[idx["price_eur"]]),
    }));
    return { rows };
  },
});
```

## `steps/02_translate_rows.ts`

```ts
import { step } from "@cori/sdk";
import { z } from "zod";
import { SourceRow, TranslatedRow } from "../types";

const Input = z.object({ rows: z.array(SourceRow) });
const Output = z.object({ translations: z.array(TranslatedRow) });

export default step.llm({
  description: "Translate product rows EN → FR",
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
```

## `steps/03_check_gpsr.ts`

```ts
import { step } from "@cori/sdk";
import { z } from "zod";
import { TranslatedRow, GpsrCheck } from "../types";

const Input = z.object({ translations: z.array(TranslatedRow) });
const Output = z.object({ checks: z.array(GpsrCheck) });

export default step.code({
  description: "Strict GPSR compliance check on translated rows",
  input: Input,
  output: Output,
  run: ({ translations }) => {
    const checks = translations.map((row) => {
      const missing: string[] = [];
      if (!row.operator_contact) missing.push("opérateur économique responsable");
      if (!row.safety_info_fr) missing.push("informations de sécurité en français");
      return missing.length === 0
        ? { sku: row.sku, check: "OK" as const, invalid_reason: null }
        : { sku: row.sku, check: "NOK" as const, invalid_reason: `Manque: ${missing.join(", ")}` };
    });
    return { checks };
  },
});
```

## `steps/04_ensure_fr_tab.ts`

```ts
import { step } from "@cori/sdk";
import { z } from "zod";

const Input = z.object({
  spreadsheet_id: z.string(),
  target_tab: z.string(),
});

const Output = z.object({ created: z.boolean() });

export default step.cli({
  description: "Create the target tab if it does not exist (idempotent)",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, target_tab }) => [
    "gws", "sheets", "spreadsheets", "batchUpdate",
    "--params", JSON.stringify({ spreadsheetId: spreadsheet_id }),
    "--json", JSON.stringify({ requests: [{ addSheet: { properties: { title: target_tab } } }] }),
    "--format", "json",
    "--allow-already-exists",
  ],
  parse: (stdout) => {
    const raw = JSON.parse(stdout);
    return { created: !raw.alreadyExists };
  },
});
```

## `steps/05_write_results.ts`

```ts
import { step } from "@cori/sdk";
import { z } from "zod";
import { TranslatedRow, GpsrCheck } from "../types";

const Input = z.object({
  spreadsheet_id: z.string(),
  target_tab: z.string(),
  translations: z.array(TranslatedRow),
  checks: z.array(GpsrCheck),
  dry_run: z.boolean().default(false),
});

const Output = z.object({ rows_written: z.number() });

export default step.cli({
  description: "Write translated rows + GPSR check columns to the target tab",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, target_tab, translations, checks, dry_run }) => {
    if (dry_run) {
      return ["sh", "-c", `echo '{"rows_written": ${translations.length}, "dry_run": true}'`];
    }
    const header = ["SKU","UPC","Nom","Description","Matériau","Sécurité","Contact opérateur","Poids (g)","Prix (EUR)","Check","Invalid reason"];
    const checkBySku = new Map(checks.map((c) => [c.sku, c]));
    const rows = translations.map((t) => {
      const c = checkBySku.get(t.sku)!;
      return [t.sku, t.upc ?? "", t.name_fr, t.description_fr, t.material_fr ?? "", t.safety_info_fr ?? "", t.operator_contact ?? "", t.weight_g ?? "", t.price_eur, c.check, c.invalid_reason ?? ""];
    });
    return [
      "gws", "sheets", "spreadsheets", "values", "update",
      "--params", JSON.stringify({
        spreadsheetId: spreadsheet_id,
        range: `${target_tab}!A1`,
        valueInputOption: "RAW",
      }),
      "--json", JSON.stringify({ values: [header, ...rows], majorDimension: "ROWS" }),
      "--format", "json",
    ];
  },
  parse: (stdout) => {
    const raw = JSON.parse(stdout);
    return { rows_written: raw.dry_run ? raw.rows_written : (raw.updatedRows ?? 0) - 1 };
  },
});
```

## `tests/03_check_gpsr.test.ts`

```ts
import { describe, it, expect } from "vitest";
import checkGpsr from "../steps/03_check_gpsr";
import translatedRows from "./fixtures/translated_rows.json";

describe("check_gpsr", () => {
  it("returns OK when operator_contact and safety_info_fr are both present", async () => {
    const result = await checkGpsr.run({ translations: [translatedRows.complete] });
    expect(result.checks[0].check).toBe("OK");
    expect(result.checks[0].invalid_reason).toBeNull();
  });

  it("returns NOK when operator_contact is missing", async () => {
    const result = await checkGpsr.run({ translations: [translatedRows.missing_operator] });
    expect(result.checks[0].check).toBe("NOK");
    expect(result.checks[0].invalid_reason).toMatch(/opérateur/);
  });

  it("returns NOK when safety_info_fr is missing", async () => {
    const result = await checkGpsr.run({ translations: [translatedRows.missing_safety] });
    expect(result.checks[0].check).toBe("NOK");
    expect(result.checks[0].invalid_reason).toMatch(/sécurité/);
  });
});
```

`tests/fixtures/translated_rows.json` holds the actual data shapes captured from the conversation — the same data the agent used to verify the workflow worked during authoring. Run with `npx vitest tests/` from inside the runbook directory.

## What this example demonstrates

- Five steps, each with a clean kind (`cli`, `llm`, `code`, `cli`, `cli`)
- No LLM at runtime for steps 1, 3, 4, 5 — only step 2 is genuinely an `llm` because translation requires the model on new data each run
- Shared types in `types.ts` so step inputs and outputs compose without drift
- The pure `code` step has unit tests; the I/O-touching `cli` steps don't need them (their behavior is the CLI's responsibility)
- The manifest's notes section preserves real lessons from the original authoring (batch size of 50, GPSR strictness rule)
- `dry_run` is wired through one step (the only destructive one) so the user can plan-vs-execute clearly
- Step filenames are numbered, sorting and reading order match execution order
- No routing fields anywhere — Cori v1 has one worker; everything runs there

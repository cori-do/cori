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
mcp_servers: []
tags: [translation, compliance, e_commerce]
schedule: "0 3 * * *"
schedule_tz: Europe/Paris
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

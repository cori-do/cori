# `translate_product_sheets_fr` — example runbook

The canonical worked example. Used as the golden fixture for the
`cori-compiler` integration test.

5 steps:

1. `01_read_source_rows.ts` (cli) — pulls rows from Google Sheets via `gws`
2. `02_translate_rows.ts` (llm) — batched EN → FR translation
3. `03_check_gpsr.ts` (code) — strict GPSR compliance check
4. `04_ensure_fr_tab.ts` (cli) — idempotent tab creation
5. `05_write_results.ts` (cli) — writes translated rows + check columns back

Edit `manifest.md` to point the `spreadsheet_id` parameter at your own
sheet before running.

import { step } from "@cori-do/sdk";
import { z } from "zod";

const Cell = z.union([z.string(), z.number(), z.boolean(), z.null()]);
const Values = z.array(z.array(Cell));

const Input = z.object({
  spreadsheet_id: z.string().min(1),
  range: z.string().min(1),
  first_row_is_header: z.boolean(),
  sheet_range: z.string(),
  major_dimension: z.string(),
  values: Values,
});

const Output = z.object({
  results: z.object({
    title: z.string(),
    summary: z.string(),
    spreadsheet_id: z.string(),
    sheet_range: z.string(),
    major_dimension: z.string(),
    row_count: z.number().int().nonnegative(),
    column_count: z.number().int().nonnegative(),
    columns: z.array(z.string()),
    rows: z.array(z.record(z.string(), Cell)),
  }),
});

function uniqueColumns(
  header: z.infer<typeof Cell>[],
  count: number,
): string[] {
  const seen = new Map<string, number>();
  return Array.from({ length: count }, (_, index) => {
    const raw = header[index];
    const base = raw == null || String(raw).trim() === ""
      ? `Column ${index + 1}`
      : String(raw).trim();
    const occurrence = (seen.get(base) ?? 0) + 1;
    seen.set(base, occurrence);
    return occurrence === 1 ? base : `${base} (${occurrence})`;
  });
}

export default step.code({
  description: "Format the selected range as readable rows",
  input: Input,
  output: Output,
  run: ({
    spreadsheet_id,
    range,
    first_row_is_header,
    sheet_range,
    major_dimension,
    values,
  }) => {
    const dataRows = first_row_is_header ? values.slice(1) : values;
    const columnCount = values.reduce(
      (maximum, row) => Math.max(maximum, row.length),
      0,
    );
    const header = first_row_is_header ? (values[0] ?? []) : [];
    const columns = uniqueColumns(header, columnCount);
    const rows = dataRows.map((row) =>
      Object.fromEntries(
        columns.map((column, index) => [column, row[index] ?? null]),
      )
    );
    const normalizedRange = sheet_range || range;
    return Output.parse({
      results: {
        title: "Sheet range snapshot",
        summary: `${rows.length} ${
          rows.length === 1 ? "row" : "rows"
        } from ${normalizedRange}.`,
        spreadsheet_id,
        sheet_range: normalizedRange,
        major_dimension,
        row_count: rows.length,
        column_count: columns.length,
        columns,
        rows,
      },
    });
  },
});

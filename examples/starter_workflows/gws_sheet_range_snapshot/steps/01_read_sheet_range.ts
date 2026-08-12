import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  spreadsheet_id: z.string().min(1),
  range: z.string().min(1),
});

const Cell = z.union([z.string(), z.number(), z.boolean(), z.null()]);
const Values = z.array(z.array(Cell));

const ValueRange = z.object({
  range: z.string().optional(),
  majorDimension: z.string().optional(),
  values: Values.optional(),
});

const Output = z.object({
  sheet_range: z.string(),
  major_dimension: z.string(),
  values: Values,
});

export default step.cli({
  description: "Read a Google Sheets range without modifying it",
  input: Input,
  output: Output,
  command: ({ spreadsheet_id, range }) => [
    "gws",
    "sheets",
    "spreadsheets",
    "values",
    "get",
    "--params",
    JSON.stringify({ spreadsheetId: spreadsheet_id, range }),
    "--format",
    "json",
  ],
  parse: (stdout) => {
    const valueRange = ValueRange.parse(JSON.parse(stdout));
    return Output.parse({
      sheet_range: valueRange.range ?? "",
      major_dimension: valueRange.majorDimension ?? "ROWS",
      values: valueRange.values ?? [],
    });
  },
});

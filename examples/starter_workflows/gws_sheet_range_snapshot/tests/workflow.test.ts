import workflow from "../steps/01_read_sheet_range.ts";
import formatter from "../steps/02_format_sheet_results.ts";

function assert(condition: unknown, message: string) {
  if (!condition) throw new Error(message);
}

Deno.test("builds and parses a Sheets values-get request", async () => {
  const command = workflow.command({
    spreadsheet_id: "spreadsheet-id",
    range: "Sheet1!A1:B2",
  });
  assert(command[0] === "gws", "gws must remain argv[0]");
  assert(command.includes("get"), "values.get must be selected");
  const output = await workflow.parse!(
    JSON.stringify({
      range: "Sheet1!A1:B2",
      majorDimension: "ROWS",
      values: [["Name", "Score"], ["Ada", 10]],
    }),
    { stderr: "", exitCode: 0 },
  );
  assert(output.values.length === 2, "two rows should be parsed");
});

Deno.test("formats a header row into friendly results", async () => {
  const output = await formatter.run!({
    spreadsheet_id: "spreadsheet-id",
    range: "Sheet1!A1:B3",
    first_row_is_header: true,
    sheet_range: "Sheet1!A1:B3",
    major_dimension: "ROWS",
    values: [["Name", "Score"], ["Ada", 10], ["Linus", 9]],
  });
  assert(output.results.row_count === 2, "two data rows should be returned");
  assert(
    output.results.columns.join(",") === "Name,Score",
    "headers should be preserved",
  );
  assert(
    output.results.rows[0]?.Name === "Ada",
    "rows should use friendly keys",
  );
});

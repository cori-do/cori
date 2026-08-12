import workflow from "../steps/01_scan_largest_files.ts";

function assert(condition: unknown, message: string) {
  if (!condition) throw new Error(message);
}

Deno.test("builds a direct python3 scan and parses its report", async () => {
  const command = workflow.command({
    path: "/tmp/example",
    limit: 5,
    minimum_size_mb: 1,
  });
  assert(command[0] === "python3", "python3 must remain argv[0]");
  assert(command[1] === "-c", "the scanner must be passed with -c");
  assert(command[3] === "/tmp/example", "the path must be an argv value");

  const output = await workflow.parse!(
    JSON.stringify({
      scanned_root: "/tmp/example",
      files_scanned: 2,
      skipped_entries: 0,
      total_bytes_scanned: 3145728,
      minimum_size_bytes: 1048576,
      largest_files: [{
        path: "/tmp/example/large.bin",
        size_bytes: 2097152,
        size_mb: 2,
      }],
    }),
    { stderr: "", exitCode: 0 },
  );
  assert(output.results.files.length === 1, "one file should be parsed");
  assert(
    output.results.summary ===
      "Found 1 file above the selected size threshold.",
    "a friendly summary should be returned",
  );
});

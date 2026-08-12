import workflow from "../steps/01_prepare_next_meeting.ts";

function assert(condition: unknown, message: string) {
  if (!condition) throw new Error(message);
}

Deno.test("builds and parses a meeting-prep request", async () => {
  const command = workflow.command({ calendar_id: "primary" });
  assert(command[0] === "gws", "gws must remain argv[0]");
  assert(command.includes("+meeting-prep"), "meeting helper must be selected");
  const output = await workflow.parse!(
    JSON.stringify({ message: "No upcoming meetings found." }),
    { stderr: "", exitCode: 0 },
  );
  assert(
    output.results.meeting === null,
    "the no-meeting result should be parsed",
  );
  assert(
    output.results.summary === "No upcoming meetings found.",
    "the helper message should be shown directly",
  );
});

import workflow from "../steps/01_build_weekly_digest.ts";

function assert(condition: unknown, message: string) {
  if (!condition) throw new Error(message);
}

Deno.test("builds and parses a weekly digest request", async () => {
  const command = workflow.command({});
  assert(command[0] === "gws", "gws must remain argv[0]");
  assert(command.includes("+weekly-digest"), "weekly helper must be selected");
  const output = await workflow.parse!(
    JSON.stringify({
      meetings: [{ summary: "Planning", start: "2026-08-12T09:00:00Z" }],
      meetingCount: 1,
      unreadEmails: 3,
      periodStart: "2026-08-11T09:00:00Z",
      periodEnd: "2026-08-18T09:00:00Z",
    }),
    { stderr: "", exitCode: 0 },
  );
  assert(output.results.meeting_count === 1, "one meeting should be parsed");
  assert(
    output.results.summary ===
      "1 meeting and 3 unread emails in this snapshot.",
    "a friendly summary should be returned",
  );
});

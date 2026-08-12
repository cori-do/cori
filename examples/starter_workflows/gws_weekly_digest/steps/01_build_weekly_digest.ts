import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({}).passthrough();

const RawMeeting = z.object({
  summary: z.string(),
  start: z.string(),
});

const RawOutput = z.object({
  meetings: z.array(RawMeeting),
  meetingCount: z.number().int().nonnegative(),
  unreadEmails: z.number().int().nonnegative(),
  periodStart: z.string().min(1),
  periodEnd: z.string().min(1),
});

const Output = z.object({
  results: z.object({
    title: z.string(),
    summary: z.string(),
    period_start: z.string(),
    period_end: z.string(),
    meeting_count: z.number().int().nonnegative(),
    unread_emails: z.number().int().nonnegative(),
    meetings: z.array(z.object({
      title: z.string(),
      start: z.string(),
    })),
  }),
});

export default step.cli({
  description: "Build a seven-day Calendar and Gmail digest",
  input: Input,
  output: Output,
  command: () => [
    "gws",
    "workflow",
    "+weekly-digest",
    "--format",
    "json",
  ],
  parse: (stdout) => {
    const raw = RawOutput.parse(JSON.parse(stdout));
    if (raw.meetingCount !== raw.meetings.length) {
      throw new Error("Weekly digest meetingCount does not match meetings");
    }
    const meetingsLabel = `${raw.meetingCount} ${
      raw.meetingCount === 1 ? "meeting" : "meetings"
    }`;
    const mailLabel = `${raw.unreadEmails} unread ${
      raw.unreadEmails === 1 ? "email" : "emails"
    }`;
    return Output.parse({
      results: {
        title: "Workspace weekly digest",
        summary: `${meetingsLabel} and ${mailLabel} in this snapshot.`,
        period_start: raw.periodStart,
        period_end: raw.periodEnd,
        meeting_count: raw.meetingCount,
        unread_emails: raw.unreadEmails,
        meetings: raw.meetings.map((meeting) => ({
          title: meeting.summary,
          start: meeting.start,
        })),
      },
    });
  },
});

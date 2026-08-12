import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  calendar_id: z.string().min(1),
});

const RawAttendee = z.object({
  email: z.string(),
  responseStatus: z.string(),
});

const RawMeeting = z.object({
  summary: z.string(),
  start: z.string(),
  end: z.string(),
  description: z.string(),
  location: z.string(),
  hangoutLink: z.string(),
  htmlLink: z.string(),
  attendees: z.array(RawAttendee),
  attendeeCount: z.number().int().nonnegative(),
});

const RawNoMeeting = z.object({
  message: z.string().min(1),
});

const Meeting = z.object({
  title: z.string(),
  start: z.string(),
  end: z.string(),
  agenda: z.string().nullable(),
  location: z.string().nullable(),
  video_call: z.string().nullable(),
  event_link: z.string().nullable(),
  attendee_count: z.number().int().nonnegative(),
  attendees: z.array(z.object({
    email: z.string(),
    response: z.string(),
  })),
});

const Output = z.object({
  results: z.object({
    title: z.string(),
    summary: z.string(),
    meeting: Meeting.nullable(),
  }),
});

export default step.cli({
  description: "Prepare the next meeting from Google Calendar",
  input: Input,
  output: Output,
  command: ({ calendar_id }) => [
    "gws",
    "workflow",
    "+meeting-prep",
    "--calendar",
    calendar_id,
    "--format",
    "json",
  ],
  parse: (stdout) => {
    const raw = z.union([RawMeeting, RawNoMeeting]).parse(JSON.parse(stdout));
    if ("message" in raw) {
      return Output.parse({
        results: {
          title: "Next meeting prep",
          summary: raw.message,
          meeting: null,
        },
      });
    }
    return Output.parse({
      results: {
        title: "Next meeting prep",
        summary: `${raw.summary} starts at ${raw.start}.`,
        meeting: {
          title: raw.summary,
          start: raw.start,
          end: raw.end,
          agenda: raw.description || null,
          location: raw.location || null,
          video_call: raw.hangoutLink || null,
          event_link: raw.htmlLink || null,
          attendee_count: raw.attendeeCount,
          attendees: raw.attendees.map((attendee) => ({
            email: attendee.email,
            response: attendee.responseStatus,
          })),
        },
      },
    });
  },
});

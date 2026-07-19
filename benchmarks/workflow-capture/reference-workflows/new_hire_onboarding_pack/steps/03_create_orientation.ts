import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ calendar_id: z.string(), summary: z.string(), start: z.string(), end: z.string() });
const Output = z.object({ event_id: z.string() });
export default step.cli({ description: "Create orientation without notifications", input: Input, output: Output, command: ({ calendar_id, summary, start, end }) => ["gws", "calendar", "events", "insert", "--params", JSON.stringify({ calendarId: calendar_id, sendUpdates: "none" }), "--json", JSON.stringify({ summary, start: { dateTime: start }, end: { dateTime: end } })], parse: (stdout) => ({ event_id: (JSON.parse(stdout) as { id: string }).id }) });

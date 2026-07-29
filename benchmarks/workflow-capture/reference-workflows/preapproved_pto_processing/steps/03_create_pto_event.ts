import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ calendar_id: z.string(), summary: z.string(), start: z.string(), end: z.string() });
const Output = z.object({ event_id: z.string() });
export default step.cli({ description: "Create ordinary all-day PTO event without notifications", input: Input, output: Output, command: ({ calendar_id, summary, start, end }) => ["gws", "calendar", "events", "insert", "--params", JSON.stringify({ calendarId: calendar_id, sendUpdates: "none" }), "--json", JSON.stringify({ summary, start: { date: start }, end: { date: end } })], parse: (stdout) => ({ event_id: (JSON.parse(stdout) as { id: string }).id }) });

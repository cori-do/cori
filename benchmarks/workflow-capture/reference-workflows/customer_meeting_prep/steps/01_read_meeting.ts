import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ calendar_id: z.string(), as_of: z.string() });
const Output = z.object({ items: z.array(z.unknown()) });
export default step.cli({ description: "Read upcoming customer calendar events", input: Input, output: Output, command: ({ calendar_id, as_of }) => ["gws", "calendar", "events", "list", "--params", JSON.stringify({ calendarId: calendar_id, timeMin: as_of, singleEvents: true })], parse: (stdout) => ({ items: (JSON.parse(stdout) as { items?: unknown[] }).items ?? [] }) });

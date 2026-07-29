import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  run_tag: z.string(),
  messages: z.array(z.object({ id: z.string(), label_names: z.array(z.string()) })),
});
const Output = z.object({ pending_ids: z.array(z.string()), skipped_ids: z.array(z.string()) });

/**
 * This job runs against the same mailbox every morning, so anything an earlier
 * run finished has to stay untouched. Re-running on an unchanged mailbox
 * therefore does nothing rather than re-labelling and double-counting.
 */
export default step.code({
  description: "Set aside the messages an earlier run already completed",
  input: Input,
  output: Output,
  run: ({ run_tag, messages }) => {
    const completed = `${run_tag}/triaged`;
    const pending: string[] = [];
    const skipped: string[] = [];
    for (const message of messages) {
      (message.label_names.includes(completed) ? skipped : pending).push(message.id);
    }
    return { pending_ids: pending, skipped_ids: skipped };
  },
});

import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ deck_title: z.string() });
const Output = z.object({ presentation_id: z.string(), first_slide_id: z.string() });
export default step.cli({
  description: "Create tagged budget variance presentation",
  input: Input,
  output: Output,
  command: ({ deck_title }) => [
    "gws", "slides", "presentations", "create",
    "--json", JSON.stringify({ title: deck_title }),
    "--format", "json",
  ],
  parse: (stdout) => {
    const presentation = JSON.parse(stdout) as {
      presentationId: string;
      slides?: { objectId?: string }[];
    };
    return {
      presentation_id: presentation.presentationId,
      first_slide_id: presentation.slides?.[0]?.objectId ?? "",
    };
  },
});

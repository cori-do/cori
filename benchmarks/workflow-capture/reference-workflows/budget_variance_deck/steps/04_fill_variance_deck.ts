import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  presentation_id: z.string(),
  first_slide_id: z.string(),
  executive_summary: z.string(),
  unfavourable_summary: z.string(),
  variance_detail: z.string(),
});
const Output = z.object({ deck_updated: z.boolean() });

export default step.cli({
  description: "Build the three-slide variance deck",
  input: Input,
  output: Output,
  command: ({
    presentation_id,
    first_slide_id,
    executive_summary,
    unfavourable_summary,
    variance_detail,
  }) => {
    if (!first_slide_id) throw new Error("new presentation has no initial slide");
    const slideIds = [first_slide_id, "unfavourableSlide", "varianceDetailSlide"];
    const shapeIds = ["executiveText", "unfavourableText", "varianceDetailText"];
    const texts = [executive_summary, unfavourable_summary, variance_detail];
    const requests: Record<string, unknown>[] = [
      {
        createSlide: {
          objectId: slideIds[1],
          slideLayoutReference: { predefinedLayout: "BLANK" },
        },
      },
      {
        createSlide: {
          objectId: slideIds[2],
          slideLayoutReference: { predefinedLayout: "BLANK" },
        },
      },
    ];
    for (let index = 0; index < slideIds.length; index += 1) {
      requests.push({
        createShape: {
          objectId: shapeIds[index],
          shapeType: "TEXT_BOX",
          elementProperties: {
            pageObjectId: slideIds[index],
            size: {
              width: { magnitude: 8_000_000, unit: "EMU" },
              height: { magnitude: 5_000_000, unit: "EMU" },
            },
            transform: {
              scaleX: 1,
              scaleY: 1,
              translateX: 500_000,
              translateY: 500_000,
              unit: "EMU",
            },
          },
        },
      }, {
        insertText: {
          objectId: shapeIds[index],
          text: texts[index],
          insertionIndex: 0,
        },
      });
    }
    return [
      "gws", "slides", "presentations", "batchUpdate",
      "--params", JSON.stringify({ presentationId: presentation_id }),
      "--json", JSON.stringify({ requests }),
    ];
  },
  parse: () => ({ deck_updated: true }),
});

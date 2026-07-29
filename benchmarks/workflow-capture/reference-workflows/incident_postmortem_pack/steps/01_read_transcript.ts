import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ transcript_document_id: z.string() });
const Output = z.object({ transcript_text: z.string() });

export default step.cli({
  description: "Read the incident channel transcript",
  input: Input,
  output: Output,
  command: ({ transcript_document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: transcript_document_id, fields: "documentId,title,body/content" }), "--format", "json"],
  parse: (stdout) => {
    const document = JSON.parse(stdout) as {
      body?: {
        content?: {
          paragraph?: {
            elements?: { textRun?: { content?: string } }[];
          };
        }[];
      };
    };
    const transcript_text = (document.body?.content ?? [])
      .flatMap((block) => block.paragraph?.elements ?? [])
      .map((element) => element.textRun?.content ?? "")
      .join("");
    return { transcript_text };
  },
});

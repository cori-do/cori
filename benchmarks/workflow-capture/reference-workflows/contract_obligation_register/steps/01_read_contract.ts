import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ contract_document_id: z.string() });
const Output = z.object({ contract_text: z.string() });

export default step.cli({
  description: "Read the signed contract",
  input: Input,
  output: Output,
  command: ({ contract_document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: contract_document_id, fields: "documentId,title,body/content" }), "--format", "json"],
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
    const contract_text = (document.body?.content ?? [])
      .flatMap((block) => block.paragraph?.elements ?? [])
      .map((element) => element.textRun?.content ?? "")
      .join("");
    return { contract_text };
  },
});

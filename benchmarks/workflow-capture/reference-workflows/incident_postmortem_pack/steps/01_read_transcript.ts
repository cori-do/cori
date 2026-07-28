import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ transcript_document_id: z.string() });
const Output = z.object({ transcript: z.unknown() });

export default step.cli({
  description: "Read the incident channel transcript",
  input: Input,
  output: Output,
  command: ({ transcript_document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: transcript_document_id, fields: "documentId,title,body/content" })],
  parse: (stdout) => ({ transcript: JSON.parse(stdout) as unknown }),
});

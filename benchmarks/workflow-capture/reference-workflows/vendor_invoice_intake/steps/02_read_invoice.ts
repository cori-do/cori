import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ document_id: z.string() });
const Output = z.object({ document: z.unknown() });

export default step.cli({
  description: "Read one invoice document",
  input: Input,
  output: Output,
  command: ({ document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: document_id, fields: "documentId,title,body/content" })],
  parse: (stdout) => ({ document: JSON.parse(stdout) as unknown }),
});

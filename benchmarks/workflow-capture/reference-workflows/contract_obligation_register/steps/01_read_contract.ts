import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ contract_document_id: z.string() });
const Output = z.object({ contract: z.unknown() });

export default step.cli({
  description: "Read the signed contract",
  input: Input,
  output: Output,
  command: ({ contract_document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: contract_document_id, fields: "documentId,title,body/content" })],
  parse: (stdout) => ({ contract: JSON.parse(stdout) as unknown }),
});

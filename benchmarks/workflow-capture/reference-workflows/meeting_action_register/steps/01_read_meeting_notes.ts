import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ meeting_notes_document_id: z.string() });
const Output = z.object({ document: z.unknown() });
export default step.cli({ description: "Read synthetic meeting notes document", input: Input, output: Output, command: ({ meeting_notes_document_id }) => ["gws", "docs", "documents", "get", "--params", JSON.stringify({ documentId: meeting_notes_document_id })], parse: (stdout) => ({ document: JSON.parse(stdout) }) });

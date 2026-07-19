import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ run_tag: z.string(), objectives: z.array(z.string()), risks: z.array(z.string()), questions: z.array(z.string()) });
const Output = z.object({ document_id: z.string() });
export default step.cli({ description: "Create tagged customer preparation document", input: Input, output: Output, command: ({ run_tag, objectives, risks, questions }) => ["gws", "docs", "documents", "create", "--json", JSON.stringify({ title: `${run_tag} Customer Prep: Objectives ${objectives.join(", ")} Risks ${risks.join(", ")} Questions ${questions.join("; ")}` })], parse: (stdout) => ({ document_id: (JSON.parse(stdout) as { documentId: string }).documentId }) });

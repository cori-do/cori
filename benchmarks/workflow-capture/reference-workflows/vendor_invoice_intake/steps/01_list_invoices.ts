import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({ invoice_folder_query: z.string() });
const Output = z.object({ documents: z.array(z.object({ id: z.string(), name: z.string() })) });

export default step.cli({
  description: "Find this week's vendor invoices",
  input: Input,
  output: Output,
  command: ({ invoice_folder_query }) => ["gws", "drive", "files", "list", "--params", JSON.stringify({ q: invoice_folder_query, fields: "files(id,name)" })],
  parse: (stdout) => ({ documents: (JSON.parse(stdout) as { files?: { id: string; name: string }[] }).files ?? [] }),
});

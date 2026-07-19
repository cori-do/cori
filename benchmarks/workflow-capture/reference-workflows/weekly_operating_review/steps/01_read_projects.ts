import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ project_spreadsheet_id: z.string() });
const Output = z.object({ values: z.array(z.array(z.string())) });
export default step.cli({ description: "Read project operating metrics", input: Input, output: Output, command: ({ project_spreadsheet_id }) => ["gws", "sheets", "spreadsheets", "values", "get", "--params", JSON.stringify({ spreadsheetId: project_spreadsheet_id, range: "Projects" })], parse: (stdout) => ({ values: (JSON.parse(stdout) as { values?: string[][] }).values ?? [] }) });

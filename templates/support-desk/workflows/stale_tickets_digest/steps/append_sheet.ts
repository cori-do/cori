// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

// `cli` connector: argv[0] frozen at compile time. The program runs in a sandbox, without the platform's credentials
// (and, in a cluster with sandboxes, without network), or on the worker pool the org registry pins the connector to.
export default step.cli({
  name: "append_sheet",
  description: "Append today's counts to the digest sheet",
  connector: "gws",
  input: (ctx: Ctx<"append_sheet">) => ({ groups: ctx.steps.by_department.groups, date: ctx.run.started_at.slice(0, 10) }),
  output: z.object({ appended: z.number() }),
  command: ({ groups, date }) => ["gws", "sheets", "append", "--spreadsheet", "1AbC…", "--range", "Digest!A:C", "--values", JSON.stringify(groups.map(g => [date, g.department, g.tickets.length]))],
  parse: (stdout) => ({ appended: JSON.parse(stdout).updates.updatedRows }),
});

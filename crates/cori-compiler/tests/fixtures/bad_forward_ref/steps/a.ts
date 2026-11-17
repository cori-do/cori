// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "a", description: "reads b before it exists", input: (ctx: Ctx<"a">) => ({ v: ctx.steps.b.v }), output: z.object({ v: z.string() }), run: ({ v }) => ({ v }) });

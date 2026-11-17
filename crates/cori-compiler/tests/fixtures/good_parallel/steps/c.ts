// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "c", description: "also from a, parallel with b", input: (ctx: Ctx<"c">) => ({ n: ctx.steps.a.n }), output: z.object({ t: z.number() }), run: ({ n }) => ({ t: n * 3 }) });

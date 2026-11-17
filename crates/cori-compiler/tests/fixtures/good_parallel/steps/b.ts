// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "b", description: "from a", input: (ctx: Ctx<"b">) => ({ n: ctx.steps.a.n }), output: z.object({ d: z.number() }), run: ({ n }) => ({ d: n * 2 }) });

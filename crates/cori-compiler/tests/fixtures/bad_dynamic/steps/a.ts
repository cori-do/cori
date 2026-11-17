// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "a", description: "dynamic", input: (ctx: Ctx<"a">) => ({ v: (ctx.steps as any)[ctx.params.x] }), output: z.object({ v: z.string() }), run: ({ v }) => ({ v }) });

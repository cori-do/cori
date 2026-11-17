// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "b", description: "b", input: (ctx: Ctx<"b">) => ({ v: ctx.params.x }), output: z.object({ v: z.string() }), run: ({ v }) => ({ v }) });

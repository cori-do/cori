// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.human({ name: "gate", description: "approve", kind: "approval", to: { role: "lead" }, input: (ctx: Ctx<"gate">) => ({ x: ctx.params.x }), output: z.object({ approved: z.boolean() }), timeout: "PT5M" });

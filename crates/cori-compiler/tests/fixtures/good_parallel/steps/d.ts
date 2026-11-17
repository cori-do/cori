// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.http({ name: "d", description: "write", connector: "api", input: (ctx: Ctx<"d">) => ({ d: ctx.steps.b.d, t: ctx.steps.c.t }), output: z.object({ ok: z.boolean() }), request: ({ d, t }) => ({ method: "POST", path: "/sum", body: { d, t } }) });

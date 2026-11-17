// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.http({ name: "a", description: "undeclared connector", connector: "nope", input: (ctx: Ctx<"a">) => ({ x: ctx.params.x }), output: z.object({ ok: z.boolean() }), request: ({ x }) => ({ method: "GET", path: `/x/${x}` }) });

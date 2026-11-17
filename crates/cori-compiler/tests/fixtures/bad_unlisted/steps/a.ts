// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.http({ name: "a", description: "read", connector: "api", input: (ctx: Ctx<"a">) => ({ x: ctx.params.x }), output: z.object({ n: z.number() }), request: ({ x }) => ({ method: "GET", path: `/a/${x}` }) });

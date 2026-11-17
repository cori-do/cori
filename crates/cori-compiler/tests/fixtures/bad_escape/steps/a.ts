// SPDX-License-Identifier: Apache-2.0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "a", description: "escape", input: (ctx: Ctx<"a">) => ({ everything: ctx }), output: z.object({ v: z.string() }), run: () => ({ v: "x" }) });

// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";

export default step.http({
  name: "post_digest",
  description: "Post the digest to #support",
  connector: "slack",
  input: (ctx: Ctx<"post_digest">) => ({ items: ctx.steps.summarise.items }),
  output: z.object({ notified: z.boolean() }),
  request: ({ items }) => ({ method: "POST", path: "/api/chat.postMessage", body: { channel: "#support", text: items.map(i => `*${i.department}*\n${i.summary}`).join("\n\n") } }),
  parse: (res) => ({ notified: res.json.ok === true }),
});

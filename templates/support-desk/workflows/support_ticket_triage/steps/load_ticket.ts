// SPDX-License-Identifier: MIT-0
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
import { Ticket } from "../types";

// `helpdesk` is a company MCP server with auth = principal: the broker presents a token
// exchanged for the run principal, and the server applies its own rules to it.
export default step.mcp({
  name: "load_ticket",
  description: "Load the ticket from the helpdesk",
  connector: "helpdesk",
  tool: "getTicket",
  input: (ctx: Ctx<"load_ticket">) => ({ id: ctx.params.ticket_id }),
  output: z.object({ ticket: Ticket }),
  parse: (result) => ({ ticket: Ticket.parse(result) }),
});

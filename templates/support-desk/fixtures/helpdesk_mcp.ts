// SPDX-License-Identifier: MIT-0
// A tiny stdio MCP server standing in for the company's helpdesk server (exercises the real MCP transport).
// Tools: getTicket, addReply. Run by Cori via `[connectors.helpdesk] command = ["deno", "run", "fixtures/helpdesk_mcp.ts"]`.
const tickets: Record<string, unknown> = { "T-1042": { id: "T-1042", subject: "Invoice charged twice", body: "Charged twice for INV-42 this month, please refund one.", customer_id: "C-77" } };
const dec = new TextDecoder(); const enc = new TextEncoder(); let buf = "";
const reply = (id: unknown, result: unknown) => Deno.stdout.write(enc.encode(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n"));
for await (const chunk of Deno.stdin.readable) {
  buf += dec.decode(chunk); let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl); buf = buf.slice(nl + 1); if (!line.trim()) continue;
    const m = JSON.parse(line);
    if (m.method === "initialize") await reply(m.id, { protocolVersion: "2025-03-26", capabilities: { tools: {} }, serverInfo: { name: "helpdesk-mcp", version: "0.1" } });
    else if (m.method === "tools/list") await reply(m.id, { tools: [
      { name: "getTicket", description: "Load a helpdesk ticket by id", inputSchema: { type: "object", properties: { id: { type: "string", description: "ticket id, e.g. T-1042" } }, required: ["id"] }, outputSchema: { type: "object", properties: { id: { type: "string" }, subject: { type: "string" }, body: { type: "string" }, customer_id: { type: "string" }, status: { type: "string", enum: ["open", "in_progress", "closed"] }, created_at: { type: "string", format: "date-time" } }, required: ["id", "subject", "body", "customer_id", "status"] }, annotations: { readOnlyHint: true, idempotentHint: true } },
      { name: "addReply", description: "Append an agent reply to a ticket", inputSchema: { type: "object", properties: { id: { type: "string" }, text: { type: "string" } }, required: ["id", "text"] }, outputSchema: { type: "object", properties: { reply_id: { type: "string" } }, required: ["reply_id"] }, annotations: { readOnlyHint: false } },
    ] });
    else if (m.method === "tools/call") {
      const { name, arguments: a } = m.params;
      if (name === "getTicket") { const t = tickets[a.id]; await reply(m.id, t ? { structuredContent: t, content: [{ type: "text", text: JSON.stringify(t) }] } : { isError: true, content: [{ type: "text", text: `no ticket ${a.id}` }] }); }
      else if (name === "addReply") await reply(m.id, { structuredContent: { reply_id: "R-" + Math.floor(Math.random() * 9000 + 1000) }, content: [] });
      else await reply(m.id, { isError: true, content: [{ type: "text", text: `unknown tool ${name}` }] });
    }
  }
}

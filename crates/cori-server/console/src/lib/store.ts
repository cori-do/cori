import { api } from "./api";
import type { Column, Table } from "./types";

export type StoreRef = { app: string; target: string; store: string };
export type Row = Record<string, unknown>;

// A store database as the caller's store role, through the same generated tools and guarded query a workflow gets:
// nothing the role's rules refuse is shown or writable.
export function storeClient({ app, target, store }: StoreRef) {
  const path = target === "live" ? `/v1/apps/${app}/stores/${store}/mcp` : `/v1/apps/${app}/versions/${target}/stores/${store}/mcp`;
  async function rpc<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const j = await api<{ error?: { message: string }; result?: T & { isError?: boolean; content?: { text?: string }[] } }>(path, {
      method: "POST", body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    if (j.error) throw new Error(j.error.message);
    if (j.result?.isError) throw new Error(j.result.content?.[0]?.text || "failed");
    return j.result as T;
  }
  const call = <T = Record<string, unknown>>(name: string, args: Record<string, unknown>) =>
    rpc<{ structuredContent?: T; content?: { text?: string }[] }>("tools/call", { name, arguments: args })
      .then((r) => (r.structuredContent ?? JSON.parse(r.content?.[0]?.text || "null")) as T);
  const query = (sql: string, params: unknown[] = [], max = 500) =>
    call<{ rows?: Row[] }>("query", { sql, params, max_rows: max }).then((r) => r?.rows || []);
  return { rpc, call, query };
}
export type StoreClient = ReturnType<typeof storeClient>;

export const readCols = (t: Table): Column[] => t.columns.filter((c) => c.read);
export const pkReadable = (t: Table) => t.primary_key.length > 0 && t.primary_key.every((k) => t.columns.some((c) => c.name === k && c.read));
export const pkArgs = (t: Table, row: Row) => Object.fromEntries(t.primary_key.map((k) => [k, row[k]]));

/** A write result parked for approval: tells the caller and returns true. */
export type WriteResult = { pending_approval?: boolean; group?: string } | null | undefined;

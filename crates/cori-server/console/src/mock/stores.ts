// The demo's stores: `cori/describe` and the generated tools (`query`, `<table>.create|update|delete`) over in-memory rows,
// as a store role sees them. `query` understands the SQL the sheet writes (filters, search, sort, paging, counts).
import type { MTable } from "./data";

type Row = Record<string, unknown>;
export type Caller = { sub: string; roles: string[]; builds: boolean };

function roleOf(tables: MTable[], who: Caller): { role: string | null; inspector: boolean } {
  const roles = new Set(tables.flatMap((t) => Object.keys(t.roles)));
  const role = who.roles.find((r) => roles.has(r)) ?? null;
  return { role: role ?? (who.builds ? "cori_inspector" : null), inspector: !role && who.builds };
}

export function describe(store: string, tables: MTable[], who: Caller) {
  const { role, inspector } = roleOf(tables, who);
  const allRoles = [...new Set(tables.flatMap((t) => Object.keys(t.roles)))];
  const d = {
    role, inspector,
    roles: allRoles.map((name) => ({ name, description: name.includes("lead") || name === "manager" || name === "finance" ? "Decides and corrects" : "Day-to-day use" })),
    you: { sub: who.sub, roles: who.roles },
    policy: { hash: "9c1e44a0b7d2f3a1", enforced_by: "postgres" },
    tables: tables.map((t) => {
      const r = role && !inspector ? t.roles[role] : undefined;
      const read = inspector || !!r;
      const can = (c: { create?: string[]; update?: string[] }, k: "create" | "update") => !inspector && !!role && !!c[k]?.includes(role);
      const creatable = t.columns.some((c) => can(c, "create"));
      const updatable = t.columns.some((c) => can(c, "update"));
      return {
        name: t.name, read, primary_key: t.columns.filter((c) => c.pk).map((c) => c.name), tenant: t.tenant,
        scope: inspector ? "all" : r?.scope ?? "hidden", rls: true, delete: !!r?.delete, max_per_page: 500,
        tools: { create: creatable ? `${t.name}.create` : undefined, update: updatable ? `${t.name}.update` : undefined, delete: r?.delete ? `${t.name}.delete` : undefined },
        columns: t.columns.map((c) => ({ name: c.name, type: c.type, nullable: c.nullable, pk: c.pk, default: c.default, read, create: can(c, "create"), update: can(c, "update"), approval: c.approval })),
      };
    }),
    store,
  };
  if (who.builds) return d;
  // one of the app's users is told of what their role reaches only (the server's `cori_store::reached_only`): the tables and
  // columns it has a right on, of the roles theirs, nothing of the policy
  const right = (c: { read: boolean; create: boolean; update: boolean }) => c.read || c.create || c.update;
  return {
    ...d, policy: undefined, roles: d.roles.filter((r) => r.name === role),
    tables: role ? d.tables.filter((t) => t.read || t.delete || t.columns.some(right)).map((t) => ({ ...t, tenant: undefined, rls: undefined, columns: t.columns.filter(right) })) : [],
  };
}

/** The rows `who` reaches in `t`. */
function visible(t: MTable, tables: MTable[], who: Caller): Row[] {
  const { role, inspector } = roleOf(tables, who);
  if (inspector) return t.rows;
  const r = role ? t.roles[role] : undefined;
  if (!r) return [];
  return r.scope === "own" && t.tenant ? t.rows.filter((x) => x[t.tenant!] === who.sub) : t.rows;
}

const text = (v: unknown) => (v === null || v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v)).toLowerCase();
const like = (pattern: unknown) => String(pattern ?? "").toLowerCase().replace(/^%|%$/g, "");

/** Splits `a AND (b OR c) AND d` at its top-level ANDs. */
function conjuncts(where: string): string[] {
  const out: string[] = [];
  let depth = 0, cur = "";
  const tokens = where.split(/(\(|\)|\sAND\s)/);
  for (const tk of tokens) {
    if (tk === "(") depth++;
    if (tk === ")") depth--;
    if (/^\sAND\s$/.test(tk) && depth === 0) { out.push(cur.trim()); cur = ""; continue; }
    cur += tk;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

function test(cond: string, row: Row, params: unknown[]): boolean {
  const p = (m: RegExpMatchArray | null) => (m ? params[Number(m[1]) - 1] : undefined);
  const col = /"([^"]+)"/.exec(cond)?.[1];
  // the search box: (lower(CAST("a" AS text)) LIKE $1 OR lower(CAST("b" AS text)) LIKE $1 …)
  if (/\sOR\s/.test(cond) && /LIKE/.test(cond) && !/IS NULL/.test(cond)) {
    const needle = like(p(/LIKE \$(\d+)/.exec(cond)));
    return Object.values(row).some((v) => text(v).includes(needle));
  }
  if (!col) return true;
  const v = row[col];
  if (/IS NULL OR CAST\([^)]*\) = ''/.test(cond)) return v === null || v === undefined || v === "";
  if (/IS NOT NULL AND/.test(cond)) return !(v === null || v === undefined || v === "");
  if (/NOT LIKE/.test(cond)) return !text(v).includes(like(p(/LIKE \$(\d+)/.exec(cond))));
  if (/LIKE/.test(cond)) return text(v).includes(like(p(/LIKE \$(\d+)/.exec(cond))));
  const m = /(<>|>=|<=|=|>|<)\s*(?:CAST\()?\$(\d+)/.exec(cond);
  if (!m) return true;
  const rhs = params[Number(m[2]) - 1];
  const num = typeof v === "number" && !Number.isNaN(Number(rhs));
  const a = num ? (v as number) : text(v), b = num ? Number(rhs) : text(rhs);
  switch (m[1]) {
    case "=": return a === b;
    case "<>": return a !== b;
    case ">": return a > b;
    case ">=": return a >= b;
    case "<": return a < b;
    case "<=": return a <= b;
  }
  return true;
}

export function query(tables: MTable[], who: Caller, sql: string, params: unknown[] = [], max = 500): Row[] {
  const name = /FROM\s+"?([A-Za-z0-9_]+)"?/i.exec(sql)?.[1];
  const t = tables.find((x) => x.name === name);
  if (!t) throw new Error(`relation "${name}" does not exist`);
  let rows = visible(t, tables, who);
  const where = /\sWHERE\s([\s\S]*?)(?:\sORDER BY\s|\sLIMIT\s|$)/i.exec(sql)?.[1];
  if (where) { const parts = conjuncts(where); rows = rows.filter((r) => parts.every((c) => test(c, r, params))); }
  if (/SELECT\s+count\(\*\)/i.test(sql)) return [{ n: rows.length }];
  const order = /ORDER BY\s+"([^"]+)"\s*(ASC|DESC)?/i.exec(sql);
  if (order) {
    const [, c, dir] = order;
    rows = [...rows].sort((a, b) => {
      const x = a[c], y = b[c];
      if (x === y) return 0;
      if (x === null || x === undefined) return 1;
      if (y === null || y === undefined) return -1;
      return (x < y ? -1 : 1) * (dir?.toUpperCase() === "DESC" ? -1 : 1);
    });
  }
  const limit = Number(/LIMIT\s+(\d+)/i.exec(sql)?.[1] ?? max), offset = Number(/OFFSET\s+(\d+)/i.exec(sql)?.[1] ?? 0);
  const cols = [...(/SELECT\s+([\s\S]*?)\s+FROM/i.exec(sql)?.[1] ?? "*").matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  return rows.slice(offset, offset + Math.min(limit, max)).map((r) => (cols.length ? Object.fromEntries(cols.map((c) => [c, r[c]])) : r));
}

export type Parked = { table: string; row: Row; changes: Row; role: string };

/** Runs a generated tool; a change to a column that needs approval is parked and reported as such. */
export function callTool(tables: MTable[], who: Caller, live: boolean, name: string, args: Row, park: (p: Parked) => string): unknown {
  const [tname, op] = name.split(".");
  const t = tables.find((x) => x.name === tname);
  if (!t) throw new Error(`unknown tool ${name}`);
  const { role, inspector } = roleOf(tables, who);
  if (inspector || !role) throw new Error("your role cannot change this table");
  const pk = t.columns.filter((c) => c.pk).map((c) => c.name);
  const find = () => t.rows.find((r) => pk.every((k) => String(r[k]) === String(args[k])));
  if (op === "create") {
    const row: Row = {};
    for (const c of t.columns) row[c.name] = args[c.name] ?? (c.default === "now()" ? new Date().toISOString() : c.default ?? null);
    if (pk.length === 1 && (row[pk[0]] === null || row[pk[0]] === undefined)) row[pk[0]] = Math.max(0, ...t.rows.map((r) => Number(r[pk[0]]) || 0)) + 1;
    if (t.tenant && !args[t.tenant]) row[t.tenant] = who.sub;
    t.rows.unshift(row);
    return { created: true };
  }
  const row = find();
  if (!row) throw new Error("no such row (or your role does not reach it)");
  if (op === "delete") { t.rows.splice(t.rows.indexOf(row), 1); return { deleted: true }; }
  const changes = Object.fromEntries(Object.entries(args).filter(([k]) => !pk.includes(k)));
  for (const k of Object.keys(changes)) {
    const c = t.columns.find((x) => x.name === k);
    if (!c?.update?.includes(role)) throw new Error(`permission denied for column ${k}`);
  }
  const needs = live && Object.keys(changes).some((k) => t.columns.find((c) => c.name === k)?.approval) && !role.includes("lead");
  if (needs) return { pending_approval: true, group: "support_lead", approval_id: park({ table: t.name, row, changes, role }) };
  Object.assign(row, changes);
  return { updated: true };
}

import type { ReactNode } from "react";
import { fmt } from "../../lib/format";
import type { Column, Describe, Table } from "../../lib/types";
import { Badge, Muted } from "../../ui/core";

const SCOPE: Record<string, string> = {
  own: "only their own rows",
  all: "every row",
  global: "every row (a shared table)",
  unscoped: "every row (the table has no owner column)",
  hidden: "no row: the table is not named in rules.yaml",
};

const Part = ({ title, children }: { title: string; children: ReactNode }) => (
  <section className="mt-7"><h3 className="mb-2.5 text-[13px] font-medium text-fg-3">{title}</h3>{children}</section>
);

/** What Postgres enforces for the role on this table: grants and row level security compiled from the role files. */
function Enforcement({ t, d }: { t: Table; d: Describe }) {
  const drift = t.drift || [];
  return (
    <Part title="Enforced by the database">
      <p className="leading-relaxed text-fg-2">
        Role <b className="font-medium">{d.role}</b> reads {SCOPE[t.scope || "own"] || t.scope}
        {t.scope === "own" && t.tenant && <> (where <code>{t.tenant}</code> is their id)</>}
        {t.soft_delete && <>, deleted rows (<code>{t.soft_delete}</code>) hidden</>}.{" "}
        {t.rls ? <Badge tone="success" dot>Row level security on</Badge> : <Badge tone="danger" dot>Row level security off</Badge>}
      </p>
      <p className="mt-1.5 text-[13px] leading-relaxed text-fg-3">
        Every read and write runs as a Postgres role granted only these columns: a query that reaches another column or another row fails in the database, whatever SQL it uses.
      </p>
      {drift.length > 0 && (
        <p className="mt-2 font-medium text-rose-700 dark:text-rose-400">
          The database refuses {drift.map((c, i) => <span key={c}>{i > 0 && ", "}<code>{c}</code></span>)} although the role files grant it: the policy is out of date. It is applied again at the next deploy.
        </p>
      )}
      {(d.policy?.warnings || []).length > 0 && (
        <ul className="mt-2 list-disc pl-5 text-amber-700 dark:text-amber-400">{(d.policy?.warnings || []).map((w) => <li key={w}>{w}</li>)}</ul>
      )}
      {d.policy?.hash && <p className="mt-1.5 text-xs text-fg-4">Policy <code>{d.policy.hash.slice(0, 12)}</code></p>}
    </Part>
  );
}

export function SchemaTab({ t, d, store, here }: { t: Table; d: Describe; store: string; here: (table?: string) => string }) {
  const access = (c: Column) => !c.read && !c.create && !c.update ? <Muted>hidden</Muted> : (
    <span className="flex flex-wrap gap-1">
      {c.read && <Badge>read</Badge>}{c.create && <Badge>create</Badge>}{c.update && <Badge>update</Badge>}
      {c.approval && <Badge tone="warning">needs approval</Badge>}
    </span>
  );
  const tools = Object.values(t.tools).filter(Boolean) as string[];
  return (
    <>
      <div className="overflow-x-auto rounded-xl ring-1 ring-line">
        <table className="w-full min-w-[560px] text-left text-[13.5px]">
          <thead className="bg-surface-2 text-[13px] text-fg-3">
            <tr><th className="px-3 py-2 font-medium">Column</th><th className="px-3 py-2 font-medium">Type</th><th className="px-3 py-2 font-medium">Empty allowed</th><th className="px-3 py-2 font-medium">Key</th><th className="px-3 py-2 font-medium">Default</th>{d.role && <th className="px-3 py-2 font-medium">Role {d.role}</th>}</tr>
          </thead>
          <tbody className="divide-y divide-line">
            {t.columns.map((c) => (
              <tr key={c.name}>
                <td className="px-3 py-2 font-medium">{c.name}</td>
                <td className="px-3 py-2"><code className="text-[12.5px] text-fg-2">{c.type}</code></td>
                <td className="px-3 py-2">{c.nullable ? "yes" : <Muted>no</Muted>}</td>
                <td className="px-3 py-2">
                  {c.pk && <Badge>primary</Badge>}
                  {c.fk && <> → <a href={here(c.fk.table)} className="link">{c.fk.table}</a>.{c.fk.column}</>}
                </td>
                <td className="px-3 py-2 font-mono text-xs text-fg-3">{fmt(c.default)}</td>
                {d.role && <td className="px-3 py-2">{access(c)}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {d.role && (
        <p className="mt-3 leading-relaxed text-fg-3">
          Role {d.role} on <b className="font-medium text-fg">{t.name}</b>:{" "}
          {tools.length ? tools.map((x, i) => <span key={x}>{i > 0 && " "}<code>{x}</code></span>) : "no tool"}
          {t.delete ? (t.delete_is_soft ? " · may delete (soft)" : " · may delete") : ""}{t.max_per_page ? ` · ${t.max_per_page} rows per page at most` : ""}.
        </p>
      )}
      {d.role && <Enforcement t={t} d={d} />}
      <Part title="Roles of this data">
        <div className="divide-y divide-line rounded-xl ring-1 ring-line">
          {(d.roles || []).map((r) => <div key={r.name} className="flex gap-4 px-3 py-2"><span className="w-40 shrink-0 font-medium">{r.name}</span><span className="text-fg-3">{r.description || ""}</span></div>)}
          {(d.roles || []).length === 0 && <div className="px-3 py-2 text-fg-3">none</div>}
        </div>
      </Part>
      <p className="mt-4 text-[13px] leading-relaxed text-fg-3">
        Schema and roles live in the app's folder: <code>stores/{store}/migrations/*.sql</code>, <code>roles/*.yaml</code>, <code>rules.yaml</code>. A deploy applies them to that release's preview data, going live to the live data.
      </p>
    </>
  );
}

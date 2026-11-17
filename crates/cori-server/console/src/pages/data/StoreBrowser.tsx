import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Code, Database, Lock, Sheet as SheetIcon, TableProperties } from "lucide-react";
import { devToken, getToken, setToken, useToken } from "../../lib/api";
import { qi } from "../../lib/format";
import { readCols, storeClient, type StoreRef } from "../../lib/store";
import type { Describe } from "../../lib/types";
import { useAsync } from "../../lib/useAsync";
import { Badge, Button, Callout, cx, Empty, ErrorNote, Spinner } from "../../ui/core";
import { SchemaTab } from "./SchemaTab";
import { SqlTab } from "./SqlTab";
import { TableSheet } from "./TableSheet";

type Mode = "sheet" | "schema" | "sql";

/** Fills the window below the element's top, so the sheet scrolls inside itself as a spreadsheet does. */
function useFillHeight(min = 420) {
  const ref = useRef<HTMLDivElement>(null);
  const [h, setH] = useState(600);
  const fit = useCallback(() => {
    const top = (ref.current?.getBoundingClientRect().top ?? 0) + scrollY;
    setH(Math.max(min, Math.floor(innerHeight - top - 16)));
  }, [min]);
  // after every render too: a notice above it may come or go
  useLayoutEffect(fit);
  useEffect(() => { addEventListener("resize", fit); return () => removeEventListener("resize", fit); }, [fit]);
  return { ref, h };
}

/** One store as a workbook: a sheet per table (tabs along the bottom) as the person's store role, and for the app's builders
 *  its schema and SQL. A builder holding none of the store's roles reads it read-only: every row, no write. One of the
 *  app's users sees the tables their role reads and nothing of the others (the server tells them of no other). */
export function StoreBrowser({ store, table, here, builder }: { store: StoreRef; table?: string; here: (table?: string) => string; builder: boolean }) {
  const client = useMemo(() => storeClient(store), [store.app, store.target, store.store]); // eslint-disable-line react-hooks/exhaustive-deps
  const { data: d, error, loading } = useAsync(() => client.rpc<Describe>("cori/describe"), [client]);
  const [mode, setMode] = useState<Mode>("sheet");
  const [counts, setCounts] = useState<Record<string, unknown>>({});
  const token = useToken();
  const { ref, h } = useFillHeight();

  const loadCounts = useCallback(() => {
    if (!d?.role) return;
    for (const t of d.tables.filter((t) => t.read && readCols(t).length)) {
      client.query(`SELECT count(*) AS n FROM ${qi(t.name)}`, [], 1)
        .then((r) => setCounts((c) => ({ ...c, [t.name]: r[0]?.n ?? "" })))
        .catch(() => {});
    }
  }, [client, d]);
  useEffect(() => { setCounts({}); loadCounts(); }, [loadCounts]);

  if (error) return <ErrorNote>{error}</ErrorNote>;
  if (!d || loading) return <Spinner />;

  // its builders see every table, those their role does not read locked; one of its users the tables they read rows of
  const tables = builder ? d.tables : d.tables.filter((x) => x.read && readCols(x).length > 0 && x.scope !== "hidden");
  const roles = d.roles || [];
  if (!builder && (!d.role || !tables.length)) {
    return <Empty icon={<Database />} title="Nothing to show here">Your role in this app reads no table of {store.store}.</Empty>;
  }
  const t = tables.find((x) => x.name === table) || tables.find((x) => x.read) || tables[0];
  // someone without a role sees nothing of the store; a builder its schema at least
  const current: Mode = d.role ? (builder ? mode : "sheet") : "schema";
  const dev = devToken(token);

  // dev tokens only: swap the store roles held for `role` (or none), keeping the other roles
  function viewAs(role?: string) {
    const p = devToken(getToken());
    if (!p) return;
    const storeRoles = roles.map((r) => r.name);
    const kept = p.roles.filter((r) => !storeRoles.includes(r));
    setToken(`dev:${p.user}:${[...kept, ...(role ? [role] : [])].join(",")}`);
  }

  const right = (
    <>
      {d.inspector ? (
        <span className="flex items-center gap-1.5 text-xs text-fg-3">
          <Badge tone="brand" title={`You build this app and hold none of ${store.store}'s roles: every row and column, no change${store.target === "live" ? "; each read of live data is logged" : ""}`}>Read-only</Badge>
          {dev && roles.length > 0 && (
            <select value="" onChange={(e) => viewAs(e.target.value)} aria-label="View as a role"
              className="h-7 cursor-pointer rounded-md bg-transparent pr-6 pl-1.5 text-xs text-fg ring-1 ring-line ring-inset">
              <option value="" disabled>View as…</option>
              {roles.map((r) => <option key={r.name}>{r.name}</option>)}
            </select>
          )}
        </span>
      ) : d.role && (
        <span className="flex items-center gap-1.5 text-xs text-fg-3" title="reads and writes run as this role: its rules, constraints and approvals apply">
          as
          {dev && builder && roles.length > 1 ? (
            <select value={d.role} onChange={(e) => viewAs(e.target.value)} aria-label="View as a role"
              className="h-7 cursor-pointer rounded-md bg-transparent pr-6 pl-1.5 text-xs font-semibold text-fg ring-1 ring-line ring-inset">
              {roles.map((r) => <option key={r.name}>{r.name}</option>)}
            </select>
          ) : <b className="text-fg">{d.role}</b>}
        </span>
      )}
      {builder && (
        <span className="inline-flex rounded-md bg-surface-3/60 p-0.5" role="tablist" aria-label="View">
          {([["sheet", "Sheet", <SheetIcon key="i" />], ["schema", "Schema", <TableProperties key="i" />], ["sql", "SQL", <Code key="i" />]] as const)
            .filter(([k]) => d.role || k === "schema")
            .map(([k, label, icon]) => (
              <button key={k} type="button" role="tab" aria-selected={current === k} onClick={() => setMode(k)}
                className={cx("inline-flex h-6 cursor-pointer items-center gap-1.5 rounded px-2 text-xs [&>svg]:size-3.5",
                  current === k ? "bg-surface font-medium text-fg shadow-xs" : "text-fg-3 hover:text-fg")}>
                {icon}{label}
              </button>
            ))}
        </span>
      )}
    </>
  );

  return (
    <>
      {!d.role && (
        <Callout tone="warning" className="mb-3" title={`You hold none of the roles of ${store.store}`}>
          Its rows stay hidden; its schema shows. Its roles: {roles.map((r) => r.name).join(", ") || "none"}.
          {dev && roles.length > 0 && (
            <span className="mt-2 flex flex-wrap items-center gap-2">View as {roles.map((r) => <Button key={r.name} size="xs" onClick={() => viewAs(r.name)}>{r.name}</Button>)}<Badge>dev</Badge></span>
          )}
        </Callout>
      )}

      <div ref={ref} style={{ height: h }} className="flex flex-col overflow-hidden rounded-xl bg-surface shadow-xs ring-1 ring-line">
        {!t ? (
          <div className="p-4"><Empty compact title="No table yet">Add a migration in <code>stores/{store.store}/migrations/</code>.</Empty></div>
        ) : current === "sheet" && d.role ? (
          <TableSheet key={`${t.name}|${d.role}`} t={t} role={d.role} client={client} live={store.target === "live"}
            prefsKey={`cori.sheet.${store.app}.${store.store}.${t.name}`} onWrite={loadCounts} right={right} />
        ) : (
          <>
            <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line bg-surface-2/60 px-2">
              <span className="px-1.5 text-sm font-semibold">{t.name}</span>
              <span className="ml-auto flex items-center gap-2">{right}</span>
            </div>
            {current === "sql" && d.role
              ? <SqlTab key={t.name} t={t} role={d.role} client={client} />
              : <div className="min-h-0 flex-1 overflow-auto p-4"><SchemaTab t={t} d={d} store={store.store} here={here} /></div>}
          </>
        )}

        {/* sheet tabs, one per table */}
        <nav aria-label="Tables" className="no-scrollbar flex h-9 shrink-0 items-stretch overflow-x-auto border-t border-line bg-surface-2 px-2">
          {tables.map((x) => {
            const on = x.name === t?.name;
            return (
              <a key={x.name} href={here(x.name)} aria-current={on ? "page" : undefined} title={x.read ? `${x.name}${counts[x.name] !== undefined ? ` · ${counts[x.name]} rows` : ""}` : "your role cannot read this table"}
                className={cx("-mt-px flex shrink-0 items-center gap-1.5 border-x border-transparent px-3.5 text-[13px]",
                  on ? "border-x-line border-t-2 border-t-brand-600 bg-surface font-semibold text-fg" : "text-fg-3 hover:bg-surface-3/60 hover:text-fg",
                  !x.read && "text-fg-4")}>
                {!x.read && <Lock className="size-3" />}
                {x.name}
                {x.read && counts[x.name] !== undefined && <span className="rounded bg-surface-3 px-1 text-[11px] font-normal text-fg-3 tabular-nums">{String(counts[x.name])}</span>}
              </a>
            );
          })}
          {tables.length === 0 && <span className="self-center px-2 text-xs text-fg-3">no table</span>}
        </nav>
      </div>
    </>
  );
}

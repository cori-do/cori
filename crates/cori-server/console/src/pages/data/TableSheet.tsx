import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Columns3, Download, ListFilter, Plus, RefreshCw, Redo2, Search, Undo2, X } from "lucide-react";
import { errMsg } from "../../lib/api";
import { coerce, qi } from "../../lib/format";
import { pkArgs, pkReadable, readCols, type Row, type StoreClient, type WriteResult } from "../../lib/store";
import type { Column, Table } from "../../lib/types";
import { Button, cx, ErrorNote, Input, Select } from "../../ui/core";
import { confirm } from "../../ui/dialog";
import { say } from "./say";
import { Popover } from "./Menu";
import { editText, kindOf, Sheet, type Edit, type Mark, type SheetCol } from "./Sheet";

// One table as a sheet: rows stream in as you scroll, filters and sort run in SQL through the role's guarded query, and
// every write — a typed cell, a paste, a cleared range, a new or deleted row — goes through the role's generated tools,
// so its rules, constraints and approvals apply. Cell edits can be undone (⌘Z) by writing the previous values back.

const PAGE = 100, EXPORT_CAP = 5000;

type Op = "contains" | "ncontains" | "eq" | "neq" | "gt" | "gte" | "lt" | "lte" | "empty" | "nempty";
const OPS: [Op, string][] = [
  ["contains", "contains"], ["ncontains", "does not contain"], ["eq", "is"], ["neq", "is not"],
  ["gt", ">"], ["gte", "≥"], ["lt", "<"], ["lte", "≤"], ["empty", "is empty"], ["nempty", "is not empty"],
];
const OP_LABEL = Object.fromEntries(OPS) as Record<Op, string>;
type Filter = { col: string; op: Op; value: string };
type Sort = { col: string; dir: "asc" | "desc" };
type Change = { pk: Row; key: string; before: Row; after: Row };
type Prefs = { widths?: Record<string, number>; hidden?: string[] };

const loadPrefs = (k: string): Prefs => { try { return JSON.parse(localStorage.getItem(k) || "{}"); } catch { return {}; } };
const savePrefs = (k: string, p: Prefs) => { try { localStorage.setItem(k, JSON.stringify(p)); } catch { /* private mode */ } };

const baseWidth = (c: Column) => {
  const k = kindOf(c.type);
  const w = k === "num" ? 110 : k === "bool" ? 90 : k === "date" ? 170 : k === "json" ? 240 : 180;
  return Math.min(320, Math.max(w, c.name.length * 8 + 60));
};

/** A WHERE clause (and its params) for the search box and the filters. */
function where(cols: Column[], search: string, filters: Filter[], params: unknown[]) {
  const ph = (v: unknown) => { params.push(v); return `$${params.length}`; };
  const parts: string[] = [];
  if (search) {
    const s = ph(`%${search.toLowerCase()}%`);
    parts.push(`(${cols.map((c) => `lower(CAST(${qi(c.name)} AS text)) LIKE ${s}`).join(" OR ")})`);
  }
  for (const f of filters) {
    const c = cols.find((x) => x.name === f.col);
    if (!c) continue;
    const q = qi(c.name), kind = kindOf(c.type);
    // compare as the column's own type for numbers and dates, as text otherwise
    const typed = (kind === "num" || kind === "date") && /^[a-z0-9 _(),]+$/i.test(c.type);
    const lhs = typed ? q : `CAST(${q} AS text)`, rhs = (v: string) => (typed ? `CAST(${ph(v)} AS ${c.type})` : ph(v));
    switch (f.op) {
      case "contains": parts.push(`lower(CAST(${q} AS text)) LIKE ${ph(`%${f.value.toLowerCase()}%`)}`); break;
      case "ncontains": parts.push(`(${q} IS NULL OR lower(CAST(${q} AS text)) NOT LIKE ${ph(`%${f.value.toLowerCase()}%`)})`); break;
      case "eq": parts.push(`${lhs} = ${rhs(f.value)}`); break;
      case "neq": parts.push(`(${q} IS NULL OR ${lhs} <> ${rhs(f.value)})`); break;
      case "gt": parts.push(`${lhs} > ${rhs(f.value)}`); break;
      case "gte": parts.push(`${lhs} >= ${rhs(f.value)}`); break;
      case "lt": parts.push(`${lhs} < ${rhs(f.value)}`); break;
      case "lte": parts.push(`${lhs} <= ${rhs(f.value)}`); break;
      case "empty": parts.push(`(${q} IS NULL OR CAST(${q} AS text) = '')`); break;
      case "nempty": parts.push(`(${q} IS NOT NULL AND CAST(${q} AS text) <> '')`); break;
    }
  }
  return parts.length ? ` WHERE ${parts.join(" AND ")}` : "";
}

const csv = (rows: Row[], cols: string[]) => {
  const cell = (v: unknown) => { const s = editText(v); return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s; };
  return [cols.map(cell).join(","), ...rows.map((r) => cols.map((c) => cell(r[c])).join(","))].join("\r\n");
};

export function TableSheet({ t, role, client, live, prefsKey, onWrite, right }: {
  t: Table; role: string; client: StoreClient; live: boolean; prefsKey: string; onWrite: () => void; right?: ReactNode;
}) {
  const cols = readCols(t);
  const [prefs, setPrefs] = useState<Prefs>(() => loadPrefs(prefsKey));
  const [search, setSearch] = useState("");
  const [searchText, setSearchText] = useState("");
  const [filters, setFilters] = useState<Filter[]>([]);
  const [sort, setSort] = useState<Sort | null>(null);
  const [data, setData] = useState<{ rows: Row[]; total: number; error?: string }>({ rows: [], total: 0 });
  const [loading, setLoading] = useState(true);
  const [draft, setDraft] = useState<Record<string, string> | null>(null);
  const [marks, setMarks] = useState<Record<string, Mark>>({});
  const [pop, setPop] = useState<{ kind: "filter" | "columns"; anchor: DOMRect; col?: string } | null>(null);
  const [history, setHistory] = useState({ undo: 0, redo: 0 });
  const undo = useRef<Change[][]>([]), redo = useRef<Change[][]>([]);
  const gen = useRef(0);
  const searchBox = useRef<HTMLInputElement>(null);

  const canUpdate = !!t.tools.update && pkReadable(t);
  const canDelete = !!t.tools.delete && pkReadable(t);
  const canCreate = !!t.tools.create;
  const rowKey = useCallback((r: Row) => JSON.stringify(t.primary_key.map((k) => r[k])), [t]);
  const markKey = useCallback((r: Row, col: string) => `${rowKey(r)}|${col}`, [rowKey]);

  // search as you type, a beat after the last key
  useEffect(() => { const h = setTimeout(() => setSearch(searchText.trim()), 250); return () => clearTimeout(h); }, [searchText]);

  const order = useMemo(() => {
    const pk = t.primary_key.filter((k) => cols.some((c) => c.name === k));
    const by = sort ? [`${qi(sort.col)} ${sort.dir === "desc" ? "DESC" : "ASC"} NULLS LAST`] : [];
    const rest = pk.filter((k) => k !== sort?.col).map((k) => qi(k));
    const all = [...by, ...rest];
    return all.length ? ` ORDER BY ${all.join(", ")}` : "";
  }, [t, cols, sort]);

  const select = (w: string, limit: number, offset: number) =>
    `SELECT ${cols.map((c) => qi(c.name)).join(", ")} FROM ${qi(t.name)}${w}${order} LIMIT ${limit} OFFSET ${offset}`;

  // (re)load from the top: a new filter, sort or search, or a refresh (which keeps as many rows as were loaded)
  const load = useCallback((keep = 0) => {
    if (!t.read || !cols.length) return;
    const n = ++gen.current, params: unknown[] = [];
    const w = where(cols, search, filters, params);
    setLoading(true);
    Promise.all([client.query(select(w, Math.min(EXPORT_CAP, Math.max(PAGE, keep)), 0), params, EXPORT_CAP), client.query(`SELECT count(*) AS n FROM ${qi(t.name)}${w}`, params, 1)])
      .then(([rows, cnt]) => { if (n === gen.current) setData({ rows, total: Number(cnt[0]?.n ?? rows.length) }); })
      .catch((e) => { if (n === gen.current) setData({ rows: [], total: 0, error: errMsg(e) }); })
      .finally(() => { if (n === gen.current) setLoading(false); });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, t, search, filters, order]);
  useEffect(() => { load(); }, [load]);

  const loadMore = () => {
    if (loading || data.rows.length >= data.total) return;
    const n = gen.current, params: unknown[] = [];
    const w = where(cols, search, filters, params);
    setLoading(true);
    client.query(select(w, PAGE, data.rows.length), params, PAGE)
      .then((more) => { if (n === gen.current) setData((d) => ({ ...d, rows: [...d.rows, ...more] })); })
      .catch((e) => say(errMsg(e), true))
      .finally(() => { if (n === gen.current) setLoading(false); });
  };
  const refresh = () => load(data.rows.length);

  const updatePrefs = (f: (p: Prefs) => Prefs) => setPrefs((p) => { const q = f(p); savePrefs(prefsKey, q); return q; });
  const hidden = new Set(prefs.hidden || []);

  const sheetCols: SheetCol[] = cols.filter((c) => !hidden.has(c.name)).map((c) => ({
    key: c.name, type: c.type, kind: kindOf(c.type), width: prefs.widths?.[c.name] ?? baseWidth(c),
    editable: canUpdate && !!c.update && !c.pk, creatable: canCreate && !!c.create,
    nullable: c.nullable, pk: c.pk, approval: c.approval, default: c.default,
    fk: c.fk ? `${c.fk.table}.${c.fk.column}` : undefined,
  }));
  // a column the role may create but not read still belongs in the new row
  const createOnly = t.columns.filter((c) => c.create && !c.read);

  const parked = (r: WriteResult) => !!r?.pending_approval;
  const setMark = (updates: Record<string, Mark | null>) => setMarks((ms) => {
    const next = { ...ms };
    for (const [k, v] of Object.entries(updates)) if (v) next[k] = v; else delete next[k];
    return next;
  });
  const patchRow = (key: string, patch: Row) => setData((d) => ({ ...d, rows: d.rows.map((r) => (rowKey(r) === key ? { ...r, ...patch } : r)) }));

  /** Writes changed cells of existing rows, one update call per row; returns the changes that applied. */
  async function write(changes: Change[], quiet = false): Promise<Change[]> {
    const done: Change[] = [];
    let parkedN = 0, failed = 0;
    await Promise.all(changes.map(async (ch) => {
      const cells = Object.keys(ch.after);
      patchRow(ch.key, ch.after);
      setMark(Object.fromEntries(cells.map((c) => [`${ch.key}|${c}`, { tone: "saving", note: "saving…" } as Mark])));
      try {
        const r = await client.call<WriteResult>(t.tools.update!, { ...ch.pk, ...ch.after });
        if (parked(r)) {
          parkedN++;
          patchRow(ch.key, ch.before);
          setMark(Object.fromEntries(cells.map((c) => [`${ch.key}|${c}`, { tone: "pending", note: `${editText(ch.after[c])}: waiting for approval${r?.group ? " by " + r.group : ""}` } as Mark])));
        } else {
          done.push(ch);
          setMark(Object.fromEntries(cells.map((c) => [`${ch.key}|${c}`, null])));
        }
      } catch (e) {
        failed++;
        patchRow(ch.key, ch.before);
        setMark(Object.fromEntries(cells.map((c) => [`${ch.key}|${c}`, { tone: "error", note: errMsg(e) } as Mark])));
        if (failed === 1) say(errMsg(e), true);
      }
    }));
    const cells = done.reduce((n, ch) => n + Object.keys(ch.after).length, 0);
    if (parkedN) say(`${parkedN} change${parkedN === 1 ? "" : "s"} parked for approval: ${parkedN === 1 ? "it applies" : "they apply"} once decided in Approvals`);
    else if (!quiet && cells > 1 && !failed) say(`${cells} cells saved`);
    return done;
  }

  async function onEdit(edits: Edit[]) {
    const rows = data.rows;
    const draftEdits = edits.filter((e) => e.row >= rows.length);
    if (draftEdits.length) setDraft((d) => ({ ...(d || {}), ...Object.fromEntries(draftEdits.map((e) => [e.col, e.raw])) }));
    const rowEdits = edits.filter((e) => e.row < rows.length);
    if (!rowEdits.length) return;
    if (live && rowEdits.length > 1 && !(await confirm({
      title: `Change ${rowEdits.length} cells in the live database?`, confirm: "Change them", danger: true,
      body: <p>They are written to the <b>live</b> store now, row by row, through <code>{t.tools.update}</code>. ⌘Z writes the previous values back.</p>,
    }))) return;
    const byRow = new Map<number, Edit[]>();
    rowEdits.forEach((e) => byRow.set(e.row, [...(byRow.get(e.row) || []), e]));
    const changes: Change[] = [];
    for (const [i, es] of byRow) {
      const row = rows[i], before: Row = {}, after: Row = {};
      for (const e of es) {
        const c = cols.find((x) => x.name === e.col)!;
        try { after[e.col] = coerce(e.raw, c); before[e.col] = row[e.col]; }
        catch (err) { say(errMsg(err), true); }
      }
      if (Object.keys(after).length) changes.push({ pk: pkArgs(t, row), key: rowKey(row), before, after });
    }
    const done = await write(changes);
    if (done.length) { undo.current.push(done); redo.current = []; setHistory({ undo: undo.current.length, redo: 0 }); }
  }

  async function travel(back: boolean) {
    const from = back ? undo.current : redo.current, to = back ? redo.current : undo.current;
    const batch = from.pop();
    if (!batch) { say(back ? "nothing to undo" : "nothing to redo"); return; }
    const flipped = batch.map((ch) => (back ? { ...ch, before: ch.after, after: ch.before } : ch));
    const done = await write(flipped, true);
    if (done.length) to.push(batch);
    setHistory({ undo: undo.current.length, redo: redo.current.length });
    say(`${back ? "undone" : "redone"}: ${done.reduce((n, ch) => n + Object.keys(ch.after).length, 0)} cell(s)`);
  }

  async function saveDraft() {
    if (!draft) return;
    const args: Row = {};
    try {
      for (const c of t.columns.filter((c) => c.create)) { const v = draft[c.name] ?? ""; if (v !== "") args[c.name] = coerce(v, c); }
    } catch (e) { say(errMsg(e), true); return; }
    try {
      const r = await client.call<WriteResult>(t.tools.create!, args);
      say(parked(r) ? `new row parked for approval${r?.group ? " by " + r.group : ""}` : `${t.name} row added`);
      setDraft(null);
      refresh(); onWrite();
    } catch (e) { say(errMsg(e), true); }
  }

  async function deleteRows(idx: number[]) {
    const rows = idx.map((i) => data.rows[i]).filter(Boolean);
    if (!rows.length) return;
    const ok = await confirm({
      title: rows.length === 1 ? `Delete this ${t.name} row?` : `Delete ${rows.length} ${t.name} rows?`, danger: true, confirm: "Delete",
      body: <>
        {rows.length === 1 && <p><code>{Object.entries(pkArgs(t, rows[0])).map(([a, b]) => `${a} = ${editText(b)}`).join(", ")}</code></p>}
        {t.delete_is_soft
          ? <p>The row is marked deleted{t.soft_delete ? <> (<code>{t.soft_delete}</code>)</> : null}: it disappears for your role but stays in the database.</p>
          : <p>Deleting cannot be undone.</p>}
        {live && <p className="font-semibold text-rose-700 dark:text-rose-400">This deletes from the LIVE database.</p>}
      </>,
    });
    if (!ok) return;
    let gone = 0, parkedN = 0;
    const deleted = new Set<string>();
    for (const r of rows) {
      try {
        const res = await client.call<WriteResult>(t.tools.delete!, pkArgs(t, r));
        if (parked(res)) parkedN++; else { gone++; deleted.add(rowKey(r)); }
      } catch (e) { say(errMsg(e), true); break; }
    }
    setData((d) => ({ ...d, rows: d.rows.filter((r) => !deleted.has(rowKey(r))), total: d.total - deleted.size }));
    if (gone) say(`${gone} row${gone === 1 ? "" : "s"} deleted${parkedN ? `, ${parkedN} parked for approval` : ""}`);
    else if (parkedN) say(`${parkedN} deletion${parkedN === 1 ? "" : "s"} parked for approval`);
    onWrite();
  }

  async function download() {
    const params: unknown[] = [];
    const w = where(cols, search, filters, params);
    try {
      const rows = await client.query(select(w, EXPORT_CAP, 0), params, EXPORT_CAP);
      const names = sheetCols.map((c) => c.key);
      const url = URL.createObjectURL(new Blob([csv(rows, names)], { type: "text/csv" }));
      const a = Object.assign(document.createElement("a"), { href: url, download: `${t.name}.csv` });
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      say(`${rows.length} row${rows.length === 1 ? "" : "s"} exported${rows.length === EXPORT_CAP ? ` (capped at ${EXPORT_CAP})` : ""}`);
    } catch (e) { say(errMsg(e), true); }
  }

  const readOnlyWhy = (c: SheetCol, isDraft: boolean) =>
    isDraft ? `role ${role} does not set ${c.key} when adding a row`
      : c.pk ? `${c.key} is the primary key: it cannot be changed`
      : !t.tools.update ? `role ${role} cannot update ${t.name} rows`
      : !pkReadable(t) ? `role ${role} cannot read the primary key of ${t.name}, so its rows cannot be updated`
      : `role ${role} cannot update ${c.key}`;

  if (!t.read || !cols.length) return <div className="p-6 text-zinc-500">role {role} reads no column of <b>{t.name}</b></div>;

  const addFilter = (f: Filter) => setFilters((fs) => [...fs, f]);

  return (
    <>
      {/* toolbar */}
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-zinc-200 bg-zinc-50 px-2 py-1.5 dark:border-zinc-800 dark:bg-zinc-900">
        <Tool label="Undo (⌘Z)" disabled={!history.undo} onClick={() => travel(true)}><Undo2 /></Tool>
        <Tool label="Redo (⇧⌘Z)" disabled={!history.redo} onClick={() => travel(false)}><Redo2 /></Tool>
        <Tool label="Reload" onClick={refresh}><RefreshCw className={cx(loading && "animate-spin")} /></Tool>
        <Divider />
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-zinc-400" />
          <Input ref={searchBox} aria-label="search rows" placeholder="Search" value={searchText}
            onChange={(e) => setSearchText(e.target.value)} onKeyDown={(e) => { if (e.key === "Escape") setSearchText(""); }}
            h="xs" className="w-44 pl-7" />
        </div>
        <Tool label="Filter" text={filters.length ? `Filter (${filters.length})` : "Filter"} active={filters.length > 0}
          onClick={(e) => setPop({ kind: "filter", anchor: e.currentTarget.getBoundingClientRect() })}><ListFilter /></Tool>
        <Tool label="Show or hide columns" text={hidden.size ? `Columns (${hidden.size} hidden)` : "Columns"} active={hidden.size > 0}
          onClick={(e) => setPop({ kind: "columns", anchor: e.currentTarget.getBoundingClientRect() })}><Columns3 /></Tool>
        <Divider />
        {canCreate && <Tool label="Add a row at the bottom" text="Row" disabled={!!draft} onClick={() => setDraft({})}><Plus /></Tool>}
        <Tool label="Download as CSV (with the current search and filters)" text="CSV" onClick={download}><Download /></Tool>
        <span className="ml-auto flex items-center gap-2">{right}</span>
      </div>

      {(filters.length > 0 || sort) && (
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-zinc-200 bg-white px-2 py-1.5 text-xs dark:border-zinc-800 dark:bg-zinc-900">
          {filters.map((f, i) => (
            <Chip key={i} onRemove={() => setFilters((fs) => fs.filter((_, j) => j !== i))}
              onClick={(e) => setPop({ kind: "filter", anchor: e.currentTarget.getBoundingClientRect() })}>
              <b className="font-medium">{f.col}</b> {OP_LABEL[f.op]}{f.op !== "empty" && f.op !== "nempty" && <> <b className="font-medium">{f.value}</b></>}
            </Chip>
          ))}
          {sort && <Chip onRemove={() => setSort(null)}>sorted by <b className="font-medium">{sort.col}</b> {sort.dir === "asc" ? "A → Z" : "Z → A"}</Chip>}
          {filters.length > 1 && <button className="cursor-pointer px-1 text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100" onClick={() => setFilters([])}>clear all</button>}
        </div>
      )}

      {data.error ? <div className="p-4"><ErrorNote>{data.error}</ErrorNote></div> : (
        <Sheet
          key={t.name}
          cols={sheetCols} rows={data.rows} total={data.total} hasMore={data.rows.length < data.total} loading={loading}
          draft={draft} sort={sort} marks={marks} markKey={markKey} readOnlyWhy={readOnlyWhy}
          onEdit={canUpdate || canCreate ? onEdit : undefined}
          onSort={(col, dir) => setSort({ col, dir })}
          onFilterValue={(col, v) => addFilter(v === null || v === undefined || v === "" ? { col, op: "empty", value: "" } : { col, op: "eq", value: editText(v) })}
          onFilterColumn={(col) => setPop({ kind: "filter", anchor: new DOMRect(Math.max(16, innerWidth / 2 - 210), 180, 0, 0), col })}
          onHide={(col) => updatePrefs((p) => ({ ...p, hidden: [...(p.hidden || []), col] }))}
          onResize={(col, w) => updatePrefs((p) => ({ ...p, widths: { ...p.widths, [col]: w } }))}
          onDeleteRows={canDelete ? deleteRows : undefined}
          onInsertRow={canCreate ? () => setDraft({}) : undefined}
          onDraftSave={saveDraft} onDraftCancel={() => setDraft(null)}
          onLoadMore={loadMore}
          onUndo={() => travel(true)} onRedo={() => travel(false)}
          onFind={() => searchBox.current?.focus()}
          empty={search || filters.length ? "no row matches" : t.scope === "hidden" ? "rows hidden: this table is not named in rules.yaml" : `no row${t.scope === "own" ? " of yours" : ""} yet`}
        />
      )}

      {draft && createOnly.length > 0 && (
        <div className="shrink-0 border-t border-zinc-200 bg-emerald-50/60 px-3 py-2 text-xs dark:border-zinc-800 dark:bg-emerald-500/5">
          <span className="mr-2 text-zinc-600 dark:text-zinc-300">Also set on the new row (not readable by {role}):</span>
          {createOnly.map((c) => (
            <label key={c.name} className="mr-3 inline-flex items-center gap-1.5">
              {c.name}
              <Input h="xs" className="w-36" placeholder={c.type} value={draft[c.name] ?? ""} onChange={(e) => setDraft((d) => ({ ...(d || {}), [c.name]: e.target.value }))} />
            </label>
          ))}
        </div>
      )}

      {pop?.kind === "filter" && (
        <Popover anchor={pop.anchor} onClose={() => setPop(null)} className="w-[30rem] max-w-[calc(100vw-2rem)]">
          <FilterEditor cols={sheetCols} filters={filters} add={pop.col}
            onChange={(f) => { setFilters(f); setPop(null); }} defaultCol={pop.col || sheetCols[0]?.key || ""} />
        </Popover>
      )}
      {pop?.kind === "columns" && (
        <Popover anchor={pop.anchor} onClose={() => setPop(null)} className="w-60">
          <div className="mb-2 flex items-center justify-between text-xs text-zinc-500">
            <span>{cols.length - hidden.size} of {cols.length} shown</span>
            <span className="flex gap-2">
              <button className="cursor-pointer hover:text-zinc-900 dark:hover:text-zinc-100" onClick={() => updatePrefs((p) => ({ ...p, hidden: [] }))}>show all</button>
              <button className="cursor-pointer hover:text-zinc-900 dark:hover:text-zinc-100" onClick={() => updatePrefs((p) => ({ ...p, widths: {} }))}>reset widths</button>
            </span>
          </div>
          <div className="max-h-80 overflow-auto">
            {cols.map((c) => (
              <label key={c.name} className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-zinc-100 dark:hover:bg-zinc-800">
                <input type="checkbox" className="accent-brand-600" checked={!hidden.has(c.name)}
                  disabled={!hidden.has(c.name) && cols.length - hidden.size <= 1}
                  onChange={(e) => updatePrefs((p) => ({ ...p, hidden: e.target.checked ? (p.hidden || []).filter((h) => h !== c.name) : [...(p.hidden || []), c.name] }))} />
                <span className="truncate">{c.name}</span>
                <span className="ml-auto text-xs text-zinc-400">{c.type}</span>
              </label>
            ))}
          </div>
        </Popover>
      )}
    </>
  );
}

function FilterEditor({ cols, filters, onChange, defaultCol, add }: { cols: SheetCol[]; filters: Filter[]; onChange: (f: Filter[]) => void; defaultCol: string; add?: string }) {
  // edited locally, applied on Enter or "Apply", so the table does not reload on every keystroke
  const [rows, setRows] = useState<Filter[]>(filters.length && !add ? filters : [...filters, { col: defaultCol, op: "contains", value: "" }]);
  const set = (i: number, f: Partial<Filter>) => setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...f } : r)));
  const apply = () => onChange(rows.filter((r) => r.col && (r.op === "empty" || r.op === "nempty" || r.value !== "")));
  return (
    <form onSubmit={(e) => { e.preventDefault(); apply(); }} className="space-y-2">
      <div className="text-xs font-medium text-zinc-500">Show rows where</div>
      {rows.map((f, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <span className="w-8 shrink-0 text-right text-xs text-zinc-400">{i ? "and" : ""}</span>
          <Select h="xs" className="w-36" value={f.col} onChange={(e) => set(i, { col: e.target.value })}>
            {cols.map((c) => <option key={c.key}>{c.key}</option>)}
          </Select>
          <Select h="xs" className="w-32" value={f.op} onChange={(e) => set(i, { op: e.target.value as Op })}>
            {OPS.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
          </Select>
          <Input h="xs" className="w-auto min-w-0 flex-1" autoFocus={i === rows.length - 1} value={f.value} disabled={f.op === "empty" || f.op === "nempty"}
            onChange={(e) => set(i, { value: e.target.value })} placeholder="value" />
          <button type="button" aria-label="remove filter" className="cursor-pointer text-zinc-400 hover:text-zinc-800 dark:hover:text-zinc-100"
            onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}><X className="size-3.5" /></button>
        </div>
      ))}
      <div className="flex items-center gap-2 pt-1">
        <Button size="xs" variant="ghost" onClick={() => setRows((rs) => [...rs, { col: defaultCol, op: "contains", value: "" }])}><PlusSmall />Add condition</Button>
        <span className="ml-auto flex gap-2">
          {filters.length > 0 && <Button size="xs" variant="ghost" onClick={() => { setRows([]); onChange([]); }}>Clear</Button>}
          <Button size="xs" variant="primary" type="submit">Apply</Button>
        </span>
      </div>
    </form>
  );
}
const PlusSmall = () => <Plus className="size-3.5" />;

function Tool({ label, text, children, onClick, disabled, active }: {
  label: string; text?: string; children: ReactNode; onClick: (e: React.MouseEvent<HTMLButtonElement>) => void; disabled?: boolean; active?: boolean;
}) {
  return (
    <button type="button" title={label} aria-label={label} disabled={disabled} onClick={onClick}
      className={cx("inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-xs text-zinc-700 hover:bg-zinc-200/70 disabled:cursor-default disabled:opacity-35 disabled:hover:bg-transparent dark:text-zinc-300 dark:hover:bg-zinc-800 [&>svg]:size-4",
        active && "bg-brand-50 text-brand-700 hover:bg-brand-100 dark:bg-brand-500/15 dark:text-brand-100")}>
      {children}{text && <span className="pr-0.5">{text}</span>}
    </button>
  );
}
const Divider = () => <span className="mx-1 h-5 w-px bg-zinc-200 dark:bg-zinc-700" />;
function Chip({ children, onRemove, onClick }: { children: ReactNode; onRemove: () => void; onClick?: (e: React.MouseEvent<HTMLButtonElement>) => void }) {
  return (
    <span className="inline-flex items-center rounded-full bg-brand-50 text-brand-700 ring-1 ring-brand-600/15 dark:bg-brand-500/15 dark:text-brand-100">
      <button type="button" onClick={onClick} className={cx("py-0.5 pl-2.5", onClick ? "cursor-pointer" : "cursor-default")}>{children}</button>
      <button type="button" aria-label="remove" onClick={onRemove} className="ml-0.5 cursor-pointer rounded-full p-1 hover:bg-brand-100 dark:hover:bg-brand-500/25"><X className="size-3" /></button>
    </span>
  );
}

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent, type ReactNode } from "react";
import {
  ArrowDownWideNarrow, ArrowUpNarrowWide, Braces, Calendar, Check, ChevronDown, ClipboardPaste, Copy, Eraser, EyeOff, Hash, KeyRound,
  ListFilter, Lock, Plus, Scissors, ToggleLeft, Trash2, Type, X,
} from "lucide-react";
import type { Row } from "../../lib/store";
import { cx } from "../../ui/core";
import { say } from "./say";
import { Menu, type MenuItem } from "./Menu";

// A spreadsheet over a page of rows: a formula bar, lettered columns and numbered rows, a cell selection driven by the
// mouse and the keyboard (arrows, Tab, Enter, F2, Delete, ⌘C/⌘X/⌘V, ⌘Z), in-cell editing, resizable columns, header and
// cell menus, and a status bar that sums the selection. It holds no data of its own: every change goes out as `onEdit`.

export type Kind = "num" | "bool" | "date" | "json" | "text";
export const kindOf = (type = ""): Kind => {
  const t = type.toLowerCase();
  if (/int|serial|numeric|decimal|double|real|float|money/.test(t)) return "num";
  if (/bool/.test(t)) return "bool";
  if (/date|time/.test(t)) return "date";
  if (/json/.test(t)) return "json";
  return "text";
};

export type SheetCol = {
  key: string; type?: string; kind: Kind; width: number;
  /** editable in an existing row / in the new row */
  editable?: boolean; creatable?: boolean;
  nullable?: boolean; pk?: boolean; approval?: boolean; fk?: string; default?: unknown;
};
/** A cell change; `row === rows.length` is the new row being drafted. `raw` is the typed text ("" clears). */
export type Edit = { row: number; col: string; raw: string };
export type Mark = { tone: "saving" | "pending" | "error"; note: string };

type Pos = { r: number; c: number };
type Editing = Pos & { text: string; original: string; mode: "enter" | "edit"; bar?: boolean };
type MenuState = { x: number; y: number; items: MenuItem[] };

const ROW_H = 28, LETTER_H = 20, NAME_H = 32, HEAD_H = LETTER_H + NAME_H, GUTTER = 56, OVERSCAN = 12;

export const letter = (i: number): string => (i >= 26 ? letter(Math.floor(i / 26) - 1) : "") + String.fromCharCode(65 + (i % 26));
/** A value as the text you edit: null is empty, objects are JSON. */
export const editText = (v: unknown) => (v === null || v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));
const shown = (v: unknown, kind: Kind) => {
  const s = editText(v);
  return kind === "date" ? s.replace("T", " ").replace(/\.\d+/, "").replace(/(\+00(:00)?|Z)$/, "") : s;
};
const KIND_ICON: Record<Kind, ReactNode> = { num: <Hash />, bool: <ToggleLeft />, date: <Calendar />, json: <Braces />, text: <Type /> };

/** Tab-separated text (as spreadsheets put on the clipboard) into rows of cells; quoted cells may hold tabs and newlines. */
export function parseTsv(s: string): string[][] {
  const out: string[][] = [[]];
  let cell = "", i = 0, quoted = false;
  s = s.replace(/\r\n?/g, "\n").replace(/\n$/, "");
  while (i < s.length) {
    const ch = s[i];
    if (quoted) {
      if (ch === '"' && s[i + 1] === '"') { cell += '"'; i += 2; continue; }
      if (ch === '"') { quoted = false; i++; continue; }
      cell += ch; i++; continue;
    }
    if (ch === '"' && cell === "") { quoted = true; i++; continue; }
    if (ch === "\t") { out[out.length - 1].push(cell); cell = ""; i++; continue; }
    if (ch === "\n") { out[out.length - 1].push(cell); out.push([]); cell = ""; i++; continue; }
    cell += ch; i++;
  }
  out[out.length - 1].push(cell);
  return out;
}
const tsvCell = (s: string) => (/[\t\n"]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s);

let measurer: CanvasRenderingContext2D | null = null;
const textWidth = (s: string, bold = false) => {
  measurer ||= document.createElement("canvas").getContext("2d");
  if (!measurer) return s.length * 7;
  measurer.font = `${bold ? 600 : 400} 13px ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif`;
  return measurer.measureText(s).width;
};

export function Sheet(p: {
  cols: SheetCol[]; rows: Row[]; total: number; hasMore: boolean; loading: boolean;
  /** the row being drafted (raw texts), shown after the loaded rows */
  draft?: Record<string, string> | null;
  sort?: { col: string; dir: "asc" | "desc" } | null;
  marks?: Record<string, Mark>; markKey?: (row: Row, col: string) => string;
  /** why a column cannot be typed into, for the toast */
  readOnlyWhy?: (col: SheetCol, draft: boolean) => string;
  onEdit?: (edits: Edit[]) => void;
  onSort?: (col: string, dir: "asc" | "desc") => void;
  onFilterValue?: (col: string, value: unknown) => void;
  onFilterColumn?: (col: string) => void;
  onHide?: (col: string) => void;
  onResize?: (col: string, width: number) => void;
  onDeleteRows?: (rows: number[]) => void;
  onInsertRow?: () => void;
  onDraftSave?: () => void;
  onDraftCancel?: () => void;
  onLoadMore?: () => void;
  onUndo?: () => void; onRedo?: () => void;
  onFind?: () => void;
  empty?: ReactNode;
}) {
  const { cols, rows, draft } = p;
  const n = rows.length + (draft ? 1 : 0), m = cols.length;
  const scroller = useRef<HTMLDivElement>(null);
  const [sel, setSel] = useState<{ a: Pos; f: Pos }>({ a: { r: 0, c: 0 }, f: { r: 0, c: 0 } });
  const [editing, setEditingState] = useState<Editing | null>(null);
  // the editor of record: a blur that follows an Enter must not commit the same edit twice
  const edRef = useRef<Editing | null>(null);
  const setEditing = (v: Editing | null | ((e: Editing | null) => Editing | null)) => {
    edRef.current = typeof v === "function" ? v(edRef.current) : v;
    setEditingState(edRef.current);
  };
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [widths, setWidths] = useState<Record<string, number>>({});
  const [view, setView] = useState({ top: 0, height: 600 });
  const dragging = useRef<"cells" | "rows" | "cols" | null>(null);

  const width = (c: SheetCol) => widths[c.key] ?? c.width;
  const lefts = useMemo(() => { const l = [0]; cols.forEach((c) => l.push(l[l.length - 1] + width(c))); return l; },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [cols, widths]);

  const isDraft = (r: number) => !!draft && r === rows.length;
  const valueAt = (r: number, c: number): unknown => (isDraft(r) ? draft![cols[c].key] : rows[r]?.[cols[c].key]);
  const canEdit = (r: number, c: number) => (isDraft(r) ? !!cols[c]?.creatable : !!cols[c]?.editable) && !!p.onEdit;
  const clampPos = useCallback((q: Pos): Pos => ({ r: Math.max(0, Math.min(n - 1, q.r)), c: Math.max(0, Math.min(m - 1, q.c)) }), [n, m]);

  // keep the selection inside the grid when rows or columns change (a new filter, a hidden column)
  useEffect(() => {
    setSel((s) => {
      const a = clampPos(s.a), f = clampPos(s.f);
      return a.r === s.a.r && a.c === s.a.c && f.r === s.f.r && f.c === s.f.c ? s : { a, f };
    });
  }, [clampPos]);

  const range = {
    r0: Math.min(sel.a.r, sel.f.r), r1: Math.max(sel.a.r, sel.f.r),
    c0: Math.min(sel.a.c, sel.f.c), c1: Math.max(sel.a.c, sel.f.c),
  };
  const inRange = (r: number, c: number) => r >= range.r0 && r <= range.r1 && c >= range.c0 && c <= range.c1;
  const active = sel.a;

  const focusGrid = () => scroller.current?.focus({ preventScroll: true });

  const scrollTo = useCallback((q: Pos) => {
    const el = scroller.current;
    if (!el) return;
    const top = q.r * ROW_H, bodyH = el.clientHeight - HEAD_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + bodyH) el.scrollTop = top + ROW_H - bodyH;
    const left = lefts[q.c] ?? 0, right = lefts[q.c + 1] ?? left, bodyW = el.clientWidth - GUTTER;
    if (left < el.scrollLeft) el.scrollLeft = left;
    else if (right > el.scrollLeft + bodyW) el.scrollLeft = Math.min(left, right - bodyW);
  }, [lefts]);

  const select = (a: Pos, f: Pos = a) => { const ca = clampPos(a), cf = clampPos(f); setSel({ a: ca, f: cf }); scrollTo(cf); };

  // a new draft row takes the selection, on its first creatable column
  const hadDraft = useRef(false);
  useEffect(() => {
    if (draft && !hadDraft.current) {
      const c = Math.max(0, cols.findIndex((x) => x.creatable));
      const q = { r: rows.length, c };
      setSel({ a: q, f: q });
      requestAnimationFrame(() => { scrollTo(q); focusGrid(); });
    }
    hadDraft.current = !!draft;
  }, [draft, rows.length, cols, scrollTo]);

  // viewport for row virtualization, and loading more rows as the bottom comes into view
  const onScroll = () => { const el = scroller.current; if (el) setView({ top: el.scrollTop, height: el.clientHeight }); };
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(onScroll);
    ro.observe(el);
    onScroll();
    return () => ro.disconnect();
  }, []);
  const first = Math.max(0, Math.floor(view.top / ROW_H) - OVERSCAN);
  const last = Math.min(n, Math.ceil((view.top + view.height) / ROW_H) + OVERSCAN);
  useEffect(() => { if (p.hasMore && !p.loading && last >= rows.length - OVERSCAN) p.onLoadMore?.(); }, [last, rows.length, p.hasMore, p.loading]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const up = () => { dragging.current = null; };
    window.addEventListener("mouseup", up);
    return () => window.removeEventListener("mouseup", up);
  }, []);

  // ---- editing
  function startEdit(q: Pos, text?: string, bar = false) {
    const col = cols[q.c];
    if (!col) return;
    if (!canEdit(q.r, q.c)) {
      if (p.onEdit) say(p.readOnlyWhy?.(col, isDraft(q.r)) ?? `${col.key} is read-only`, true);
      return;
    }
    const original = editText(valueAt(q.r, q.c));
    if (col.kind === "bool" && !bar) {
      // a checkbox: typing t/y/1 checks it, f/n/0 unchecks it, anything else (or Enter) flips it
      if (text === undefined || !/^[tyx1fn0]$/i.test(text)) { toggle(q); return; }
      if (editText(valueAt(q.r, q.c)) !== String(/^[tyx1]$/i.test(text))) p.onEdit!([{ row: q.r, col: col.key, raw: String(/^[tyx1]$/i.test(text)) }]);
      return;
    }
    setEditing({ ...q, original, text: text ?? original, mode: text === undefined ? "edit" : "enter", bar });
  }
  function toggle(q: Pos) {
    if (!canEdit(q.r, q.c)) return startEdit(q, "");
    const v = valueAt(q.r, q.c);
    p.onEdit!([{ row: q.r, col: cols[q.c].key, raw: String(!(v === true || v === "true")) }]);
  }
  function commit(move?: [number, number]) {
    const e = edRef.current;
    if (!e) return;
    setEditing(null);
    if (e.text !== e.original) p.onEdit?.([{ row: e.r, col: cols[e.c].key, raw: e.text }]);
    if (move) select({ r: e.r + move[0], c: e.c + move[1] });
    focusGrid();
  }
  const cancel = () => { setEditing(null); focusGrid(); };

  function editorKeys(e: KeyboardEvent<HTMLInputElement>) {
    e.stopPropagation();
    const k = e.key;
    if (k === "Enter" && (e.metaKey || e.ctrlKey) && editing && isDraft(editing.r)) { e.preventDefault(); commit(); setTimeout(() => p.onDraftSave?.()); return; }
    if (k === "Enter") { e.preventDefault(); commit([e.shiftKey ? -1 : 1, 0]); }
    else if (k === "Tab") { e.preventDefault(); commit([0, e.shiftKey ? -1 : 1]); }
    else if (k === "Escape") { e.preventDefault(); cancel(); }
    else if (editing?.mode === "enter" && !editing.bar && (k === "ArrowUp" || k === "ArrowDown" || k === "ArrowLeft" || k === "ArrowRight")) {
      e.preventDefault();
      commit(k === "ArrowUp" ? [-1, 0] : k === "ArrowDown" ? [1, 0] : k === "ArrowLeft" ? [0, -1] : [0, 1]);
    }
  }

  // ---- range operations
  const rangeCells = () => {
    const out: Pos[] = [];
    for (let r = range.r0; r <= range.r1; r++) for (let c = range.c0; c <= range.c1; c++) out.push({ r, c });
    return out;
  };
  const rangeTsv = () => {
    const lines: string[] = [];
    for (let r = range.r0; r <= range.r1; r++) {
      const cells: string[] = [];
      for (let c = range.c0; c <= range.c1; c++) cells.push(tsvCell(editText(valueAt(r, c))));
      lines.push(cells.join("\t"));
    }
    return lines.join("\n");
  };
  function clearRange() {
    const cells = rangeCells().filter((q) => valueAt(q.r, q.c) !== null && valueAt(q.r, q.c) !== undefined && valueAt(q.r, q.c) !== "");
    const edits = cells.filter((q) => canEdit(q.r, q.c)).map((q) => ({ row: q.r, col: cols[q.c].key, raw: "" }));
    if (cells.length && !edits.length) { startEdit(active, ""); return; }
    if (edits.length) p.onEdit?.(edits);
    if (edits.length < cells.length) say(`${cells.length - edits.length} read-only cell${cells.length - edits.length === 1 ? "" : "s"} left as is`);
  }
  function paste(text: string) {
    if (!p.onEdit) return;
    const data = parseTsv(text);
    const h = data.length, w = Math.max(...data.map((d) => d.length));
    const fill = h === 1 && w === 1 && (range.r1 > range.r0 || range.c1 > range.c0);
    const r1 = fill ? range.r1 : range.r0 + h - 1, c1 = fill ? range.c1 : range.c0 + w - 1;
    const edits: Edit[] = [];
    let skipped = 0;
    for (let r = range.r0; r <= r1; r++) for (let c = range.c0; c <= c1; c++) {
      const raw = fill ? data[0][0] : data[r - range.r0]?.[c - range.c0];
      if (raw === undefined) continue;
      if (r >= n || c >= m || !canEdit(r, c)) { skipped++; continue; }
      if (raw !== editText(valueAt(r, c))) edits.push({ row: r, col: cols[c].key, raw });
    }
    if (edits.length) p.onEdit(edits);
    if (skipped) say(`${skipped} cell${skipped === 1 ? "" : "s"} not pasted: read-only or outside the loaded rows`, !edits.length);
    select(range.r0 === sel.a.r && range.c0 === sel.a.c ? sel.a : { r: range.r0, c: range.c0 }, { r: Math.min(r1, n - 1), c: Math.min(c1, m - 1) });
  }

  // ---- keyboard
  function onKey(e: KeyboardEvent<HTMLDivElement>) {
    if (editing || !m) return;
    const mod = e.metaKey || e.ctrlKey, k = e.key;
    const page = Math.max(1, Math.floor((view.height - HEAD_H) / ROW_H) - 1);
    const move = (dr: number, dc: number) => {
      e.preventDefault();
      const from = e.shiftKey ? sel.f : sel.a;
      let to = { r: from.r + dr, c: from.c + dc };
      if (mod && dr) to = { r: dr < 0 ? 0 : n - 1, c: from.c };
      if (mod && dc) to = { r: from.r, c: dc < 0 ? 0 : m - 1 };
      to = clampPos(to);
      if (e.shiftKey) { setSel({ a: sel.a, f: to }); scrollTo(to); } else select(to);
    };
    if (k === "ArrowUp") return move(-1, 0);
    if (k === "ArrowDown") return move(1, 0);
    if (k === "ArrowLeft") return move(0, -1);
    if (k === "ArrowRight") return move(0, 1);
    if (k === "PageUp") return move(-page, 0);
    if (k === "PageDown") return move(page, 0);
    if (k === "Home") { e.preventDefault(); return select(mod ? { r: 0, c: 0 } : { r: active.r, c: 0 }); }
    if (k === "End") { e.preventDefault(); return select(mod ? { r: n - 1, c: m - 1 } : { r: active.r, c: m - 1 }); }
    if (k === "Tab") { e.preventDefault(); return select({ r: active.r, c: active.c + (e.shiftKey ? -1 : 1) }); }
    if (mod && k === "a") { e.preventDefault(); return setSel({ a: { r: 0, c: 0 }, f: { r: n - 1, c: m - 1 } }); }
    if (mod && k === "z") { e.preventDefault(); return e.shiftKey ? p.onRedo?.() : p.onUndo?.(); }
    if (mod && k === "y") { e.preventDefault(); return p.onRedo?.(); }
    if (mod && k === "f") { e.preventDefault(); return p.onFind?.(); }
    if (mod && k === "Enter" && isDraft(active.r)) { e.preventDefault(); return p.onDraftSave?.(); }
    if (k === "Escape") {
      if (isDraft(active.r) && !(range.r1 > range.r0 || range.c1 > range.c0)) return p.onDraftCancel?.();
      return setSel({ a: active, f: active });
    }
    if (k === "Enter" || k === "F2") { e.preventDefault(); return startEdit(active); }
    if (k === "Delete" || k === "Backspace") { e.preventDefault(); return clearRange(); }
    if (k === " " && cols[active.c]?.kind === "bool") { e.preventDefault(); return toggle(active); }
    if (!mod && !e.altKey && k.length === 1) { e.preventDefault(); startEdit(active, k); }
  }
  function onCopy(e: ClipboardEvent<HTMLDivElement>) {
    if (editing || !n) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", rangeTsv());
  }
  function onCut(e: ClipboardEvent<HTMLDivElement>) { if (editing || !n) return; onCopy(e); clearRange(); }
  function onPaste(e: ClipboardEvent<HTMLDivElement>) {
    if (editing || !n) return;
    e.preventDefault();
    paste(e.clipboardData.getData("text/plain"));
  }
  const copyToClipboard = (cut = false) => navigator.clipboard?.writeText(rangeTsv()).then(() => { if (cut) clearRange(); }, () => say("the browser refused the clipboard: use ⌘C", true));
  const pasteFromClipboard = () => navigator.clipboard?.readText().then(paste, () => say("the browser refused the clipboard: use ⌘V", true));

  // ---- mouse
  function cellDown(e: React.MouseEvent, q: Pos) {
    if (e.button !== 0) return;
    if (editing) commit();
    if (e.shiftKey) setSel({ a: sel.a, f: q }); else setSel({ a: q, f: q });
    dragging.current = "cells";
    focusGrid();
  }
  function cellEnter(q: Pos) {
    if (dragging.current === "cells") setSel((s) => ({ a: s.a, f: q }));
    else if (dragging.current === "rows") setSel((s) => ({ a: s.a, f: { r: q.r, c: m - 1 } }));
  }
  function rowDown(e: React.MouseEvent, r: number) {
    if (e.button !== 0) return;
    if (editing) commit();
    setSel((s) => (e.shiftKey ? { a: { r: s.a.r, c: 0 }, f: { r, c: m - 1 } } : { a: { r, c: 0 }, f: { r, c: m - 1 } }));
    dragging.current = "rows";
    focusGrid();
  }
  function colDown(e: React.MouseEvent, c: number) {
    if (e.button !== 0 || !n) return;
    if (editing) commit();
    setSel((s) => (e.shiftKey ? { a: { r: 0, c: s.a.c }, f: { r: n - 1, c } } : { a: { r: 0, c }, f: { r: n - 1, c } }));
    dragging.current = "cols";
    focusGrid();
  }
  function colEnter(c: number) { if (dragging.current === "cols") setSel((s) => ({ a: s.a, f: { r: n - 1, c } })); }

  function startResize(e: React.MouseEvent, col: SheetCol) {
    e.preventDefault(); e.stopPropagation();
    const x0 = e.clientX, w0 = width(col);
    let w = w0;
    const mv = (ev: MouseEvent) => { w = Math.max(48, Math.round(w0 + ev.clientX - x0)); setWidths((ws) => ({ ...ws, [col.key]: w })); };
    const up = () => { document.removeEventListener("mousemove", mv); document.removeEventListener("mouseup", up); document.body.style.cursor = ""; if (w !== w0) p.onResize?.(col.key, w); };
    document.addEventListener("mousemove", mv);
    document.addEventListener("mouseup", up);
    document.body.style.cursor = "col-resize";
  }
  function autofit(col: SheetCol) {
    let w = textWidth(col.key, true) + 56;
    for (const r of rows.slice(0, 500)) w = Math.max(w, textWidth(shown(r[col.key], col.kind)) + 24);
    w = Math.round(Math.min(480, Math.max(60, w)));
    setWidths((ws) => ({ ...ws, [col.key]: w }));
    p.onResize?.(col.key, w);
  }

  // ---- menus
  const sortItems = (col: SheetCol): MenuItem[] => p.onSort ? [
    { label: "Sort A → Z", icon: <ArrowUpNarrowWide />, onClick: () => p.onSort!(col.key, "asc") },
    { label: "Sort Z → A", icon: <ArrowDownWideNarrow />, onClick: () => p.onSort!(col.key, "desc") },
  ] : [];
  function columnMenu(x: number, y: number, c: number) {
    const col = cols[c];
    setMenu({
      x, y, items: [
        ...sortItems(col),
        ...(p.onFilterColumn ? [{ label: "Filter this column…", icon: <ListFilter />, onClick: () => p.onFilterColumn!(col.key) }] : []),
        "sep",
        { label: "Copy column", icon: <Copy />, onClick: () => { setSel({ a: { r: 0, c }, f: { r: n - 1, c } }); setTimeout(() => navigator.clipboard?.writeText(rows.map((r) => tsvCell(editText(r[col.key]))).join("\n"))); } },
        { label: "Fit to content", icon: <Type />, onClick: () => autofit(col) },
        ...(p.onHide ? [{ label: "Hide column", icon: <EyeOff />, onClick: () => p.onHide!(col.key) }] : []),
      ],
    });
  }
  function cellMenu(e: React.MouseEvent, q: Pos, wholeRow = false) {
    e.preventDefault();
    const hit = inRange(q.r, q.c) && (!wholeRow || (range.c0 === 0 && range.c1 === m - 1));
    const r = hit ? range : wholeRow ? { r0: q.r, r1: q.r, c0: 0, c1: m - 1 } : { r0: q.r, r1: q.r, c0: q.c, c1: q.c };
    if (!hit) setSel({ a: { r: r.r0, c: r.c0 }, f: { r: r.r1, c: r.c1 } });
    const rowsSel: number[] = [];
    for (let i = r.r0; i <= r.r1; i++) if (i < rows.length) rowsSel.push(i);
    const col = cols[q.c], v = valueAt(q.r, q.c);
    let anyEditable = false;
    for (let i = r.r0; i <= r.r1 && !anyEditable; i++) for (let j = r.c0; j <= r.c1; j++) if (canEdit(i, j)) { anyEditable = true; break; }
    const rowsLabel = rowsSel.length > 1 ? `rows ${rowsSel[0] + 1}–${rowsSel[rowsSel.length - 1] + 1}` : `row ${q.r + 1}`;
    const items: MenuItem[] = [
      { label: "Copy", icon: <Copy />, hint: "⌘C", onClick: () => copyToClipboard() },
      ...(p.onEdit ? [
        { label: "Cut", icon: <Scissors />, hint: "⌘X", disabled: !anyEditable, onClick: () => copyToClipboard(true) },
        { label: "Paste", icon: <ClipboardPaste />, hint: "⌘V", disabled: !anyEditable, onClick: pasteFromClipboard },
        { label: "Clear", icon: <Eraser />, hint: "⌫", disabled: !anyEditable, onClick: clearRange },
      ] as MenuItem[] : []),
      "sep",
      ...(p.onFilterValue && !isDraft(q.r) ? [{ label: <>Filter: {col.key} = <b className="font-medium">{v === null || v === undefined ? "empty" : shown(v, col.kind).slice(0, 24)}</b></>, icon: <ListFilter />, onClick: () => p.onFilterValue!(col.key, v) }] : []),
      ...sortItems(col),
      ...(p.onInsertRow || (p.onDeleteRows && rowsSel.length) ? ["sep" as const] : []),
      ...(p.onInsertRow && !draft ? [{ label: "Insert row", icon: <Plus />, onClick: p.onInsertRow }] : []),
      ...(p.onDeleteRows && rowsSel.length ? [{ label: `Delete ${rowsLabel}`, icon: <Trash2 />, danger: true, onClick: () => p.onDeleteRows!(rowsSel) }] : []),
    ];
    setMenu({ x: e.clientX, y: e.clientY, items });
  }

  // ---- status bar: sum, average, count of the selection, as spreadsheets do
  const stats = useMemo(() => {
    const cells = (range.r1 - range.r0 + 1) * (range.c1 - range.c0 + 1);
    if (cells < 2) return null;
    let count = 0, nums = 0, sum = 0, min = Infinity, max = -Infinity;
    for (let r = range.r0; r <= range.r1; r++) for (let c = range.c0; c <= range.c1; c++) {
      const v = valueAt(r, c);
      if (v === null || v === undefined || v === "") continue;
      count++;
      const x = typeof v === "number" ? v : cols[c].kind === "num" && typeof v === "string" && v.trim() !== "" ? Number(v) : NaN;
      if (!Number.isNaN(x)) { nums++; sum += x; min = Math.min(min, x); max = Math.max(max, x); }
    }
    return { cells, count, nums, sum, min, max };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [range.r0, range.r1, range.c0, range.c1, rows, draft, cols]);
  const num = (x: number) => x.toLocaleString(undefined, { maximumFractionDigits: 4 });

  // ---- formula bar
  const activeCol = cols[active.c];
  const activeVal = n ? valueAt(active.r, active.c) : undefined;
  const ref = !n || !m ? "" : range.r0 === range.r1 && range.c0 === range.c1
    ? `${letter(active.c)}${active.r + 1}`
    : `${letter(range.c0)}${range.r0 + 1}:${letter(range.c1)}${range.r1 + 1}`;
  const barValue = editing ? editing.text : activeCol?.kind === "json" && activeVal !== null && typeof activeVal === "object" ? JSON.stringify(activeVal) : editText(activeVal);
  const barEditable = !!n && !!m && canEdit(active.r, active.c);

  const totalW = lefts[m] ?? 0;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* formula bar */}
      <div className="flex h-9 shrink-0 items-center border-b border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex h-full w-28 shrink-0 items-center border-r border-zinc-200 px-3 font-mono text-xs text-zinc-600 dark:border-zinc-800 dark:text-zinc-300" title="selected cell">{ref}</div>
        <div className="flex h-full shrink-0 items-center px-2.5 font-serif text-sm text-zinc-400 italic select-none">fx</div>
        <input
          aria-label="cell value" spellCheck={false}
          value={barValue}
          readOnly={!barEditable}
          placeholder={n && activeVal === null ? "null" : ""}
          onFocus={() => { if (barEditable && !editing && activeCol?.kind !== "bool") setEditing({ ...active, original: editText(activeVal), text: editText(activeVal), mode: "edit", bar: true }); }}
          onChange={(e) => setEditing((ed) => (ed ? { ...ed, text: e.target.value } : { ...active, original: editText(activeVal), text: e.target.value, mode: "edit", bar: true }))}
          onKeyDown={editorKeys}
          onBlur={() => { if (editing?.bar) commit(); }}
          className={cx("h-full min-w-0 flex-1 bg-transparent pr-3 font-mono text-xs outline-none placeholder:text-zinc-400 placeholder:italic", !barEditable && "text-zinc-500")}
        />
        {activeCol && n > 0 && (
          <span className="mr-3 hidden shrink-0 items-center gap-1.5 text-xs text-zinc-400 sm:flex">
            <span className="[&>svg]:size-3.5">{KIND_ICON[activeCol.kind]}</span>{activeCol.type}
            {!barEditable && <><Lock className="size-3" />read-only</>}
            {barEditable && activeCol.approval && <span className="text-amber-600 dark:text-amber-400">· needs approval</span>}
          </span>
        )}
      </div>

      {/* grid */}
      <div ref={scroller} tabIndex={0} onScroll={onScroll} onKeyDown={onKey} onCopy={onCopy} onCut={onCut} onPaste={onPaste}
        className="relative min-h-0 flex-1 overflow-auto bg-white outline-none select-none dark:bg-zinc-950"
        role="grid" aria-rowcount={n} aria-colcount={m}>
        <table className="border-separate border-spacing-0 text-[13px] tabular-nums" style={{ tableLayout: "fixed", width: GUTTER + totalW }}>
          <colgroup><col style={{ width: GUTTER }} />{cols.map((c) => <col key={c.key} style={{ width: width(c) }} />)}</colgroup>
          <thead>
            <tr style={{ height: LETTER_H }}>
              <th className="sticky top-0 left-0 z-30 border-r border-b border-zinc-300 bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800" style={{ height: LETTER_H }}
                onMouseDown={() => { if (n) { setSel({ a: { r: 0, c: 0 }, f: { r: n - 1, c: m - 1 } }); focusGrid(); } }} title="select all (⌘A)" />
              {cols.map((c, i) => {
                const hit = i >= range.c0 && i <= range.c1;
                return (
                  <th key={c.key} onMouseDown={(e) => colDown(e, i)} onMouseEnter={() => colEnter(i)}
                    className={cx("sticky top-0 z-20 border-r border-b border-zinc-300 text-[11px] font-normal dark:border-zinc-700",
                      hit ? "bg-brand-100 text-brand-700 dark:bg-brand-500/25 dark:text-brand-100" : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400")}>
                    {letter(i)}
                    <span onMouseDown={(e) => startResize(e, c)} onDoubleClick={() => autofit(c)} className="absolute top-0 -right-1 z-10 h-full w-2 cursor-col-resize" />
                  </th>
                );
              })}
            </tr>
            <tr style={{ height: NAME_H }}>
              <th className="sticky left-0 z-30 border-r border-b-2 border-zinc-300 bg-zinc-50 dark:border-zinc-700 dark:bg-zinc-900" style={{ top: LETTER_H }} />
              {cols.map((c, i) => {
                const sorted = p.sort?.col === c.key ? p.sort.dir : null;
                return (
                  <th key={c.key} style={{ top: LETTER_H }} onMouseDown={(e) => colDown(e, i)} onMouseEnter={() => colEnter(i)}
                    onContextMenu={(e) => { e.preventDefault(); columnMenu(e.clientX, e.clientY, i); }}
                    title={[c.type, c.pk && "primary key", c.fk && `→ ${c.fk}`, c.editable ? "editable" : "read-only", c.approval && "changes need approval"].filter(Boolean).join(" · ")}
                    className="group sticky z-20 border-r border-b-2 border-zinc-300 bg-zinc-50 px-2 text-left font-semibold text-zinc-800 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100">
                    <span className="flex items-center gap-1.5">
                      <span className="shrink-0 text-zinc-400 [&>svg]:size-3.5">{c.pk ? <KeyRound /> : KIND_ICON[c.kind]}</span>
                      <span className="truncate">{c.key}</span>
                      {!c.editable && !c.pk && p.onEdit && <Lock className="size-3 shrink-0 text-zinc-300 dark:text-zinc-600" aria-label="read-only" />}
                      {c.approval && c.editable && <span title="changes need approval" className="size-1.5 shrink-0 rounded-full bg-amber-500" />}
                      {sorted && (sorted === "asc" ? <ArrowUpNarrowWide className="size-3.5 shrink-0 text-brand-600" /> : <ArrowDownWideNarrow className="size-3.5 shrink-0 text-brand-600" />)}
                      <button aria-label={`${c.key} menu`} onMouseDown={(e) => e.stopPropagation()}
                        onClick={(e) => { const r = e.currentTarget.getBoundingClientRect(); columnMenu(r.left, r.bottom + 2, i); }}
                        className="absolute top-1/2 right-1 flex size-5 -translate-y-1/2 cursor-pointer items-center justify-center rounded bg-zinc-50 text-zinc-500 opacity-0 ring-1 ring-zinc-200 group-hover:opacity-100 hover:bg-zinc-200 hover:text-zinc-800 focus:opacity-100 dark:bg-zinc-900 dark:ring-zinc-700 dark:hover:bg-zinc-700">
                        <ChevronDown className="size-3.5" />
                      </button>
                    </span>
                    <span onMouseDown={(e) => startResize(e, c)} onDoubleClick={() => autofit(c)} className="absolute top-0 -right-1 z-10 h-full w-2 cursor-col-resize" />
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {first > 0 && <tr style={{ height: first * ROW_H }} />}
            {Array.from({ length: Math.max(0, last - first) }, (_, k) => {
              const r = first + k, dr = isDraft(r), row = dr ? null : rows[r];
              const rowHit = r >= range.r0 && r <= range.r1;
              return (
                <tr key={dr ? "draft" : r} style={{ height: ROW_H }}>
                  <th onMouseDown={(e) => rowDown(e, r)} onMouseEnter={() => cellEnter({ r, c: 0 })}
                    onContextMenu={(e) => cellMenu(e, { r, c: 0 }, true)}
                    className={cx("sticky left-0 z-10 border-r border-b border-zinc-300 px-1 text-center text-[11px] font-normal dark:border-zinc-700",
                      dr ? "bg-emerald-50 dark:bg-emerald-500/10" : rowHit ? "bg-brand-100 text-brand-700 dark:bg-brand-500/25 dark:text-brand-100" : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400")}>
                    {dr ? (
                      <span className="flex items-center justify-center gap-0.5">
                        <button title="save the new row (⌘Enter)" onMouseDown={(e) => e.stopPropagation()} onClick={p.onDraftSave} className="cursor-pointer rounded p-0.5 text-emerald-700 hover:bg-emerald-100 dark:text-emerald-400 dark:hover:bg-emerald-500/20"><Check className="size-3.5" /></button>
                        <button title="discard the new row (Esc)" onMouseDown={(e) => e.stopPropagation()} onClick={p.onDraftCancel} className="cursor-pointer rounded p-0.5 text-zinc-500 hover:bg-zinc-200 dark:hover:bg-zinc-700"><X className="size-3.5" /></button>
                      </span>
                    ) : r + 1}
                  </th>
                  {cols.map((col, c) => {
                    const v = dr ? draft![col.key] : row![col.key];
                    const on = inRange(r, c), act = active.r === r && active.c === c;
                    const mark = !dr && row && p.markKey ? p.marks?.[p.markKey(row, col.key)] : undefined;
                    const ed = editing && editing.r === r && editing.c === c ? editing : null;
                    const empty = v === null || v === undefined || v === "";
                    const writable = dr ? col.creatable : col.editable;
                    return (
                      <td key={col.key}
                        onMouseDown={(e) => cellDown(e, { r, c })} onMouseEnter={() => cellEnter({ r, c })}
                        onDoubleClick={() => startEdit({ r, c })} onContextMenu={(e) => cellMenu(e, { r, c })}
                        title={mark?.note ?? (col.kind === "json" || (typeof v === "string" && v.length > 30) ? editText(v) : undefined)}
                        className={cx("relative overflow-hidden border-r border-b border-zinc-200 px-2 whitespace-nowrap dark:border-zinc-800",
                          col.kind === "num" && "text-right", col.kind === "bool" && "text-center", col.kind === "json" && "font-mono text-xs text-zinc-600 dark:text-zinc-400",
                          dr && !writable && "bg-zinc-50 dark:bg-zinc-900",
                          dr && writable && "bg-emerald-50/40 dark:bg-emerald-500/5",
                          on && !act && "bg-brand-50! dark:bg-brand-500/15!",
                          mark?.tone === "saving" && "text-zinc-400",
                          act && "outline-2 -outline-offset-2 outline-brand-600",
                        )}>
                        {ed && !ed.bar ? (
                          <input autoFocus spellCheck={false} value={ed.text}
                            onChange={(e) => setEditing({ ...ed, text: e.target.value })}
                            onKeyDown={editorKeys} onBlur={() => commit()} onMouseDown={(e) => e.stopPropagation()}
                            placeholder={col.nullable ? "empty = null" : ""} list={col.kind === "text" ? "sheet-suggest" : undefined}
                            className={cx("absolute inset-0 z-10 w-full bg-white px-2 text-[13px] outline-2 -outline-offset-2 outline-brand-600 placeholder:text-zinc-400 dark:bg-zinc-950", col.kind === "num" && "text-right", col.kind === "json" && "font-mono text-xs")} />
                        ) : col.kind === "bool" && !empty ? (
                          <input type="checkbox" tabIndex={-1} checked={v === true || v === "true"} readOnly disabled={!writable || !p.onEdit}
                            onMouseDown={(e) => e.stopPropagation()} onClick={(e) => { e.preventDefault(); setSel({ a: { r, c }, f: { r, c } }); toggle({ r, c }); focusGrid(); }}
                            className="size-3.5 cursor-pointer align-middle accent-brand-600 disabled:cursor-default" />
                        ) : dr && empty && col.default !== undefined && col.default !== null ? (
                          <span className="text-zinc-400 italic">{editText(col.default).slice(0, 40)}</span>
                        ) : (
                          <span className={cx("block truncate", ed?.bar && "text-brand-700 dark:text-brand-100")}>{ed?.bar ? ed.text : shown(v, col.kind)}</span>
                        )}
                        {mark && mark.tone !== "saving" && (
                          <span aria-label={mark.note} className={cx("absolute top-0 right-0 size-0 border-t-[7px] border-l-[7px] border-l-transparent", mark.tone === "error" ? "border-t-rose-500" : "border-t-amber-500")} />
                        )}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            {last < n && <tr style={{ height: (n - last) * ROW_H }} />}
            {!p.hasMore && p.onInsertRow && !draft && (
              <tr style={{ height: ROW_H }}>
                <th className="sticky left-0 z-10 border-r border-b border-zinc-300 bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800" />
                <td colSpan={m} className="border-b border-zinc-200 p-0 dark:border-zinc-800">
                  <button onClick={p.onInsertRow} className="sticky left-14 flex h-7 cursor-pointer items-center gap-1.5 px-2 text-zinc-500 hover:text-brand-600">
                    <Plus className="size-3.5" />Add a row
                  </button>
                </td>
              </tr>
            )}
          </tbody>
        </table>
        {!n && !p.loading && <div className="sticky left-0 px-4 py-8 text-center text-zinc-500">{p.empty ?? "no row"}</div>}
      </div>

      {/* status bar */}
      <div className="flex h-7 shrink-0 items-center gap-4 border-t border-zinc-200 bg-zinc-50 px-3 text-xs text-zinc-500 tabular-nums dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-400">
        <span>{p.loading && !rows.length ? "loading…" : `${rows.length.toLocaleString()}${p.total > rows.length ? ` of ${p.total.toLocaleString()}` : ""} row${p.total === 1 ? "" : "s"}`}{p.loading && rows.length ? " · loading…" : ""}</span>
        {stats && (
          <span className="ml-auto flex gap-4">
            {stats.nums > 0 && <>
              <span>Sum <b className="font-medium text-zinc-800 dark:text-zinc-200">{num(stats.sum)}</b></span>
              <span>Average <b className="font-medium text-zinc-800 dark:text-zinc-200">{num(stats.sum / stats.nums)}</b></span>
              <span className="hidden md:inline">Min <b className="font-medium text-zinc-800 dark:text-zinc-200">{num(stats.min)}</b></span>
              <span className="hidden md:inline">Max <b className="font-medium text-zinc-800 dark:text-zinc-200">{num(stats.max)}</b></span>
            </>}
            <span>Count <b className="font-medium text-zinc-800 dark:text-zinc-200">{stats.count}</b></span>
          </span>
        )}
      </div>
      {/* as spreadsheets do, typing into a text cell suggests the values already in its column */}
      {editing && cols[editing.c]?.kind === "text" && (
        <datalist id="sheet-suggest">
          {[...new Set(rows.map((r) => r[cols[editing.c].key]).filter((v): v is string => typeof v === "string" && v.length > 0 && v.length < 60))].slice(0, 50).map((v) => <option key={v} value={v} />)}
        </datalist>
      )}
      {menu && <Menu {...menu} onClose={() => setMenu(null)} />}
    </div>
  );
}

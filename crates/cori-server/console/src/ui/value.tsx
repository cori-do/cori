import { createContext, useContext, useState, type ReactNode } from "react";
import { Braces, Download, File as FileIcon, List as ListIcon } from "lucide-react";
import { download, errMsg } from "../lib/api";
import { words } from "../lib/format";
import { cx, Dash } from "./core";
import { toast } from "./toast";

// Data shown to a person: an approval's payload, a run's parameters and result. Objects read as labelled fields, lists
// as lists or small tables, long text as text; the raw JSON is one click away.

const isPlain = (v: unknown) => v === null || ["string", "number", "boolean"].includes(typeof v);
const URL_RE = /^https?:\/\/\S+$/;

/** A file a run was given (a `file` parameter): its reference, shown as a file to download. */
type FileRef = { id: string; name: string; size?: number; content_type?: string };
const isFile = (v: unknown): v is FileRef => !!v && typeof v === "object" && !Array.isArray(v) && typeof (v as FileRef).id === "string" && (v as FileRef).id.startsWith("file_") && typeof (v as FileRef).name === "string" && "content_type" in (v as object);
/** The app whose files the values name (their downloads), set by `Payload`'s `app`. */
const FilesOf = createContext<string | null>(null);
const bytes = (n?: number) => n === undefined ? "" : n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;

function FileChip({ f }: { f: FileRef }) {
  const app = useContext(FilesOf);
  const [busy, setBusy] = useState(false);
  async function get() {
    if (!app) return;
    setBusy(true);
    try { await download(`/v1/apps/${app}/files/${encodeURIComponent(f.id)}/content`, f.name); } catch (e) { toast.error(errMsg(e)); }
    setBusy(false);
  }
  return (
    <span className="inline-flex max-w-full flex-wrap items-center gap-x-2.5 gap-y-1 rounded-md bg-surface-2 px-2 py-1 text-[13px] ring-1 ring-line">
      <span className="inline-flex min-w-0 items-center gap-1.5">
        <FileIcon className="size-3.5 shrink-0 text-fg-3" aria-hidden />
        <span className="min-w-0 break-words text-fg">{f.name}</span>
      </span>
      <span className="inline-flex items-center gap-2.5">
        {f.size !== undefined && <span className="text-fg-4 tabular-nums">{bytes(f.size)}</span>}
        {app && <button type="button" disabled={busy} onClick={get} className="inline-flex cursor-pointer items-center gap-1 text-fg-3 hover:text-fg disabled:opacity-50" title={`Download ${f.name}`}>
          <Download className="size-3.5" aria-hidden />Download
        </button>}
      </span>
    </span>
  );
}

function Scalar({ v }: { v: unknown }) {
  if (v === null || v === undefined || v === "") return <Dash />;
  if (typeof v === "boolean") return <span>{v ? "Yes" : "No"}</span>;
  if (typeof v === "number") return <span className="tabular-nums">{v.toLocaleString(undefined, { maximumFractionDigits: 6 })}</span>;
  const s = String(v);
  if (URL_RE.test(s)) return <a href={s} target="_blank" rel="noopener noreferrer" className="link break-all">{s}</a>;
  return <span className={cx("break-words", s.length > 80 && "whitespace-pre-wrap")}>{s}</span>;
}

/** Whether a value is a file a run was given, or a list of them. */
export const isFileValue = (v: unknown) => isFile(v) || (Array.isArray(v) && v.length > 0 && v.every(isFile));

/** A file (or a list of files) of `app`, offered for download. */
export function FileValue({ value, app }: { value: unknown; app: string }) {
  return <FilesOf.Provider value={app}><ValueView value={value} /></FilesOf.Provider>;
}

export function ValueView({ value, depth = 0 }: { value: unknown; depth?: number }): ReactNode {
  if (isPlain(value) || value === undefined) return <Scalar v={value} />;
  if (isFile(value)) return <FileChip f={value} />;
  if (Array.isArray(value)) {
    if (!value.length) return <span className="text-fg-4">none</span>;
    if (value.every(isFile)) return <span className="flex flex-wrap gap-2">{value.map((f) => <FileChip key={f.id} f={f} />)}</span>;
    if (value.every(isPlain)) return <span className="break-words">{value.map((x, i) => <span key={i}>{i > 0 && ", "}<Scalar v={x} /></span>)}</span>;
    const objs = value.every((x) => x && typeof x === "object" && !Array.isArray(x)) ? (value as Record<string, unknown>[]) : null;
    const keys = objs ? [...new Set(objs.flatMap((o) => Object.keys(o)))] : [];
    if (objs && keys.length <= 6 && objs.every((o) => Object.values(o).every(isPlain)) && depth < 2) {
      return (
        <div className="overflow-x-auto rounded-lg ring-1 ring-line">
          <table className="w-full text-left text-[13px]">
            <thead className="bg-surface-2 text-fg-3"><tr>{keys.map((k) => <th key={k} className="px-2.5 py-1.5 font-medium whitespace-nowrap">{words(k)}</th>)}</tr></thead>
            <tbody className="divide-y divide-line">{objs.map((o, i) => <tr key={i}>{keys.map((k) => <td key={k} className="px-2.5 py-1.5 align-top"><Scalar v={o[k]} /></td>)}</tr>)}</tbody>
          </table>
        </div>
      );
    }
    if (depth >= 2) return <Json value={value} />;
    return <ol className="space-y-2">{value.map((x, i) => <li key={i} className="rounded-lg bg-surface-2/60 p-2.5 ring-1 ring-line"><ValueView value={x} depth={depth + 1} /></li>)}</ol>;
  }
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (!entries.length) return <span className="text-fg-4">nothing</span>;
    if (depth >= 2) return <Json value={value} />;
    return (
      <dl className={cx("grid gap-x-5 gap-y-2", depth ? "grid-cols-[minmax(5rem,auto)_1fr]" : "grid-cols-[minmax(5.5rem,auto)_1fr] sm:grid-cols-[minmax(8rem,auto)_1fr]")}>
        {entries.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="pt-px text-[13px] text-fg-3">{words(k)}</dt>
            <dd className="min-w-0 text-fg">{isPlain(v) || v === undefined ? <Scalar v={v} /> : <ValueView value={v} depth={depth + 1} />}</dd>
          </div>
        ))}
      </dl>
    );
  }
  return <Scalar v={String(value)} />;
}

export function Json({ value, className }: { value: unknown; className?: string }) {
  return (
    <pre className={cx("overflow-auto rounded-lg bg-surface-2 p-3 font-mono text-[12px] leading-relaxed text-fg-2 ring-1 ring-line", className)}>
      {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
    </pre>
  );
}

/** Fields, with a switch to the raw JSON. `app`: the app whose files the values may name, offered for download. */
export function Payload({ value, empty = "Nothing to show", className, app }: { value: unknown; empty?: ReactNode; className?: string; app?: string }) {
  const [raw, setRaw] = useState(false);
  const nothing = value === null || value === undefined || (typeof value === "object" && !Array.isArray(value) && !Object.keys(value as object).length);
  if (nothing) return <p className="text-fg-3">{empty}</p>;
  return (
    <div className={className}>
      {raw ? <Json value={value} className="max-h-96" /> : <FilesOf.Provider value={app ?? null}><ValueView value={value} /></FilesOf.Provider>}
      <button type="button" onClick={() => setRaw(!raw)} className="mt-3 inline-flex cursor-pointer items-center gap-1.5 text-[13px] text-fg-3 hover:text-fg">
        {raw ? <ListIcon className="size-3.5" aria-hidden /> : <Braces className="size-3.5" aria-hidden />}
        {raw ? "Show as fields" : "Show raw JSON"}
      </button>
    </div>
  );
}

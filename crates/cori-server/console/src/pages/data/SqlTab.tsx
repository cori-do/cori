import { useState } from "react";
import { Play } from "lucide-react";
import { errMsg } from "../../lib/api";
import { qi } from "../../lib/format";
import { readCols, type Row, type StoreClient } from "../../lib/store";
import type { Table } from "../../lib/types";
import { Button, ErrorNote, Muted, Textarea } from "../../ui/core";
import { Sheet, type Kind, type SheetCol } from "./Sheet";

const CAP = 1000;

/** The kind of a result column, from its values (a query result carries no types). */
const kindOfValues = (rows: Row[], key: string): Kind => {
  const v = rows.map((r) => r[key]).find((x) => x !== null && x !== undefined);
  return typeof v === "number" ? "num" : typeof v === "boolean" ? "bool" : typeof v === "object" ? "json"
    : typeof v === "string" && /^\d{4}-\d{2}-\d{2}([T ]\d{2}:\d{2})?/.test(v) ? "date" : "text";
};

export function SqlTab({ t, role, client }: { t: Table; role: string; client: StoreClient }) {
  const cols = readCols(t);
  const [sql, setSql] = useState(`SELECT ${cols.map((c) => qi(c.name)).join(", ") || "…"}\nFROM ${qi(t.name)}\nLIMIT 50`);
  const [out, setOut] = useState<{ rows: Row[]; ms: number } | { error: string } | null>(null);
  const [running, setRunning] = useState(false);

  async function run() {
    setRunning(true);
    const t0 = performance.now();
    try { setOut({ rows: await client.query(sql, [], CAP), ms: Math.round(performance.now() - t0) }); }
    catch (e) { setOut({ error: errMsg(e) }); }
    setRunning(false);
  }

  const rows = out && "rows" in out ? out.rows : [];
  const sheetCols: SheetCol[] = [...new Set(rows.flatMap((r) => Object.keys(r)))].map((key) => {
    const kind = kindOfValues(rows, key);
    return { key, kind, type: kind === "num" ? "number" : kind === "bool" ? "boolean" : kind, width: kind === "num" ? 110 : kind === "json" ? 240 : 180 };
  });
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-zinc-200 p-3 dark:border-zinc-800">
        <Textarea rows={5} spellCheck={false} className="font-mono text-xs" value={sql} onChange={(e) => setSql(e.target.value)}
          onKeyDown={(e) => { if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); run(); } }} />
        <div className="mt-2 flex flex-wrap items-center gap-3">
          <Button variant="primary" onClick={run} loading={running}>{!running && <Play className="size-3.5" />}Run</Button>
          <Muted className="text-xs">⌘/Ctrl+Enter · one SELECT (CTEs and window functions welcome), run by Postgres as role {role}: only its columns{t.scope === "own" ? ", only your rows" : ""}; writes go through the Sheet view or a workflow</Muted>
          {out && "rows" in out && <Muted className="ml-auto text-xs tabular-nums">{out.rows.length} row{out.rows.length === 1 ? "" : "s"}{out.rows.length === CAP ? ` (capped at ${CAP})` : ""} · {out.ms} ms</Muted>}
        </div>
      </div>
      {out && "error" in out ? <div className="p-4"><ErrorNote>{out.error}</ErrorNote></div>
        : out ? <Sheet key={sheetCols.map((c) => c.key).join("|") + out.ms} cols={sheetCols} rows={rows} total={rows.length} hasMore={false} loading={false} empty="the query returned no row" />
        : <div className="p-4 text-zinc-500">Run a query to see its result as a sheet.</div>}
    </div>
  );
}

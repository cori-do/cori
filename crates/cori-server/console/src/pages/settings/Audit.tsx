import { useMemo, useState } from "react";
import { ChevronRight, Search } from "lucide-react";
import { api } from "../../lib/api";
import { ago, when } from "../../lib/format";
import { useQuery } from "../../lib/query";
import type { AuditRow } from "../../lib/types";
import { Card, cx, Empty, ErrorNote, Input, Select, SkeletonRows, Time } from "../../ui/core";
import { PersonName } from "../../ui/people";
import { Json } from "../../ui/value";

// Who did what, and when: every change the platform recorded, newest first.

export function Audit() {
  const q = useQuery<AuditRow[]>("/v1/audit", () => api<AuditRow[]>("/v1/audit"));
  const [text, setText] = useState("");
  const [area, setArea] = useState("");
  const rows = q.data ?? [];
  const areas = useMemo(() => [...new Set(rows.map((r) => r.action.split(".")[0]))].sort(), [rows]);
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={6} />;
  const shown = rows.filter((r) => (!area || r.action.startsWith(`${area}.`) || r.action === area)
    && (!text || `${r.actor} ${r.action} ${r.object} ${JSON.stringify(r.details)}`.toLowerCase().includes(text.toLowerCase())));
  return (
    <>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="relative w-full sm:w-72">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
          <Input h="sm" className="pl-9" placeholder="Search who, what, which object" aria-label="Search the audit log" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
        <Select h="sm" className="w-auto" value={area} onChange={(e) => setArea(e.target.value)} aria-label="Kind of action">
          <option value="">Every action</option>
          {areas.map((a) => <option key={a} value={a}>{a}</option>)}
        </Select>
        <span className="ml-auto text-[13px] text-fg-3">{shown.length} of {rows.length}</span>
      </div>
      {shown.length ? <Card className="divide-y divide-line overflow-hidden">{shown.slice(0, 300).map((r, i) => <Line key={i} r={r} />)}</Card>
        : <Empty compact title="Nothing matches" />}
    </>
  );
}

function Line({ r }: { r: AuditRow }) {
  const [open, setOpen] = useState(false);
  const has = r.details !== null && r.details !== undefined && !(typeof r.details === "object" && !Object.keys(r.details as object).length);
  return (
    <div>
      <button type="button" disabled={!has} onClick={() => setOpen(!open)} aria-expanded={open}
        className={cx("flex w-full items-center gap-3 px-4 py-2.5 text-left text-[14px]", has ? "cursor-pointer hover:bg-surface-2/60" : "cursor-default")}>
        <ChevronRight className={cx("size-4 shrink-0 text-fg-4 transition-transform", open && "rotate-90", !has && "invisible")} aria-hidden />
        <span className="w-28 shrink-0 text-[13px] text-fg-3"><Time at={r.at}>{ago(r.at)}</Time></span>
        <span className="w-36 shrink-0 truncate"><PersonName id={r.actor} /></span>
        <code className="shrink-0 text-[12.5px] text-fg">{r.action}</code>
        <span className="min-w-0 flex-1 truncate text-fg-3">{r.object}</span>
      </button>
      {open && <div className="px-4 pb-3 pl-11"><div className="mb-1 text-xs text-fg-4">{when(r.at)}</div><Json value={r.details} className="max-h-72" /></div>}
    </div>
  );
}

import { useState } from "react";
import { errMsg, post, uploadFile } from "../../lib/api";
import { words } from "../../lib/format";
import { invalidate } from "../../lib/query";
import { href, navigate } from "../../lib/router";
import type { Param, PromotedWorkflow } from "../../lib/types";
import { Button, Field, Input, Textarea } from "../../ui/core";
import { Modal, openDialog } from "../../ui/dialog";
import { toast } from "../../ui/toast";
import { refusedFor } from "../../ui/connections";

// Starting a workflow by hand: a form from its parameters, then its run page.

export async function runWorkflow(app: string, w: PromotedWorkflow) {
  const body = await openDialog<Record<string, unknown>>((close) => <RunForm w={w} close={close} />);
  if (!body) return;
  try {
    // the files the form was given: uploaded for their parameter first, the run gets their ids
    for (const [k, v] of Object.entries(body)) {
      if (v instanceof File) body[k] = (await uploadFile(app, w.id, k, v)).id;
      else if (Array.isArray(v) && v.some((x) => x instanceof File)) body[k] = await Promise.all(v.map(async (x) => (x instanceof File ? (await uploadFile(app, w.id, k, x)).id : x)));
    }
    const r = await post<{ run_id: string }>(`/v1/apps/${app}/wf/${w.id}/runs`, body);
    invalidate(`/v1/apps/${app}/runs`, "/v1/me/apps");
    navigate(href("app", app, "runs", r.run_id));
  } catch (e) { refusedFor(e) || toast.error(errMsg(e)); }
}

type P = Param;
const kind = (p: P) => {
  const t = (p.type || "").toLowerCase();
  if (t === "file" || t === "files") return t;
  return /bool/.test(t) ? "bool" : /int|number|float|decimal/.test(t) ? "num" : /object|array|json|map|list/.test(t) ? "json" : /date/.test(t) ? "date" : "text";
};
const initial = (p: P) => (p.default === undefined || p.default === null ? "" : typeof p.default === "string" ? p.default : JSON.stringify(p.default));

function RunForm({ w, close }: { w: PromotedWorkflow; close: (v: Record<string, unknown> | null) => void }) {
  const params = (w.parameters || []) as P[];
  const [values, setValues] = useState<Record<string, string>>(() => Object.fromEntries(params.map((p) => [p.name, initial(p)])));
  const [files, setFiles] = useState<Record<string, File[]>>({});
  const [error, setError] = useState<Record<string, string>>({});
  const set = (k: string, v: string) => setValues((x) => ({ ...x, [k]: v }));
  function submit() {
    const out: Record<string, unknown> = {}, bad: Record<string, string> = {};
    for (const p of params) {
      const v = values[p.name] ?? "";
      const k = kind(p);
      if (k === "file" || k === "files") {
        const fs = files[p.name] ?? [];
        if (!fs.length) { if (p.required !== false) bad[p.name] = "Required"; continue; }
        if (k === "files" && p.max_files && fs.length > p.max_files) { bad[p.name] = `At most ${p.max_files} files`; continue; }
        out[p.name] = k === "file" ? fs[0] : fs;
        continue;
      }
      if (v === "" && k !== "bool") { if (p.required) bad[p.name] = "Required"; continue; }
      if (k === "bool") out[p.name] = v === "true";
      else if (k === "num") { const n = Number(v); if (Number.isNaN(n)) bad[p.name] = "A number"; else out[p.name] = n; }
      else if (k === "json") { try { out[p.name] = JSON.parse(v); } catch { bad[p.name] = "Not valid JSON"; } }
      else out[p.name] = v;
    }
    setError(bad);
    if (!Object.keys(bad).length) close(out);
  }
  return (
    <Modal title={`Run ${words(w.id)}`} description={params.length ? "It starts as you, with these values." : "It takes no parameter: it starts as you."} onClose={() => close(null)} onSubmit={submit}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary">Run</Button></>}>
      {params.map((p, i) => {
        const k = kind(p);
        const label = <span className="inline-flex items-baseline gap-2">{words(p.name)}{p.type && <span className="font-mono text-xs font-normal text-fg-4">{p.type}</span>}</span>;
        if (k === "bool") return (
          <label key={p.name} className="flex cursor-pointer items-start gap-3">
            <input type="checkbox" className="mt-1 size-4" checked={values[p.name] === "true"} onChange={(e) => set(p.name, String(e.target.checked))} />
            <span><span className="block text-[13px] font-medium text-fg">{label}</span>{p.description && <span className="block text-[13px] text-fg-3">{p.description}</span>}</span>
          </label>
        );
        if (k === "file" || k === "files") return (
          <Field key={p.name} label={label} hint={[p.description, p.accept?.join(", "), p.max_size !== undefined && `at most ${p.max_size}`].filter(Boolean).join(" · ") || undefined} error={error[p.name]} optional={p.required === false}>
            <input type="file" multiple={k === "files"} accept={p.accept?.join(",")} autoFocus={i === 0} onChange={(e) => { const list = Array.from(e.target.files ?? []); setFiles((x) => ({ ...x, [p.name]: list })); }}
              className="block w-full cursor-pointer text-[13px] text-fg-2 file:mr-3 file:cursor-pointer file:rounded-md file:border-0 file:bg-surface-2 file:px-3 file:py-1.5 file:text-[13px] file:font-medium file:text-fg hover:file:bg-surface-3" />
          </Field>
        );
        return (
          <Field key={p.name} label={label} hint={p.description} error={error[p.name]} optional={!p.required && p.default === undefined}>
            {k === "json"
              ? <Textarea rows={3} className="font-mono text-[13px]" autoFocus={i === 0} value={values[p.name]} onChange={(e) => set(p.name, e.target.value)} placeholder="{ }" />
              : <Input autoFocus={i === 0} type={k === "date" ? "date" : "text"} inputMode={k === "num" ? "decimal" : undefined} value={values[p.name]} onChange={(e) => set(p.name, e.target.value)} />}
          </Field>
        );
      })}
    </Modal>
  );
}

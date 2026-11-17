import { useEffect, useState, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { clsx } from "clsx";

// Short notices, bottom right (bottom centre on a phone), callable from anywhere: `toast.success("Saved")`,
// `toast.error(message)`, `toast("…", { action: { label: "View", href } })`. Errors stay longer and are announced assertively.

type Kind = "info" | "success" | "error";
type Item = { id: number; kind: Kind; text: ReactNode; action?: { label: string; href?: string; onClick?: () => void } };
type Opts = { action?: Item["action"] };

let push: ((t: Item) => void) | null = null;
let n = 0;
const show = (kind: Kind) => (text: ReactNode, opts: Opts = {}) => push?.({ id: ++n, kind, text, ...opts });

export const toast = Object.assign(show("info"), { success: show("success"), error: show("error"), info: show("info") });

export function Toaster() {
  const [items, setItems] = useState<Item[]>([]);
  useEffect(() => {
    push = (t) => setItems((xs) => [...xs.slice(-3), t]);
    return () => { push = null; };
  }, []);
  const close = (id: number) => setItems((xs) => xs.filter((x) => x.id !== id));
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex flex-col items-center gap-2 p-4 sm:items-end" aria-live="polite">
      {items.map((t) => <Toast key={t.id} t={t} close={() => close(t.id)} />)}
    </div>
  );
}

function Toast({ t, close }: { t: Item; close: () => void }) {
  useEffect(() => {
    const h = setTimeout(close, t.kind === "error" ? 9000 : t.action ? 7000 : 4000);
    return () => clearTimeout(h);
  }, [t, close]);
  const Icon = t.kind === "error" ? AlertTriangle : t.kind === "success" ? CheckCircle2 : Info;
  return (
    <div role={t.kind === "error" ? "alert" : "status"}
      className="pointer-events-auto flex w-full max-w-sm animate-rise items-start gap-3 rounded-xl bg-zinc-900 py-3 pr-2 pl-3.5 text-[13.5px] text-white shadow-lg ring-1 ring-white/10 dark:bg-zinc-800">
      <Icon className={clsx("mt-0.5 size-4 shrink-0", t.kind === "error" ? "text-rose-400" : t.kind === "success" ? "text-emerald-400" : "text-zinc-300")} aria-hidden />
      <div className="min-w-0 flex-1 leading-snug break-words">{t.text}</div>
      {t.action && (
        t.action.href
          ? <a href={t.action.href} onClick={close} className="shrink-0 rounded-md px-2 py-0.5 font-medium text-brand-100 hover:bg-white/10">{t.action.label}</a>
          : <button type="button" onClick={() => { t.action?.onClick?.(); close(); }} className="shrink-0 cursor-pointer rounded-md px-2 py-0.5 font-medium text-brand-100 hover:bg-white/10">{t.action.label}</button>
      )}
      <button type="button" onClick={close} aria-label="Dismiss" className="shrink-0 cursor-pointer rounded-md p-0.5 text-zinc-400 hover:bg-white/10 hover:text-white"><X className="size-4" /></button>
    </div>
  );
}

import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { X } from "lucide-react";
import { Button, cx, Field, IconButton, Input, Textarea } from "./core";

// Modal dialogs and side drawers on the native <dialog> (focus kept inside, Escape closes, the page behind inert).
// Dialogs can be opened imperatively and awaited: `if (!(await confirm({…}))) return;` — the host renders them.

type Close<T> = (v: T | null) => void;
type Req = { id: number; node: ReactNode };
let push: ((r: Req) => void) | null = null;
let pop: ((id: number) => void) | null = null;
let n = 0;

export function openDialog<T>(render: (close: Close<T>) => ReactNode): Promise<T | null> {
  return new Promise((resolve) => {
    if (!push) return resolve(null);
    const id = ++n;
    push({ id, node: render((v) => { pop?.(id); resolve(v); }) });
  });
}

export function DialogHost() {
  const [reqs, setReqs] = useState<Req[]>([]);
  useEffect(() => {
    push = (r) => setReqs((rs) => [...rs, r]);
    pop = (id) => setReqs((rs) => rs.filter((x) => x.id !== id));
    return () => { push = null; pop = null; };
  }, []);
  return <>{reqs.map((r) => <div key={r.id}>{r.node}</div>)}</>;
}

/** Opens `ref`'s dialog as a modal on mount; Escape and a click on the backdrop call `onClose`. */
function useModal(onClose: () => void) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    d.showModal();
    // what the keyboard starts on: a marked control, else the first field, else the body (never the close button)
    const first = d.querySelector<HTMLElement>("[data-autofocus]")
      ?? d.querySelector<HTMLElement>("input:not([disabled]):not([type=hidden]), textarea:not([disabled]), select:not([disabled])")
      ?? d.querySelector<HTMLElement>("[data-dialog-body]");
    first?.focus({ preventScroll: true });
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => { document.body.style.overflow = prev; };
  }, []);
  return {
    ref,
    onCancel: (e: React.SyntheticEvent) => { e.preventDefault(); onClose(); },
    // a press that starts and ends on the backdrop (the dialog element itself, around its panel)
    onMouseDown: (e: React.MouseEvent) => { if (e.target === ref.current) (ref.current as HTMLDialogElement & { _down?: boolean })._down = true; },
    onClick: (e: React.MouseEvent) => {
      const d = ref.current as (HTMLDialogElement & { _down?: boolean }) | null;
      if (d && e.target === d && d._down) onClose();
      if (d) d._down = false;
    },
  };
}

export function Modal({ title, description, children, onClose, onSubmit, footer, size = "md" }: {
  title: ReactNode; description?: ReactNode; children?: ReactNode; onClose: () => void; onSubmit?: (e: FormEvent<HTMLFormElement>) => void; footer?: ReactNode; size?: "sm" | "md" | "lg";
}) {
  const m = useModal(onClose);
  return (
    <dialog ref={m.ref} onCancel={m.onCancel} onMouseDown={m.onMouseDown} onClick={m.onClick} aria-labelledby="dialog-title"
      className={cx("m-auto max-h-[min(88dvh,820px)] w-[calc(100%-2rem)] animate-pop overflow-hidden rounded-2xl bg-surface p-0 text-fg shadow-2xl ring-1 ring-line backdrop:animate-fade",
        size === "sm" ? "max-w-sm" : size === "lg" ? "max-w-2xl" : "max-w-lg")}>
      <form onSubmit={(e) => { e.preventDefault(); onSubmit?.(e); }} className="flex max-h-[inherit] flex-col">
        <div className="flex items-start gap-3 px-5 pt-5 pb-1">
          <div className="min-w-0 flex-1">
            <h2 id="dialog-title" className="text-[17px] leading-snug font-semibold">{title}</h2>
            {description && <div className="mt-1 text-[14px] leading-relaxed text-fg-3">{description}</div>}
          </div>
          <IconButton label="Close" size="xs" onClick={onClose} className="-mt-0.5 -mr-1.5"><X /></IconButton>
        </div>
        {children && <div data-dialog-body tabIndex={-1} className="min-h-0 space-y-4 overflow-y-auto px-5 pt-3 pb-5 text-[14px] leading-relaxed text-fg-2 outline-none">{children}</div>}
        {footer && <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-line bg-surface-2/50 px-5 py-3">{footer}</div>}
      </form>
    </dialog>
  );
}

/** A side panel over the page's right edge (full width on a phone), for the details of one thing. */
export function Drawer({ title, subtitle, children, footer, onClose, wide }: { title: ReactNode; subtitle?: ReactNode; children: ReactNode; footer?: ReactNode; onClose: () => void; wide?: boolean }) {
  const m = useModal(onClose);
  return (
    <dialog ref={m.ref} onCancel={m.onCancel} onMouseDown={m.onMouseDown} onClick={m.onClick} aria-labelledby="drawer-title"
      className={cx("fixed inset-y-0 right-0 left-auto m-0 h-dvh max-h-dvh w-full animate-slide bg-surface p-0 text-fg shadow-2xl ring-1 ring-line backdrop:animate-fade", wide ? "max-w-2xl" : "max-w-xl")}>
      <div className="flex h-full flex-col">
        <div className="flex items-start gap-3 border-b border-line px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2 id="drawer-title" className="text-[17px] leading-snug font-semibold">{title}</h2>
            {subtitle && <div className="mt-0.5 text-[13px] text-fg-3">{subtitle}</div>}
          </div>
          <IconButton label="Close" size="xs" onClick={onClose} className="-mr-1.5"><X /></IconButton>
        </div>
        <div data-dialog-body tabIndex={-1} className="min-h-0 flex-1 overflow-y-auto px-5 py-5 outline-none">{children}</div>
        {footer && <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-line px-5 py-3">{footer}</div>}
      </div>
    </dialog>
  );
}

type ConfirmOpts = {
  title: ReactNode; body?: ReactNode; confirm?: string; danger?: boolean;
  /** the person types this (an app's id, an organization's slug) before the button works */
  typeToConfirm?: string;
};

export const confirm = (o: ConfirmOpts) => openDialog<true>((close) => <ConfirmBody {...o} close={close} />).then((v) => v === true);

function ConfirmBody({ title, body, confirm: label = "Confirm", danger, typeToConfirm, close }: ConfirmOpts & { close: Close<true> }) {
  const [typed, setTyped] = useState("");
  const ok = !typeToConfirm || typed.trim() === typeToConfirm;
  return (
    <Modal title={title} size="sm" onClose={() => close(null)} onSubmit={() => ok && close(true)}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant={danger ? "danger" : "primary"} disabled={!ok} data-autofocus={!typeToConfirm || undefined}>{label}</Button></>}>
      {body && <div className="space-y-2">{body}</div>}
      {typeToConfirm && (
        <Field label={<>Type <code className="rounded bg-surface-2 px-1 py-0.5 text-fg">{typeToConfirm}</code> to confirm</>}>
          <Input autoFocus value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} />
        </Field>
      )}
    </Modal>
  );
}

type PromptOpts = { title: ReactNode; body?: ReactNode; label: string; initial?: string; confirm?: string; multiline?: boolean; danger?: boolean; placeholder?: string; optional?: boolean };

/** Asks for a line (or a paragraph) of text; null when cancelled. */
export const prompt = (o: PromptOpts) => openDialog<string>((close) => <PromptBody {...o} close={close} />);

function PromptBody({ title, body, label, initial = "", confirm: action = "OK", multiline, danger, placeholder, optional, close }: PromptOpts & { close: Close<string> }) {
  const [v, setV] = useState(initial);
  const ok = optional || !!v.trim();
  return (
    <Modal title={title} onClose={() => close(null)} onSubmit={() => ok && close(v)}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant={danger ? "danger" : "primary"} disabled={!ok}>{action}</Button></>}>
      {body && <div>{body}</div>}
      <Field label={label} optional={optional}>
        {multiline
          ? <Textarea autoFocus rows={3} value={v} placeholder={placeholder} onChange={(e) => setV(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && ok) { e.preventDefault(); close(v); } }} />
          : <Input autoFocus value={v} placeholder={placeholder} onChange={(e) => setV(e.target.value)} />}
      </Field>
    </Modal>
  );
}

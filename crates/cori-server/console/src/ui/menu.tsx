import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type Ref } from "react";
import { createPortal } from "react-dom";
import { Check } from "lucide-react";
import { cx } from "./core";

// A menu of actions opened from a button: arrow keys move, Enter picks, Escape closes and gives focus back.
// It opens in a portal at fixed coordinates, so a card that clips its rows (overflow-hidden) cannot cut it off.

export type MenuItem =
  | { label: ReactNode; icon?: ReactNode; onSelect?: () => void; href?: string; external?: boolean; danger?: boolean; disabled?: boolean; hint?: ReactNode; checked?: boolean; description?: ReactNode }
  | { heading: ReactNode }
  | "sep"
  | null | false | undefined | "";

type TriggerProps = { ref: Ref<HTMLButtonElement>; onClick: () => void; "aria-haspopup": "menu"; "aria-expanded": boolean; "aria-controls": string };

export function Menu({ items, children, align = "end", side = "bottom", className, width = "w-60" }: {
  items: MenuItem[]; children: (p: TriggerProps) => ReactNode; align?: "start" | "end"; side?: "bottom" | "top"; className?: string; width?: string;
}) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const box = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const shown = items.filter(Boolean) as Exclude<MenuItem, null | false | undefined | "">[];
  const [pos, setPos] = useState<CSSProperties>({ visibility: "hidden" });

  // place it next to the trigger, on the other side when the preferred one has no room; follow scrolls and resizes
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const t = trigger.current?.getBoundingClientRect();
      const h = box.current?.offsetHeight ?? 0;
      if (!t) return;
      const below = window.innerHeight - t.bottom, above = t.top;
      const up = side === "top" ? above >= h + 6 || above > below : below < h + 6 && above > below;
      setPos({
        position: "fixed",
        ...(up ? { bottom: window.innerHeight - t.top + 6 } : { top: t.bottom + 6 }),
        ...(align === "end" ? { right: Math.max(8, window.innerWidth - t.right) } : { left: Math.max(8, t.left) }),
      });
    };
    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => { window.removeEventListener("scroll", place, true); window.removeEventListener("resize", place); setPos({ visibility: "hidden" }); };
  }, [open, side, align]);

  useEffect(() => {
    if (!open) return;
    const down = (e: MouseEvent) => { if (!box.current?.contains(e.target as Node) && !trigger.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", down);
    // the first item takes the focus, so the keyboard works at once
    requestAnimationFrame(() => box.current?.querySelector<HTMLElement>("[role=menuitem]:not([aria-disabled=true])")?.focus({ preventScroll: true }));
    return () => document.removeEventListener("mousedown", down);
  }, [open]);

  function keys(e: React.KeyboardEvent) {
    const all = [...(box.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not([aria-disabled=true])") ?? [])];
    const i = all.indexOf(document.activeElement as HTMLElement);
    const go = (el?: HTMLElement) => el?.focus({ preventScroll: true });
    if (e.key === "ArrowDown") { e.preventDefault(); go(all[(i + 1) % all.length]); }
    else if (e.key === "ArrowUp") { e.preventDefault(); go(all[(i - 1 + all.length) % all.length]); }
    else if (e.key === "Home") { e.preventDefault(); go(all[0]); }
    else if (e.key === "End") { e.preventDefault(); go(all[all.length - 1]); }
    else if (e.key === "Escape") { e.preventDefault(); setOpen(false); trigger.current?.focus(); }
    else if (e.key === "Tab") setOpen(false);
  }

  const itemClass = (danger?: boolean, disabled?: boolean) => cx(
    "flex w-full items-start gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[14px] outline-none",
    disabled ? "cursor-default opacity-45" : "cursor-pointer",
    danger ? "text-rose-600 focus:bg-rose-50 dark:text-rose-400 dark:focus:bg-rose-500/10" : "text-fg focus:bg-surface-2 hover:bg-surface-2",
  );

  return (
    <div className={cx("relative", className)}>
      {children({ ref: trigger, onClick: () => setOpen((o) => !o), "aria-haspopup": "menu", "aria-expanded": open, "aria-controls": id })}
      {open && createPortal(
        <div ref={box} id={id} role="menu" onKeyDown={keys} style={pos}
          className={cx("fixed z-50 max-h-[70vh] animate-pop overflow-y-auto rounded-xl bg-surface p-1 shadow-xl ring-1 ring-line", width)}>
          {shown.map((it, i) => {
            if (it === "sep") return <div key={i} role="separator" className="mx-1 my-1 h-px bg-line" />;
            if ("heading" in it) return <div key={i} className="px-2.5 pt-2 pb-1 text-xs font-medium text-fg-3">{it.heading}</div>;
            const body = (
              <>
                {it.checked !== undefined ? <Check className={cx("mt-0.5 size-4 shrink-0 text-brand-600", !it.checked && "invisible")} aria-hidden />
                  : it.icon ? <span className="mt-0.5 flex size-4 shrink-0 items-center justify-center text-fg-3 [&_svg]:size-4">{it.icon}</span> : null}
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{it.label}</span>
                  {it.description && <span className="block text-xs text-fg-3">{it.description}</span>}
                </span>
                {it.hint && <span className="mt-0.5 shrink-0 text-xs text-fg-4">{it.hint}</span>}
              </>
            );
            return it.href ? (
              <a key={i} role="menuitem" tabIndex={-1} href={it.href} target={it.external ? "_blank" : undefined} rel={it.external ? "noopener noreferrer" : undefined}
                className={itemClass(it.danger)} onClick={() => setOpen(false)}>{body}</a>
            ) : (
              <button key={i} type="button" role="menuitem" tabIndex={-1} aria-disabled={it.disabled || undefined} className={itemClass(it.danger, it.disabled)}
                onClick={() => { if (it.disabled) return; setOpen(false); it.onSelect?.(); }}>{body}</button>
            );
          })}
        </div>,
        // inside a modal <dialog> the page behind is inert, so the menu has to stay in the dialog's top layer
        trigger.current?.closest("dialog") ?? document.body,
      )}
    </div>
  );
}

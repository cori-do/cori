import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cx } from "../../ui/core";

// Floating menus and popovers of the sheet: fixed-position, closed by Escape, a click outside, or a scroll of the page.

export type MenuItem = { label: ReactNode; icon?: ReactNode; hint?: string; onClick: () => void; disabled?: boolean; danger?: boolean } | "sep";

/** Keeps a floating box at (x, y) inside the viewport once it is measured. */
function useFit(x: number, y: number) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ left: Math.max(8, Math.min(x, innerWidth - r.width - 8)), top: Math.max(8, Math.min(y, innerHeight - r.height - 8)) });
  }, [x, y]);
  return { ref, pos };
}

function useDismiss(ref: React.RefObject<HTMLDivElement | null>, onClose: () => void) {
  useEffect(() => {
    const down = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) onClose(); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } };
    const blur = () => onClose();
    document.addEventListener("mousedown", down, true);
    document.addEventListener("keydown", key, true);
    window.addEventListener("resize", blur);
    return () => { document.removeEventListener("mousedown", down, true); document.removeEventListener("keydown", key, true); window.removeEventListener("resize", blur); };
  }, [ref, onClose]);
}

export function Menu({ x, y, items, onClose }: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const { ref, pos } = useFit(x, y);
  useDismiss(ref, onClose);
  return (
    <div ref={ref} role="menu" style={pos}
      className="fixed z-50 min-w-52 rounded-lg bg-white py-1 text-sm shadow-lg ring-1 ring-zinc-200 dark:bg-zinc-900 dark:ring-zinc-700">
      {items.map((it, i) => it === "sep" ? <div key={i} className="my-1 border-t border-zinc-100 dark:border-zinc-800" /> : (
        <button key={i} role="menuitem" disabled={it.disabled}
          onClick={() => { onClose(); it.onClick(); }}
          className={cx("flex w-full cursor-pointer items-center gap-2.5 px-3 py-1.5 text-left disabled:cursor-default disabled:opacity-40",
            it.danger ? "text-rose-700 hover:bg-rose-50 dark:text-rose-400 dark:hover:bg-rose-500/10" : "hover:bg-zinc-100 dark:hover:bg-zinc-800")}>
          <span className="flex size-4 items-center justify-center text-zinc-500 [&>svg]:size-4">{it.icon}</span>
          <span className="flex-1">{it.label}</span>
          {it.hint && <span className="text-xs text-zinc-400">{it.hint}</span>}
        </button>
      ))}
    </div>
  );
}

/** A panel under `anchor` (a button's rect). */
export function Popover({ anchor, onClose, children, className }: { anchor: DOMRect; onClose: () => void; children: ReactNode; className?: string }) {
  const { ref, pos } = useFit(anchor.left, anchor.bottom + 4);
  useDismiss(ref, onClose);
  return (
    <div ref={ref} style={pos} className={cx("fixed z-50 rounded-lg bg-white p-3 text-sm shadow-lg ring-1 ring-zinc-200 dark:bg-zinc-900 dark:ring-zinc-700", className)}>
      {children}
    </div>
  );
}

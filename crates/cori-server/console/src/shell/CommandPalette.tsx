import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AppWindow, CornerDownLeft, Home, Inbox, Moon, Search, Settings, Sun, Terminal } from "lucide-react";
import { href, navigate } from "../lib/router";
import { useSession } from "../lib/session";
import { setTheme } from "../lib/theme";
import { cx, Kbd } from "../ui/core";
import { appTabs, settingsSections } from "./nav";

// ⌘K (Ctrl+K): jump to any page by typing a few letters of it.

let openIt: (() => void) | null = null;
export const openPalette = () => openIt?.();

type Item = { id: string; group: string; label: string; hint?: string; icon: ReactNode; keywords?: string; run: () => void };

export function PaletteHost() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    openIt = () => setOpen(true);
    const key = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setOpen((o) => !o); }
    };
    window.addEventListener("keydown", key);
    return () => { openIt = null; window.removeEventListener("keydown", key); };
  }, []);
  return open ? <Palette close={() => setOpen(false)} /> : null;
}

function Palette({ close }: { close: () => void }) {
  const s = useSession();
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const ref = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => { ref.current?.showModal(); }, []);

  const items = useMemo<Item[]>(() => {
    const go = (to: string) => () => { navigate(to); };
    const out: Item[] = [
      { id: "home", group: "Go to", label: "Home", icon: <Home />, run: go(href()) },
      { id: "inbox", group: "Go to", label: "Inbox", hint: s.waiting ? `${s.waiting} waiting` : undefined, icon: <Inbox />, keywords: "approvals decisions", run: go(href("inbox")) },
    ];
    if (s.builds) out.push({ id: "connect", group: "Go to", label: "Connect your agent", icon: <Terminal />, keywords: "mcp cli claude cursor install", run: go(href("connect")) });
    for (const sec of settingsSections(s.me, s.builds)) out.push({ id: `settings/${sec.key}`, group: "Settings", label: sec.label, hint: sec.description, icon: <Settings />, keywords: "settings", run: go(href("settings", sec.key)) });
    for (const a of s.apps) {
      const name = a.name || a.id;
      for (const t of appTabs(a.id, a, { admin: !!s.me?.can?.admin })) {
        out.push({ id: `app/${a.id}/${t.key}`, group: "Apps", label: t.key === "overview" ? name : `${name} › ${t.label}`, hint: t.key === "overview" ? (a.promoted ? `${a.promoted_release || ""} live` : "not live") : undefined,
          icon: <AppWindow />, keywords: `${a.id} ${t.key === "overview" ? "" : "tab"}`, run: go(t.href) });
      }
    }
    out.push({ id: "theme-dark", group: "Preferences", label: "Dark theme", icon: <Moon />, keywords: "appearance", run: () => setTheme("dark") });
    out.push({ id: "theme-light", group: "Preferences", label: "Light theme", icon: <Sun />, keywords: "appearance", run: () => setTheme("light") });
    return out;
  }, [s.apps, s.me, s.builds, s.waiting]);

  const shown = useMemo(() => {
    const terms = q.toLowerCase().split(/\s+/).filter(Boolean);
    // with nothing typed: the places, and each app once (not every tab)
    if (!terms.length) return items.filter((x) => x.group !== "Preferences" && !(x.group === "Apps" && x.label.includes(" › ")));
    const score = (x: Item) => {
      const text = `${x.label} ${x.keywords || ""} ${x.group}`.toLowerCase();
      if (!terms.every((t) => text.includes(t))) return -1;
      return (x.label.toLowerCase().startsWith(terms[0]) ? 2 : 0) + (x.label.includes(" › ") ? 0 : 1);
    };
    return items.map((x) => [x, score(x)] as const).filter(([, sc]) => sc >= 0).sort((a, b) => b[1] - a[1]).map(([x]) => x).slice(0, 40);
  }, [items, q]);

  useEffect(() => setI(0), [q]);
  useEffect(() => { list.current?.querySelector(`[data-i="${i}"]`)?.scrollIntoView({ block: "nearest" }); }, [i]);

  const pick = (x?: Item) => { if (!x) return; close(); x.run(); };
  const keys = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") { e.preventDefault(); setI((n) => Math.min(shown.length - 1, n + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setI((n) => Math.max(0, n - 1)); }
    else if (e.key === "Enter") { e.preventDefault(); pick(shown[i]); }
  };

  let lastGroup = "";
  return (
    <dialog ref={ref} onCancel={(e) => { e.preventDefault(); close(); }} onClick={(e) => { if (e.target === ref.current) close(); }} aria-label="Search or jump to"
      className="mx-auto mt-[12vh] w-[calc(100%-2rem)] max-w-xl animate-pop overflow-hidden rounded-2xl bg-surface p-0 text-fg shadow-2xl ring-1 ring-line backdrop:animate-fade">
      <div className="flex items-center gap-3 border-b border-line px-4">
        <Search className="size-4.5 shrink-0 text-fg-4" aria-hidden />
        <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={keys} placeholder="Search apps, pages and settings…"
          role="combobox" aria-expanded="true" aria-controls="palette-list" aria-activedescendant={shown[i] ? `pal-${shown[i].id}` : undefined}
          className="h-13 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-fg-4" />
        <Kbd>esc</Kbd>
      </div>
      <div ref={list} id="palette-list" role="listbox" className="max-h-[min(60vh,420px)] overflow-y-auto p-1.5">
        {shown.map((x, n) => {
          const head = x.group !== lastGroup ? (lastGroup = x.group) : null;
          return (
            <div key={x.id}>
              {head && <div className="px-2.5 pt-2.5 pb-1 text-xs font-medium text-fg-3">{head}</div>}
              <div id={`pal-${x.id}`} data-i={n} role="option" aria-selected={n === i} onMouseMove={() => setI(n)} onClick={() => pick(x)}
                className={cx("flex cursor-pointer items-center gap-3 rounded-lg px-2.5 py-2 text-[14px]", n === i ? "bg-surface-2 text-fg" : "text-fg-2")}>
                <span className="text-fg-3 [&_svg]:size-4">{x.icon}</span>
                <span className="min-w-0 flex-1 truncate">{x.label}</span>
                {x.hint && <span className="hidden max-w-[45%] truncate text-xs text-fg-4 sm:block">{x.hint}</span>}
                {n === i && <CornerDownLeft className="size-3.5 shrink-0 text-fg-4" aria-hidden />}
              </div>
            </div>
          );
        })}
        {shown.length === 0 && <div className="px-3 py-10 text-center text-fg-3">Nothing matches “{q}”.</div>}
      </div>
    </dialog>
  );
}

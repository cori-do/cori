import { useSyncExternalStore } from "react";

// Light, dark, or the system's: the choice is the person's (account menu), kept in this browser.
export type Theme = "system" | "light" | "dark";
const KEY = "cori.theme";
const listeners = new Set<() => void>();
const media = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;

function read(): Theme {
  try { const v = localStorage.getItem(KEY); return v === "light" || v === "dark" ? v : "system"; } catch { return "system"; }
}
let choice: Theme = read();

/** Writes the theme on <html> (`data-theme`), where the styles read it. */
export function applyTheme() {
  const dark = choice === "dark" || (choice === "system" && !!media?.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}
media?.addEventListener("change", () => { if (choice === "system") applyTheme(); });

export function setTheme(t: Theme) {
  choice = t;
  try { if (t === "system") localStorage.removeItem(KEY); else localStorage.setItem(KEY, t); } catch { /* private mode */ }
  applyTheme();
  listeners.forEach((l) => l());
}
export const useTheme = () => useSyncExternalStore((l) => { listeners.add(l); return () => { listeners.delete(l); }; }, () => choice);

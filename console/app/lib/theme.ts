// Theme management.
//
// Default: respect `prefers-color-scheme`. Manual override persists to
// `localStorage` under the same key the no-flash boot script in
// `root.tsx` reads (`cori-theme`).

export type ThemeChoice = "system" | "light" | "dark";

const KEY = "cori-theme";

export function readChoice(): ThemeChoice {
  if (typeof window === "undefined") return "system";
  const v = window.localStorage.getItem(KEY);
  if (v === "light" || v === "dark") return v;
  return "system";
}

export function effectiveTheme(choice: ThemeChoice): "light" | "dark" {
  if (choice !== "system") return choice;
  if (typeof window === "undefined") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

export function applyTheme(choice: ThemeChoice): void {
  if (typeof document === "undefined") return;
  const effective = effectiveTheme(choice);
  document.documentElement.classList.toggle("dark", effective === "dark");
}

export function setChoice(choice: ThemeChoice): void {
  if (typeof window === "undefined") return;
  if (choice === "system") window.localStorage.removeItem(KEY);
  else window.localStorage.setItem(KEY, choice);
  applyTheme(choice);
}

/** Cycle through system → light → dark → system. */
export function nextChoice(current: ThemeChoice): ThemeChoice {
  if (current === "system") return "light";
  if (current === "light") return "dark";
  return "system";
}

// Small icon button that cycles through system → light → dark → system.
// Renders a sun (light), moon (dark), or half-shaded "auto" glyph
// (system). Tooltip names the current choice.

import { useEffect, useState } from "react";
import {
  type ThemeChoice,
  applyTheme,
  nextChoice,
  readChoice,
  setChoice,
} from "../lib/theme";

const LABEL: Record<ThemeChoice, string> = {
  system: "Auto",
  light: "Light",
  dark: "Dark",
};

export function ThemeIconButton() {
  const [choice, setChoiceState] = useState<ThemeChoice>("system");

  useEffect(() => {
    setChoiceState(readChoice());
  }, []);

  // When in `system` mode, follow OS-level theme changes.
  useEffect(() => {
    if (choice !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => applyTheme("system");
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [choice]);

  function cycle() {
    const next = nextChoice(choice);
    setChoice(next);
    setChoiceState(next);
  }

  return (
    <button
      type="button"
      className="theme-icon-btn"
      onClick={cycle}
      title={`Theme: ${LABEL[choice]} — click to cycle`}
      aria-label={`Theme: ${LABEL[choice]}`}
    >
      <ThemeGlyph choice={choice} />
    </button>
  );
}

function ThemeGlyph({ choice }: { choice: ThemeChoice }) {
  if (choice === "light") return <SunGlyph />;
  if (choice === "dark") return <MoonGlyph />;
  return <AutoGlyph />;
}

function SunGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <circle cx="12" cy="12" r="4" />
      <path d="M12 3v2" />
      <path d="M12 19v2" />
      <path d="M3 12h2" />
      <path d="M19 12h2" />
      <path d="M5.6 5.6l1.4 1.4" />
      <path d="M17 17l1.4 1.4" />
      <path d="M5.6 18.4l1.4-1.4" />
      <path d="M17 7l1.4-1.4" />
    </svg>
  );
}

function MoonGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />
    </svg>
  );
}

function AutoGlyph() {
  // Circle with one half filled — the conventional "follows system" glyph.
  return (
    <svg
      viewBox="0 0 24 24"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 3.5a8.5 8.5 0 0 1 0 17z" fill="currentColor" stroke="none" />
    </svg>
  );
}

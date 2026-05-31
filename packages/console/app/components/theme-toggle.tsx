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

export function ThemeToggle() {
  const [choice, setChoiceState] = useState<ThemeChoice>("system");

  useEffect(() => {
    setChoiceState(readChoice());
  }, []);

  // Respond to system theme changes when in `system` mode.
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
      className="theme-toggle"
      onClick={cycle}
      title={`Theme: ${LABEL[choice]} — click to cycle`}
      aria-label={`Theme: ${LABEL[choice]}`}
    >
      {LABEL[choice]}
    </button>
  );
}

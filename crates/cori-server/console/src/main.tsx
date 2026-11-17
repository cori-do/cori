import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
import { App } from "./App";
import { initAuth, MOCK } from "./lib/api";
import { applyTheme, setTheme } from "./lib/theme";

// the demo takes its theme from its address too: `?theme=dark`
if (MOCK) { const t = new URLSearchParams(location.search).get("theme"); if (t === "dark" || t === "light") setTheme(t); }
applyTheme();
// the auth mode decides where the token comes from (and a sign-in returning from the issuer finishes here)
initAuth().finally(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  ),
);

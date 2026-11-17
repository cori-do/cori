import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import { applyTheme } from "../lib/theme";
import { Portal } from "./Portal";

// The apps portal, the console's second page (portal.html → dist-portal/portal.html): its own sign-in, the same look.
applyTheme();
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Portal />
  </StrictMode>,
);

import { defineConfig } from "vite";
import { reactRouter } from "@react-router/dev/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [reactRouter()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // Pre-bundle every dep the SPA imports at startup so Vite never
  // mid-session re-discovers and rehashes. Mid-session rehashing is
  // what triggers the "504 Outdated Optimize Dep" errors in the Tauri
  // webview (WKWebView caches the old `?v=` URL and Vite no longer
  // recognises that hash).
  //
  // "Every dep" means every one, including the two only reached once a
  // route module loads: discovering those on first navigation forces the
  // reload this list exists to prevent, and the app hangs on its
  // hydration fallback. Keep in sync with:
  //   grep -rho 'from "@tauri-apps/[^"]*"' app | sort -u
  optimizeDeps: {
    include: [
      "react",
      "react-dom",
      "react-dom/client",
      "react/jsx-dev-runtime",
      "react/jsx-runtime",
      "react-router",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/api/webviewWindow",
      "@tauri-apps/plugin-opener",
    ],
  },
}));

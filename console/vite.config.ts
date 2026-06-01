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
    ],
  },
}));

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const APP = "__APP_ID__";
const server = process.env.CORI_SERVER ?? "http://127.0.0.1:8080";
// @cori-do/client is not published on npm: `cori init` vendors it here from the cli's own copy.
const vendor = (f: string) => fileURLToPath(new URL(`./vendor/cori-client/${f}`, import.meta.url));

export default defineConfig({
  base: "./", // relative: the same build serves the root of the app's own host and /apps/<app>/ (cori up, previews)
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: /^@cori-do\/client\/react$/, replacement: vendor("react.ts") },
      { find: /^@cori-do\/client$/, replacement: vendor("index.ts") },
    ],
  },
  build: { outDir: "../dist", emptyOutDir: true },
  // `npm run dev` against `cori up`: the config comes from the promoted app, the API from the server
  server: {
    proxy: {
      "/__cori": { target: server, changeOrigin: true, rewrite: (p) => `/apps/${APP}${p}` },
      "/v1": { target: server, changeOrigin: true },
    },
  },
});

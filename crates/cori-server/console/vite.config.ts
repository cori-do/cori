import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// `npm run dev` proxies the API to a running server (`cori up`). CONSOLE_API points it elsewhere; it is not CORI_SERVER,
// which shells often export for the cli (and which may name production).
const api = process.env.CONSOLE_API ?? "http://127.0.0.1:8080";

// The console signs in with @cori-do/client's OIDC session: the core's own packages/client, imported from the repository.
const client = fileURLToPath(new URL("../../../packages/client", import.meta.url));

export default defineConfig(({ mode }) => ({
  // One self-contained page per build (scripts and styles inlined): dist/index.html, the console, which cori-server embeds
  // with include_str! and serves at `/`; and with `--mode portal`, dist-portal/portal.html, the apps portal, which it serves
  // at the root of the portal's host. The binary carries both and needs no asset routes; both folders are committed.
  plugins: [react(), tailwindcss(), viteSingleFile()],
  resolve: { alias: { "@cori-do/client": `${client}/index.ts` } },
  build: mode === "portal"
    ? { outDir: "dist-portal", emptyOutDir: true, reportCompressedSize: false, rollupOptions: { input: "portal.html" } }
    : { outDir: "dist", emptyOutDir: true, reportCompressedSize: false },
  server: {
    port: mode === "mock" ? 5175 : 5174,
    fs: { allow: [".", client] },
    proxy: mode === "mock" ? undefined : { "/v1": { target: api, changeOrigin: true }, "/apps": { target: api, changeOrigin: true } },
  },
}));

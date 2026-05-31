import { reactRouter } from "@react-router/dev/vite";
import { defineConfig } from "vite";

// Dev mode: SPA at http://localhost:5173 proxies /api/* to `cori work`
// (default 7878). The user starts `cori work` separately and pastes
// the tokenised URL printed in its banner.
export default defineConfig({
  plugins: [reactRouter()],
  resolve: {
    tsconfigPaths: true,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:7878",
        changeOrigin: false,
      },
    },
  },
});

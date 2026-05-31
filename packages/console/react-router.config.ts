import type { Config } from "@react-router/dev/config";

// SPA mode — no server-side rendering. The whole app runs in the
// browser; `cori-console` serves the static build output and the
// JSON API.
export default {
  ssr: false,
  future: {
    v8_middleware: true,
    v8_passThroughRequests: true,
    v8_splitRouteModules: true,
    v8_trailingSlashAwareDataRequests: true,
    v8_viteEnvironmentApi: true,
  },
} satisfies Config;

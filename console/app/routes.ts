import { type RouteConfig, route, index } from "@react-router/dev/routes";

// No index route — every Tauri window loads the SPA at a specific
// path. The launcher window is configured with `"url": "/launcher"` in
// tauri.conf.json; openRun/openSettings spawn the others. Choosing a
// workflow spawns nothing: it fills the launcher's own right pane (see
// `components/workflow-pane.tsx`).
//
// The bare "/" route is kept as a redirect-only fallback so a stray
// navigation lands somewhere sensible during dev.
export default [
  index("routes/_index.tsx"),
  route("launcher", "routes/launcher.tsx"),
  route("runs/live/:runId", "routes/run-live.tsx"),
  route("runs/:key/:utc", "routes/run-detail.tsx"),
  route("settings", "routes/manage.tsx", [
    route("capabilities", "routes/manage.capabilities.tsx"),
    route("providers", "routes/manage.providers.tsx"),
    route("workers", "routes/manage.workers.tsx"),
    route("runs", "routes/manage.runs.tsx"),
  ]),
] satisfies RouteConfig;

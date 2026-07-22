import { type RouteConfig, route, index } from "@react-router/dev/routes";

// No index route — every Tauri window loads the SPA at a specific
// path. The launcher window is configured with `"url": "/launcher"` in
// tauri.conf.json; openLaunch/openRun/openManage spawn the others.
//
// The bare "/" route is kept as a redirect-only fallback so a stray
// navigation lands somewhere sensible during dev.
export default [
  index("routes/_index.tsx"),
  route("launcher", "routes/launcher.tsx"),
  route("launch", "routes/launch.tsx"),
  route("runs/live/:runId", "routes/run-live.tsx"),
  route("runs/:key/:utc", "routes/run-detail.tsx"),
  route("manage", "routes/manage.tsx", [
    route("capabilities", "routes/manage.capabilities.tsx"),
    route("workers", "routes/manage.workers.tsx"),
    route("schedules", "routes/manage.schedules.tsx"),
    route("runs", "routes/manage.runs.tsx"),
    route("approvals", "routes/manage.approvals.tsx"),
  ]),
] satisfies RouteConfig;

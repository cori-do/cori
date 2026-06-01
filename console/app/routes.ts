import { type RouteConfig, index, route } from "@react-router/dev/routes";

export default [
  index("routes/dashboard.tsx"),
  route("run", "routes/run.tsx"),
  route("runs", "routes/runs.tsx"),
  route("runs/live/:runId", "routes/run-live.tsx"),
  route("runs/:key/:utc", "routes/run-detail.tsx"),
  route("workers", "routes/workers.tsx"),
  route("schedules", "routes/schedules.tsx"),
] satisfies RouteConfig;

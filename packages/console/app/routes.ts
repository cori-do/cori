import { type RouteConfig, index, route } from "@react-router/dev/routes";

export default [
  index("routes/dashboard.tsx"),
  route("runs", "routes/runs.tsx"),
  route("runs/:key/:utc", "routes/run-detail.tsx"),
] satisfies RouteConfig;

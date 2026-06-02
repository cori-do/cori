import { redirect } from "react-router";

// Fallback for stray "/" navigations — the real entry is /launcher,
// loaded by the launcher window per tauri.conf.json.
export function clientLoader() {
  return redirect("/launcher");
}

export default function Index() {
  return null;
}

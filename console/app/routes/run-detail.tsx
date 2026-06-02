import { getRun, type RunTrace } from "../lib/api";
import { RunView } from "../components/run-view";

export function meta() {
  return [{ title: "Run trace — Cori" }];
}

interface LoaderArgs {
  params: { key?: string; utc?: string };
}

export async function clientLoader({ params }: LoaderArgs): Promise<RunTrace> {
  const key = params.key;
  const utc = params.utc;
  if (!key || !utc) {
    throw new Response("missing run path", { status: 400 });
  }
  const filename = utc.endsWith(".json") ? utc : `${utc}.json`;
  return getRun({ key, filename });
}

export default function RunDetail({ loaderData }: { loaderData: RunTrace }) {
  return <RunView runId={loaderData.run_id} initialTrace={loaderData} />;
}

import { useParams } from "react-router";
import { RunView } from "../components/run-view";

export function meta() {
  return [{ title: "Live run — Cori" }];
}

export default function RunLive() {
  const { runId } = useParams();
  if (!runId) return null;
  return <RunView runId={runId} />;
}

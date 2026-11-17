import type { ReactNode } from "react";
import { del, errMsg, post } from "../../lib/api";
import { refreshAfterDecision } from "../../lib/decisions";
import { personName } from "../../lib/people";
import { invalidate } from "../../lib/query";
import type { Version } from "../../lib/types";
import { Badge, type Tone } from "../../ui/core";
import { confirm } from "../../ui/dialog";
import { toast } from "../../ui/toast";

// A release (`v1`, `v2`…) and the one action that moves what people use: "Go live", whichever way it goes. The server
// applies the gate: a release that was live before goes back without approval or migration; a new one waits for an
// approver on a protected app, or when it adds a write.

export const refreshReleases = (app: string) => invalidate(`/v1/apps/${app}`, "/v1/me", "/v1/apps", "decisions");

/** What "Go live" does for the person, as a button label; null when there is nothing to do. */
export function goLiveLabel(r: Version): string | null {
  if (!r.go_live || r.go_live === "pending") return null;
  if (r.go_live === "request") return "Request go-live";
  if (r.go_live === "approve") return "Approve and go live";
  if (r.section === "older") return "Roll back";
  return r.was_live ? "Go live again" : "Go live";
}

export async function goLive(app: string, r: Version, live: Version | undefined, frontUrl?: string | null) {
  const back = r.section === "older";
  const body: ReactNode = r.go_live === "request"
    ? <p>An approver decides it from their inbox{r.gate.reason ? <> ({r.gate.reason})</> : null}. <b>{r.release}</b> goes live once approved.</p>
    : <>
        <p><b>{r.release}</b> becomes what people use{frontUrl ? <> at <code className="text-fg">{frontUrl.replace(/^https?:\/\//, "")}</code></> : null}{live ? <>, instead of {live.release}</> : null}.</p>
        {r.go_live === "approve" && <p>You approve its change review{r.gate.reason ? <> ({r.gate.reason})</> : null}.</p>}
        {r.was_live && <p>It was live before: no approval and no migration needed.</p>}
        {r.pending_migrations.length > 0 && <p>These migrations run on the live data first, and cannot be undone: <code className="text-fg">{r.pending_migrations.join(", ")}</code></p>}
        {r.schema_ahead.length > 0 && <p className="text-amber-700 dark:text-amber-300">The live data already ran migrations this release does not have (<code>{r.schema_ahead.join(", ")}</code>): its code will run against that newer schema.</p>}
      </>;
  const title = r.go_live === "request" ? `Ask an approver to make ${r.release} live?` : back ? `Roll back to ${r.release}?` : `Make ${r.release} live?`;
  if (!(await confirm({ title, body, confirm: goLiveLabel(r) || "Go live" }))) return false;
  try {
    const out = await post<{ status?: string }>(`/v1/apps/${app}/versions/${r.hash}/promote`);
    if (out.status === "pending_approval") toast.success(`${r.release}: an approver decides it now`);
    else toast.success(`${r.release} is live`);
  } catch (e) { toast.error(errMsg(e)); }
  refreshReleases(app);
  refreshAfterDecision(app);
  return true;
}

export async function resetPreview(app: string, r: Version) {
  if (!(await confirm({ title: `Reset the preview data of ${r.release}?`, danger: true, confirm: "Reset",
    body: <p>Its preview databases are recreated from the migrations and seed files. Test data entered since is lost.</p> }))) return;
  try { await post(`/v1/apps/${app}/versions/${r.hash}/reseed`); toast.success("Preview data reset"); } catch (e) { toast.error(errMsg(e)); }
  invalidate(`/v1/apps/${app}`);
}

export async function deleteRelease(app: string, r: Version): Promise<boolean> {
  if (!(await confirm({ title: `Delete ${r.release}?`, danger: true, confirm: "Delete",
    body: <p>Its preview data{r.preview.stores.length ? <> ({r.preview.stores.join(", ")})</> : null}, its bundle and its preview address are deleted. The audit log and the run history keep it.</p> }))) return false;
  const base = `/v1/apps/${app}/versions/${r.hash}`;
  try { await del(base); toast.success(`${r.release} deleted`); }
  catch (e) {
    const m = errMsg(e);
    if (!/still running/.test(m)) { toast.error(m); return false; }
    if (!(await confirm({ title: "Stop its runs?", body: <p>{m}</p>, confirm: "Stop runs and delete", danger: true }))) return false;
    try { await del(`${base}?force=true`); toast.success(`${r.release} deleted, its runs stopped`); } catch (e2) { toast.error(errMsg(e2)); return false; }
  }
  invalidate(`/v1/apps/${app}`);
  return true;
}

/** Where a release stands, as a chip. */
export function gateChip(r: Version): { tone: Tone; label: string; title?: string } {
  const g = r.gate;
  switch (g.state) {
    case "live": return { tone: "success", label: "Live" };
    case "approved": return { tone: "success", label: `Approved by ${personName(g.by)}`, title: g.against ? `approved against ${g.against}` : undefined };
    case "requested": return { tone: "warning", label: "Waiting for an approver", title: g.reason };
    case "needs_approval": return { tone: "warning", label: "Needs approval", title: g.reason };
    default: return r.was_live ? { tone: "neutral", label: "Was live", title: "goes back without approval" } : { tone: "brand", label: "Ready", title: g.reason };
  }
}
export function GateBadge({ r }: { r: Version }) {
  const c = gateChip(r);
  return <Badge tone={c.tone} dot title={c.title}>{c.label}</Badge>;
}

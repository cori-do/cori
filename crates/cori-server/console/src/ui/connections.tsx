import { useEffect, useState } from "react";
import { Link2, Unplug } from "lucide-react";
import { ApiError, api, del, errMsg, post } from "../lib/api";
import { ago } from "../lib/format";
import { useQuery } from "../lib/query";
import type { ProviderAccount } from "../lib/types";
import { Badge, Button, Empty, ErrorNote, List, SkeletonRows, Time } from "./core";
import { confirm } from "./dialog";
import { toast } from "./toast";

// The accounts connected at the deployment's providers (Google), which runs act with: a person's own, or an app's.
// Connecting one goes to the provider's consent page and comes back here (`?connected=<provider>`, `?error=…`);
// disconnecting tells the provider to forget the access.

/** How a consent that came back to this page went, said once, then taken out of the address. */
function useConsentOutcome(reload: () => void) {
  useEffect(() => {
    const m = /\?(.*)$/.exec(location.hash);
    if (!m) return;
    const q = new URLSearchParams(m[1]);
    const connected = q.get("connected"), error = q.get("error");
    if (!connected && !error) return;
    if (error) toast.error(error); else toast.success("Account connected");
    history.replaceState(null, "", `${location.pathname}${location.search}${location.hash.replace(/\?.*$/, "")}`);
    reload();
  }, [reload]);
}

/** The providers and `base`'s connection to each (`/v1/connections`, or an app's `/v1/apps/<app>/connections`). */
export function Accounts({ base, mayEdit, whose }: { base: string; mayEdit: boolean; whose: "person" | "app" }) {
  const q = useQuery<{ providers: ProviderAccount[] }>(base, () => api<{ providers: ProviderAccount[] }>(base));
  useConsentOutcome(q.reload);
  const [busy, setBusy] = useState<string | null>(null);
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={1} />;
  if (!q.data.providers.length) return <Empty compact title="No account to connect">This server connects to no account provider.</Empty>;

  async function connect(p: ProviderAccount) {
    setBusy(p.provider);
    try { location.assign((await post<{ url: string }>(`${base}/${encodeURIComponent(p.provider)}`, {})).url); }
    catch (e) { toast.error(errMsg(e)); setBusy(null); }
  }
  async function disconnect(p: ProviderAccount) {
    const runs = whose === "person" ? "Your runs stop" : "Its runs stop";
    if (!(await confirm({ title: `Disconnect ${p.label}?`, danger: true, confirm: "Disconnect", body: <p>{runs} calling {p.connectors.join(", ")} with it until it is connected again, and {p.label} is told to forget the access.</p> }))) return;
    setBusy(p.provider);
    try { await del(`${base}/${encodeURIComponent(p.provider)}`); toast.success(`${p.label} disconnected`); } catch (e) { toast.error(errMsg(e)); }
    q.reload();
    setBusy(null);
  }

  return (
    <List>
      {q.data.providers.map((p) => {
        const c = p.connection;
        const lacking = !!c && c.missing.length > 0;
        return (
          <div key={p.provider} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium text-fg">{p.label}</span>
                {!c ? <Badge>Not connected</Badge> : lacking ? <Badge tone="warning" dot title={c.missing.join("\n")}>Needs more access</Badge> : <Badge tone="success" dot>Connected</Badge>}
              </div>
              <div className="truncate text-[13px] text-fg-3">
                {c?.account && <>{c.account} · </>}For {p.connectors.join(", ")}
                {c?.used_at && <> · used <Time at={c.used_at}>{ago(c.used_at)}</Time></>}
              </div>
            </div>
            {mayEdit && (
              <div className="flex items-center gap-1.5">
                {(!c || lacking) && <Button size="xs" variant="primary" icon={<Link2 />} loading={busy === p.provider} onClick={() => connect(p)}>{c ? "Connect again" : "Connect"}</Button>}
                {c && <Button size="xs" variant="ghost" icon={<Unplug />} disabled={busy === p.provider} onClick={() => disconnect(p)}>Disconnect</Button>}
              </div>
            )}
          </div>
        );
      })}
    </List>
  );
}

/** A run refused because an account is not connected (409 with `connect`): said with a way to connect it. True when it was. */
export function refusedFor(e: unknown): boolean {
  if (!(e instanceof ApiError) || e.status !== 409) return false;
  const c = e.body.connect as { url?: string | null; label?: string } | undefined;
  if (!c) return false;
  toast.error(e.message, c.url ? { action: { label: `Connect ${c.label ?? ""}`.trim(), href: c.url } } : {});
  return true;
}

// SPDX-License-Identifier: MIT-0
// React hooks over @cori-do/client. React is a peer dependency; import from "@cori-do/client/react".
import { createElement, Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { type Approval, type CoriClient, type RunView, type User, onDevIdentityChange } from "./index.ts";

/** Polls a run until it leaves `running`; every 5 s at most while it waits for a person (`pendingApproval`). */
export function useRun(client: CoriClient, runId: string | undefined, pollMs = 500): RunView | undefined {
  const [view, setView] = useState<RunView | undefined>();
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!runId) { setView(undefined); return; }
    let stop = false;
    const tick = async () => { try { const v = await client.runs.get(runId); if (stop) return; setView(v); if (v.status === "running") timer.current = setTimeout(tick, v.pendingApproval ? Math.max(pollMs, 5000) : pollMs) as unknown as number; } catch (e) { if (!stop) setView({ runId, status: "failed", error: String(e) }); } };
    tick();
    return () => { stop = true; if (timer.current) clearTimeout(timer.current); };
  }, [client, runId, pollMs]);
  return view;
}

/** The signed-in user as the server sees it (`GET /v1/me`); `loading` until the first answer, `error` when the call
 *  is refused (not signed in). Under dev auth it follows the identity chosen in the dev widget or the console. */
export function useUser(client: CoriClient): { sub?: string; roles: string[]; email?: string | null; kind?: string; loading: boolean; error?: string } {
  const [u, setU] = useState<{ sub?: string; roles: string[]; email?: string | null; kind?: string; loading: boolean; error?: string }>({ roles: [], loading: true });
  useEffect(() => {
    let stop = false;
    const load = () => client.me().then((m: User) => { if (!stop) setU({ sub: m.sub, roles: m.roles ?? [], email: m.email, kind: m.kind, loading: false }); }).catch((e) => { if (!stop) setU({ roles: [], loading: false, error: String(e?.message ?? e) }); });
    load();
    const off = onDevIdentityChange(load);
    return () => { stop = true; off(); };
  }, [client]);
  return u;
}

/** Renders `children` when the signed-in user has one of `roles` (any role when the list is empty), else `fallback`.
 *  A display guard only: the endpoint's own `allow` list is what the server enforces. */
export function Protected({ client, roles, children, fallback = null }: { client: CoriClient; roles: string[]; children?: ReactNode; fallback?: ReactNode }): ReactNode {
  const u = useUser(client);
  if (u.loading) return null;
  const ok = roles.length === 0 || roles.some((r) => u.roles.includes(r));
  return createElement(Fragment, null, ok ? children : fallback);
}

/** Pending approvals for the signed-in user, refreshed on an interval and on identity change; `decide` resolves one and refreshes. */
export function useApprovals(client: CoriClient, pollMs = 3000): { approvals: Approval[]; decide: (id: string, value: unknown) => Promise<void>; refresh: () => Promise<void> } {
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const refresh = async () => { try { setApprovals(await client.approvals.list()); } catch { /* not signed in yet */ } };
  useEffect(() => { refresh(); const t = setInterval(refresh, pollMs); const off = onDevIdentityChange(refresh); return () => { clearInterval(t); off(); }; }, [client, pollMs]);
  return { approvals, decide: async (id, value) => { await client.approvals.decide(id, value); await refresh(); }, refresh };
}

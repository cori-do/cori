import { onDevIdentityChange } from "@cori-do/client";
import { useUser } from "@cori-do/client/react";
import { ClipboardList, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge, Button, Card, CardHeader, EmptyState, ErrorBanner, Select, Spinner } from "../components/ui";
import { must } from "../lib/api";
import { cori, listRequests, type ListRequestsInput, type ListRequestsOutputs } from "../lib/cori";
import { STATUS_TONE, capitalized, euros, when } from "../lib/requests";

type Row = ListRequestsOutputs["latest"]["rows"][number];
type Status = NonNullable<ListRequestsInput["status"]>;
const STATUSES: Status[] = ["all", "pending", "approved", "declined"];

/** The latest requests (`list_requests`, sync): a member's own, every member's for a manager. The store decides which
 *  rows a role sees; this screen only asks. */
export function Requests() {
  const user = useUser(cori);
  const [status, setStatus] = useState<Status>("all");
  const [rows, setRows] = useState<Row[]>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    setError(undefined);
    try {
      const run = await must(listRequests.run({ status }));
      setRows(run.outputs?.latest.rows ?? []);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [status]);
  useEffect(() => { load(); return onDevIdentityChange(load); }, [load]);

  const manager = user.roles.includes("manager");
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Requests</h1>
          <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">{manager ? "Every member's requests, newest first." : "Your requests, newest first."}</p>
        </div>
        <div className="flex items-center gap-2">
          <label htmlFor="status" className="sr-only">Status</label>
          <Select id="status" value={status} onChange={(e) => setStatus(e.target.value as Status)} className="w-36">
            {STATUSES.map((s) => <option key={s} value={s}>{s === "all" ? "All statuses" : capitalized(s)}</option>)}
          </Select>
          <Button variant="ghost" onClick={load} aria-label="Refresh" disabled={loading}><RefreshCw className="size-4" aria-hidden /></Button>
        </div>
      </div>
      <Card>
        <CardHeader title={rows ? `${rows.length} request${rows.length === 1 ? "" : "s"}` : "Requests"} />
        {error ? (
          <div className="p-5"><ErrorBanner title="Could not load the requests" message={error} onRetry={load} /></div>
        ) : !rows || (loading && rows.length === 0) ? (
          <Spinner />
        ) : rows.length === 0 ? (
          <EmptyState icon={<ClipboardList className="size-5" />} title={status === "all" ? "No request yet" : `No ${status} request`} description="Requests you send from New request show up here." action={<a href="#/new" className="text-sm font-medium text-brand-600 hover:underline dark:text-brand-100">New request</a>} />
        ) : (
          <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
            {rows.map((r) => <RequestRow key={r.id} r={r} showRequester={manager} />)}
          </ul>
        )}
      </Card>
    </div>
  );
}

function RequestRow({ r, showRequester }: { r: Row; showRequester: boolean }) {
  return (
    <li className="flex flex-col gap-2 px-5 py-4 sm:flex-row sm:items-start sm:justify-between sm:gap-6">
      <div className="min-w-0 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="font-medium">{r.title}</p>
          <Badge>{r.category}</Badge>
        </div>
        {r.summary && r.summary !== r.title && <p className="text-sm text-zinc-600 dark:text-zinc-300">{r.summary}</p>}
        <p className="text-xs text-zinc-500 dark:text-zinc-400">
          {when(r.created_at)}{showRequester && r.requester ? ` · ${r.requester}` : ""}
          {r.decided_by ? ` · decided by ${r.decided_by === "rule" ? "the rule" : "a manager"}` : ""}
          {r.decision_note ? `: ${r.decision_note}` : ""}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-3 sm:flex-col sm:items-end sm:gap-1.5">
        <span className="text-sm font-semibold tabular-nums">{euros(r.amount_eur)}</span>
        <Badge tone={STATUS_TONE[r.status] ?? "neutral"}>{r.status}</Badge>
      </div>
    </li>
  );
}

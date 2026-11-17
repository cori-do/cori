import type { Approval } from "@cori-do/client";
import { useApprovals } from "@cori-do/client/react";
import { Check, Inbox, X } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, CardBody, EmptyState, ErrorBanner, Field, Textarea } from "../components/ui";
import { cori } from "../lib/cori";
import { euros, when, type Ask } from "../lib/requests";

/** The managers' inbox: every `submit_request` run waiting on its `approve_request` step. Deciding resumes the run,
 *  which records the outcome on the request; the decision is audited with the manager's identity. */
export function Approvals() {
  const { approvals, decide } = useApprovals(cori);
  const asks = approvals.filter((a) => a.workflow === "submit_request" && a.step === "approve_request");
  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Approvals</h1>
        <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">Requests the rule did not approve. Each one waits up to 7 days, then counts as declined.</p>
      </div>
      {asks.length === 0 ? (
        <Card><EmptyState icon={<Inbox className="size-5" />} title="Nothing waits for you" description="Requests above the rule's amount, and travel, show up here." /></Card>
      ) : (
        asks.map((a) => <Decision key={a.id} a={a} decide={decide} />)
      )}
    </div>
  );
}

function Decision({ a, decide }: { a: Approval; decide: (id: string, value: unknown) => Promise<void> }) {
  const ask = a.payload?.payload as Ask | undefined;
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState<"approve" | "decline">();
  const [error, setError] = useState<string>();
  async function answer(approved: boolean) {
    setBusy(approved ? "approve" : "decline");
    setError(undefined);
    try {
      await decide(a.id, note.trim() ? { approved, note: note.trim() } : { approved });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(undefined);
    }
  }
  if (!ask) return null;
  return (
    <Card>
      <CardBody className="space-y-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="text-base font-semibold">{ask.title}</h2>
              <Badge>{ask.category}</Badge>
            </div>
            <p className="text-xs text-zinc-500 dark:text-zinc-400">{ask.requester} · {when(a.created_at)} · {ask.reason}</p>
          </div>
          <span className="text-lg font-semibold tabular-nums">{euros(ask.amount_eur)}</span>
        </div>
        {ask.summary && <p className="text-sm">{ask.summary}</p>}
        {ask.details && <p className="rounded-lg bg-zinc-50 px-3 py-2 text-sm whitespace-pre-wrap text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">{ask.details}</p>}
        <Field label="Note for the requester" htmlFor={`note-${a.id}`} optional>
          <Textarea id={`note-${a.id}`} value={note} maxLength={500} onChange={(e) => setNote(e.target.value)} className="min-h-16" />
        </Field>
        {error && <ErrorBanner title="The decision was not recorded" message={error} />}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => answer(false)} loading={busy === "decline"} disabled={!!busy}><X className="size-4" aria-hidden />Decline</Button>
          <Button variant="primary" onClick={() => answer(true)} loading={busy === "approve"} disabled={!!busy}><Check className="size-4" aria-hidden />Approve</Button>
        </div>
      </CardBody>
    </Card>
  );
}

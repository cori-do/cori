// Excerpt of the vibe-coded SPA (the agent wrote it against front/src/lib/cori.ts).
// Under `cori up` the hosted front carries a dev widget (bottom right) to impersonate a role; `useUser` and `useApprovals` follow it.
import { cori, supportTicketTriage, refundRequest } from "./lib/cori";
import { useRun, useApprovals, useUser, Protected } from "@cori-do/client/react";

export function TicketPage({ ticketId }: { ticketId: string }) {
  const user = useUser(cori);                      // { sub, roles, loading } as the server sees the caller
  const [triage, setTriage] = useState<Awaited<ReturnType<typeof supportTicketTriage.run>>>();
  const [refundRunId, setRefundRunId] = useState<string>();
  const [refundKey] = useState(() => crypto.randomUUID()); // refund_request has `idempotency: header`: one key per form instance, so a double submit replays the first run
  const refund = useRun(cori, refundRunId);        // { status, steps, result, pendingApproval }

  return (
    <>
      <button onClick={async () => setTriage(await supportTicketTriage.run({ ticket_id: ticketId }))}>Triage</button>
      {triage?.result && <ReplyEditor draft={triage.result.reply_draft} priority={triage.result.priority} />}
      {triage?.runId && !triage.result && <p>Still running… <RunProgress runId={triage.runId} /></p>}

      <RefundForm onSubmit={async (v) => setRefundRunId((await refundRequest.start({ ticket_id: ticketId, ...v }, { idempotencyKey: refundKey })).runId)} />
      {refund.pendingApproval && <p>Waiting for a support lead ({refund.pendingApproval.to.role}).</p>}
      {refund.result && <p>Refund {refund.result.status}.</p>}

      <Protected client={cori} roles={["support_lead"]}><ApprovalsInbox /></Protected>
    </>
  );
}

function ApprovalsInbox() {
  const { approvals, decide } = useApprovals(cori); // approvals routed to the caller's roles, across workflows
  return approvals.map(a => (
    <div key={a.id}>
      <pre>{JSON.stringify(a.input, null, 2)}</pre>
      <button onClick={() => decide(a.id, { approved: true })}>Approve</button>
      <button onClick={() => decide(a.id, { approved: false, note: "declined" })}>Decline</button>
    </div>
  ));
}

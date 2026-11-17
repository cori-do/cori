import { useRun } from "@cori-do/client/react";
import { CheckCircle2, Hourglass, XCircle } from "lucide-react";
import { useState, type FormEvent } from "react";
import { RunResult } from "../components/RunResult";
import { Badge, Button, Card, CardBody, CardHeader, ErrorBanner, Field, Input, Textarea } from "../components/ui";
import { newIdempotencyKey } from "../lib/api";
import { cori, submitRequest, type SubmitRequestRun } from "../lib/cori";

/** Files a request (`submit_request`, async) and follows its run: sorted by the model, approved by the rule or waiting
 *  for a manager, then closed. */
export function NewRequest() {
  const [title, setTitle] = useState("");
  const [amount, setAmount] = useState("");
  const [details, setDetails] = useState("");
  const [touched, setTouched] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string>();
  const [runId, setRunId] = useState<string>();
  // one key per form: a double submit replays the first run (`idempotency: header`)
  const [key, setKey] = useState(newIdempotencyKey);
  const run = useRun(cori, runId) as SubmitRequestRun | undefined;

  const amountEur = Number(amount);
  const errors = {
    title: title.trim() ? undefined : "Say what you need",
    amount: amount.trim() !== "" && Number.isFinite(amountEur) && amountEur >= 0 ? undefined : "An amount in euros, 0 or more",
  };
  const valid = !errors.title && !errors.amount;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setTouched(true);
    if (!valid) return;
    setSending(true);
    setError(undefined);
    try {
      const { runId } = await submitRequest.start({ title: title.trim(), amount_eur: amountEur, details: details.trim() || undefined }, { idempotencyKey: key });
      setRunId(runId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSending(false);
    }
  }

  function again() {
    setTitle(""); setAmount(""); setDetails(""); setTouched(false); setRunId(undefined); setError(undefined);
    setKey(newIdempotencyKey());
  }

  if (runId) return <Following run={run} onAgain={again} />;

  return (
    <div className="mx-auto max-w-2xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">New request</h1>
        <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">
          Small amounts are approved by a rule at once; larger ones, and travel, go to a manager.
        </p>
      </div>
      <Card>
        <form onSubmit={submit} noValidate>
          <CardBody className="space-y-5">
            <Field label="What you need" htmlFor="title" error={touched ? errors.title : undefined}>
              <Input id="title" value={title} maxLength={200} placeholder="A second monitor" onChange={(e) => setTitle(e.target.value)} autoFocus />
            </Field>
            <Field label="Estimated cost (€)" htmlFor="amount" error={touched ? errors.amount : undefined}>
              <Input id="amount" type="number" inputMode="decimal" min={0} step="0.01" value={amount} placeholder="180" onChange={(e) => setAmount(e.target.value)} className="max-w-40" />
            </Field>
            <Field label="Why, and anything a manager should know" htmlFor="details" optional>
              <Textarea id="details" value={details} maxLength={2000} onChange={(e) => setDetails(e.target.value)} />
            </Field>
            {error && <ErrorBanner title="The request was not sent" message={error} />}
          </CardBody>
          <div className="flex justify-end border-t border-zinc-100 px-5 py-3 dark:border-zinc-800">
            <Button type="submit" variant="primary" loading={sending}>Send request</Button>
          </div>
        </form>
      </Card>
    </div>
  );
}

/** The run of the request just sent: its steps while it runs, the outcome when it ends. */
function Following({ run, onAgain }: { run?: SubmitRequestRun; onAgain: () => void }) {
  const waiting = run?.status === "running" && !!run.pendingApproval;
  const status = run?.result?.values?.status;
  return (
    <div className="mx-auto max-w-2xl space-y-6">
      <h1 className="text-2xl font-semibold tracking-tight">Your request</h1>
      <Card>
        <CardHeader
          title={!run || run.status === "running" ? (waiting ? "Recorded, waiting for a manager" : "Sending…") : run.status === "succeeded" ? "Done" : "It did not go through"}
          description={waiting ? "A manager decides it from their Approvals screen. You can leave this page: the request keeps its place." : undefined}
          action={status ? <Outcome status={status} /> : waiting ? <Badge tone="amber"><Hourglass className="size-3" aria-hidden />pending</Badge> : undefined}
        />
        <CardBody>{run ? <RunResult run={run} showSteps /> : <p className="text-sm text-zinc-500">Starting…</p>}</CardBody>
      </Card>
      <div className="flex gap-2">
        <Button variant="primary" onClick={onAgain}>New request</Button>
        <a href="#/requests" className="inline-flex h-9 items-center rounded-lg px-3.5 text-sm font-medium text-zinc-600 hover:bg-zinc-100 dark:text-zinc-300 dark:hover:bg-zinc-800">See all requests</a>
      </div>
    </div>
  );
}

function Outcome({ status }: { status: string }) {
  return status === "approved"
    ? <Badge tone="green"><CheckCircle2 className="size-3" aria-hidden />approved</Badge>
    : <Badge tone="red"><XCircle className="size-3" aria-hidden />{status}</Badge>;
}

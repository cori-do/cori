import { RotateCw } from "lucide-react";
import { errMsg, post } from "../lib/api";
import { ago } from "../lib/format";
import { href } from "../lib/router";
import type { IdentityEvent, IdentityRole } from "../lib/types";
import { Badge, Button, Card, Time, type Tone } from "./core";
import { toast } from "./toast";

// What Cori told the organization's identity provider (the client hook): each app's sign-in client, its roles, the events.

/** An app's sign-in client: ready, being set up by the organization, or missing. */
export function ClientState({ status, clientId, note }: { status?: string; clientId?: string | null; note?: string | null }) {
  if (status === "ready") return <span className="inline-flex flex-wrap items-center gap-2"><Badge tone="success" dot>Ready</Badge>{clientId && <code className="text-[12.5px] text-fg-3">{clientId}</code>}</span>;
  if (status === "pending") return <span className="inline-flex flex-wrap items-center gap-2"><Badge tone="warning" dot title="Cori asks again until it answers">Being set up</Badge>{note && <span className="text-[13px] text-fg-3">{note}</span>}</span>;
  return <Badge tone="danger" dot title="nobody can sign in to the app until there is one">Missing</Badge>;
}

const ROLE: Record<IdentityRole["state"], [Tone, string]> = { pending: ["warning", "Not confirmed yet"], confirmed: ["success", "Confirmed"], deleting: ["neutral", "Being removed"] };
export const RoleState = ({ r }: { r: IdentityRole }) => <Badge tone={ROLE[r.state][0]} dot>{ROLE[r.state][1]}</Badge>;

const EVENT: Record<IdentityEvent["status"], [Tone, string]> = { queued: ["brand", "Waiting"], done: ["success", "Delivered"], dead: ["danger", "Given up"], cancelled: ["neutral", "Cancelled"] };

/** What the receiver answered last, in a line. */
const answer = (e: IdentityEvent) => {
  const text = e.last_answer && e.last_answer !== `http ${e.last_status}` ? e.last_answer : "";
  return [e.last_status, text].filter((x) => x !== null && x !== undefined && x !== "").join(" · ");
};
const detail = (e: IdentityEvent) => {
  const b = typeof e.body === "object" && e.body ? (e.body as Record<string, unknown>) : {};
  return typeof b.role === "string" ? b.role : typeof b.email === "string" ? b.email : "";
};

/** The hook's events, newest first; `retry` (admins) sends a waiting or given-up one again. */
export function EventsList({ events, showApp, retry, reload }: { events: IdentityEvent[]; showApp?: boolean; retry?: boolean; reload?: () => void }) {
  async function again(e: IdentityEvent) {
    try {
      const r = await post<{ status: string }>(`/v1/identity/events/${e.id}/retry`);
      (r.status === "done" ? toast.success : toast)(r.status === "done" ? `${e.event} delivered` : `${e.event} sent again: still ${r.status}`);
      reload?.();
    } catch (err) { toast.error(errMsg(err)); }
  }
  return (
    <Card className="divide-y divide-line overflow-hidden">
      {events.map((e) => {
        const [tone, label] = EVENT[e.status];
        return (
          <div key={e.id} className="flex flex-wrap items-center gap-x-4 gap-y-1.5 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                <code className="text-[12.5px] font-medium text-fg">{e.event}</code>
                {detail(e) && <span className="text-[13px] text-fg-2">{detail(e)}</span>}
                {showApp && (e.app ? <a href={href("app", e.app)} className="link text-[13px]">{e.app}</a> : <span className="text-[13px] text-fg-3">people</span>)}
              </div>
              <div className="mt-0.5 text-[13px] break-words text-fg-3">
                <Time at={e.occurred_at}>{ago(e.occurred_at)}</Time> · {e.attempts} attempt{e.attempts === 1 ? "" : "s"}{answer(e) ? ` · ${answer(e)}` : ""}{e.status === "queued" ? ` · next ${ago(e.next_at)}` : ""}
              </div>
            </div>
            <Badge tone={tone} dot>{label}</Badge>
            {retry && (e.status === "dead" || e.status === "queued") && <Button size="xs" variant="ghost" icon={<RotateCw />} onClick={() => again(e)}>Send again</Button>}
          </div>
        );
      })}
    </Card>
  );
}

/** What wraps a secret's data key, in words. */
export function keyLabel(keyBy?: string): { label: string; title: string } {
  if (!keyBy || keyBy === "master") return { label: "Master key", title: "the deployment's master key (CORI_MASTER_KEY)" };
  const [service, key, version] = keyBy.split(":");
  if (service === "vault-transit") return { label: `Vault · ${key} v${version}`, title: `Vault Transit key ${key}, version ${version}: Vault holds the key, Cori the sealed value` };
  return { label: keyBy, title: keyBy };
}

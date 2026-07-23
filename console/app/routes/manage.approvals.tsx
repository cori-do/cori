import { useEffect, useState } from "react";
import { useRevalidator } from "react-router";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  decideApproval,
  listApprovals,
  listDecidedApprovals,
  onApprovalsChanged,
  type ApprovalDecisionEntry,
  type ApprovalKind,
  type ApprovalRequest,
} from "../lib/api";
import { formatRelative } from "../lib/format";

export function meta() {
  return [{ title: "Inbox — Cori" }];
}

interface InboxData {
  pending: ApprovalRequest[];
  decided: ApprovalDecisionEntry[];
}

export async function clientLoader(): Promise<InboxData> {
  const [pending, decided] = await Promise.all([
    listApprovals().catch(() => [] as ApprovalRequest[]),
    listDecidedApprovals().catch(() => [] as ApprovalDecisionEntry[]),
  ]);
  return { pending, decided };
}

export default function Approvals({ loaderData }: { loaderData: InboxData }) {
  const revalidator = useRevalidator();
  const [pending, setPending] = useState(loaderData.pending);

  // Live updates: the Rust watcher emits on every change to pending/.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    onApprovalsChanged((p) => {
      if (cancelled) return;
      setPending(p);
      revalidator.revalidate(); // refresh the decided history too
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [revalidator]);

  return (
    <>
      <p className="hint" style={{ marginTop: 0 }}>
        Human gates from <code>~/.cori/approvals/</code> — run requests and
        trust consents from agents (via <code>cori mcp</code>), and action
        items like capability re-authentication. Nothing here ever
        auto-approves.
      </p>

      {pending.length === 0 ? (
        <div className="empty">Nothing waiting on you. 🎉</div>
      ) : (
        pending.map((a) => <PendingCard key={a.nonce} approval={a} />)
      )}

      {loaderData.decided.length > 0 && (
        <>
          <h4
            style={{
              margin: "20px 0 8px",
              fontSize: 11,
              textTransform: "uppercase",
              color: "var(--muted)",
              letterSpacing: "0.06em",
            }}
          >
            Recently decided
          </h4>
          {loaderData.decided.map((d) => (
            <div
              className="card"
              key={`${d.nonce}-${d.decided_at}`}
              style={{ display: "flex", alignItems: "center", gap: 10, padding: "8px 12px" }}
            >
              <span className={`pill ${d.decision === "approved" ? "ok" : "muted"}`}>
                {d.decision}
              </span>
              <code style={{ fontSize: 12, color: "var(--muted)" }}>{d.nonce}</code>
              <span style={{ fontSize: 12, color: "var(--muted)", marginLeft: "auto" }}>
                via {d.via} · {formatRelative(d.decided_at)}
              </span>
            </div>
          ))}
        </>
      )}
    </>
  );
}

function PendingCard({ approval: a }: { approval: ApprovalRequest }) {
  const [busy, setBusy] = useState(false);
  const decide = (approved: boolean) => {
    setBusy(true);
    // The watcher event removes the card; on error just re-enable
    // (the item may have expired meanwhile).
    decideApproval(a.nonce, approved)
      .catch(() => {})
      .finally(() => setBusy(false));
  };
  const isAction = a.kind === "reauth_required";
  const loginCommand =
    typeof a.payload.login_command === "string" ? a.payload.login_command : null;

  return (
    <div className="card approval-card">
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span className={`pill ${pillFor(a.kind)}`}>{kindLabel(a.kind)}</span>
        <span style={{ fontSize: 12, color: "var(--muted)" }}>via {a.requested_by}</span>
        <span style={{ fontSize: 12, color: "var(--muted)", marginLeft: "auto" }}>
          {formatRelative(a.created_at)} · expires {formatRelative(a.expires_at)}
        </span>
      </div>

      <p style={{ margin: "10px 0", fontSize: 13.5, lineHeight: 1.45 }}>{a.message}</p>

      {isAction && loginCommand && (
        <div className="approval-command">
          <code>{loginCommand}</code>
          <button
            type="button"
            className="btn"
            onClick={() => void navigator.clipboard.writeText(loginCommand)}
            title="Copy command"
          >
            Copy
          </button>
        </div>
      )}

      <FactsTable payload={a.payload} />

      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        {isAction ? (
          <button type="button" className="btn" disabled={busy} onClick={() => decide(false)}>
            Dismiss
          </button>
        ) : (
          <>
            <button
              type="button"
              className="btn approval-approve"
              disabled={busy}
              onClick={() => decide(true)}
            >
              Approve
            </button>
            <button type="button" className="btn" disabled={busy} onClick={() => decide(false)}>
              Decline
            </button>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Structured payload rendered as a proper key → value table. `params`
 * objects are flattened one level so each workflow parameter gets its
 * own readable row instead of a JSON blob.
 */
function FactsTable({ payload }: { payload: Record<string, unknown> }) {
  const rows: Array<[string, string]> = [];
  const push = (k: string, v: unknown) => {
    if (v === undefined || v === null || v === "" || typeof v === "object") return;
    rows.push([k, String(v)]);
  };
  push("source", payload.source ?? payload.remote_ref);
  if (typeof payload.sha === "string") push("commit", payload.sha.slice(0, 12));
  if (typeof payload.pinned_sha === "string")
    push("consented", payload.pinned_sha.slice(0, 12));
  if (typeof payload.new_sha === "string")
    push("upstream now", payload.new_sha.slice(0, 12));
  push("cron", payload.schedule);
  push("timezone", payload.schedule_tz);
  push("workflow", payload.workflow_name ?? payload.workflow_id);
  push("steps", payload.steps);
  if (payload.dry_run === true) push("mode", "dry run");
  push("capability", payload.capability);
  push("failed step", payload.step);
  push("error", payload.error);
  if (payload.params && typeof payload.params === "object") {
    for (const [k, v] of Object.entries(payload.params as Record<string, unknown>)) {
      rows.push([`param · ${k}`, typeof v === "string" ? v : JSON.stringify(v)]);
    }
  }
  if (payload.capabilities && typeof payload.capabilities === "object") {
    for (const [k, v] of Object.entries(payload.capabilities as Record<string, unknown>)) {
      if (Array.isArray(v) && v.length > 0) rows.push([`declares · ${k}`, v.join(", ")]);
    }
  }
  if (rows.length === 0) return null;
  return (
    <table className="approval-table">
      <tbody>
        {rows.map(([k, v]) => (
          <tr key={k}>
            <td className="approval-table-key">{k}</td>
            <td className="approval-table-val">
              <code>{v}</code>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function kindLabel(kind: ApprovalKind): string {
  switch (kind) {
    case "run_confirm":
      return "run request";
    case "trust_consent":
      return "trust request";
    case "schedule_reconsent":
      return "schedule changed";
    case "step_gate":
      return "step approval";
    case "reauth_required":
      return "sign-in needed";
  }
}

function pillFor(kind: ApprovalKind): string {
  if (kind === "trust_consent") return "bad";
  if (kind === "reauth_required") return "warn";
  return "warn";
}

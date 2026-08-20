import { useCallback, useEffect, useState } from "react";
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

export interface InboxData {
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
  return <Inbox initialData={loaderData} />;
}

/**
 * Reusable inbox content. It owns its refresh cycle so the same view can
 * live in the launcher without depending on a route loader revalidation.
 */
export function Inbox({ initialData }: { initialData?: InboxData }) {
  const [data, setData] = useState<InboxData>(
    initialData ?? { pending: [], decided: [] },
  );
  const [loading, setLoading] = useState(initialData === undefined);

  const refresh = useCallback(async () => {
    try {
      const [pending, decided] = await Promise.all([
        listApprovals().catch(() => [] as ApprovalRequest[]),
        listDecidedApprovals().catch(() => [] as ApprovalDecisionEntry[]),
      ]);
      setData({ pending, decided });
    } finally {
      setLoading(false);
    }
  }, []);

  // Live updates: the Rust watcher emits on every change to pending/.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    if (initialData === undefined) void refresh();
    onApprovalsChanged(() => {
      if (!cancelled) void refresh();
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
  }, [initialData, refresh]);

  if (loading) {
    return <div className="empty">Loading inbox…</div>;
  }

  return (
    <>
      <p className="hint" style={{ marginTop: 0 }}>
        Human gates from <code>~/.cori/approvals/</code> — run requests and
        trust consents from agents (via <code>cori mcp</code>), and action
        items like capability re-authentication. Nothing here ever
        auto-approves.
      </p>

      {data.pending.length === 0 ? (
        <div className="empty">Nothing waiting on you. 🎉</div>
      ) : (
        data.pending.map((a) => (
          <PendingCard key={a.nonce} approval={a} onDecided={refresh} />
        ))
      )}

      {data.decided.length > 0 && (
        <>
          <h2>Recently decided</h2>
          {data.decided.map((d) => (
            <div className="card decided-row" key={`${d.nonce}-${d.decided_at}`}>
              <span className={`pill ${d.decision === "approved" ? "ok" : "muted"}`}>
                {d.decision}
              </span>
              <code>{d.nonce}</code>
              <span className="decided-meta">
                via {d.via} · {formatRelative(d.decided_at)}
              </span>
            </div>
          ))}
        </>
      )}
    </>
  );
}

function PendingCard({
  approval: a,
  onDecided,
}: {
  approval: ApprovalRequest;
  onDecided: () => void | Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [answer, setAnswer] = useState("");
  const [note, setNote] = useState("");
  const decide = (approved: boolean, response?: Record<string, unknown>) => {
    setBusy(true);
    // The watcher event removes the card; on error just re-enable
    // (the item may have expired meanwhile).
    decideApproval(a.nonce, approved, response)
      .then(() => onDecided())
      .catch(() => {})
      .finally(() => setBusy(false));
  };
  const isAction = a.kind === "reauth_required";
  // An agent question: the decision carries the answer back to the
  // blocked request_input call.
  const isInput = a.kind === "agent_input";
  const inputOptions: string[] = isInput && Array.isArray(a.payload.options)
    ? (a.payload.options as unknown[]).filter((o): o is string => typeof o === "string")
    : [];
  const inputDefault =
    typeof a.payload.default === "string" ? a.payload.default : null;
  // An agent approval: a denial may carry a note — information the
  // agent reads, not an error.
  const isAgentApproval = a.kind === "agent_approval";
  const loginCommand =
    typeof a.payload.login_command === "string" ? a.payload.login_command : null;

  return (
    <div className="card approval-card">
      <div className="approval-head">
        <span className={`pill ${pillFor(a.kind)}`}>{kindLabel(a.kind)}</span>
        <span className="approval-from">via {a.requested_by}</span>
        <span className="approval-from approval-when">
          {formatRelative(a.created_at)} · expires {formatRelative(a.expires_at)}
        </span>
      </div>

      <p className="approval-message">{a.message}</p>

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

      {isAgentApproval && (
        <input
          className="approval-note"
          type="text"
          placeholder="Note to the agent (optional — sent with either decision)"
          value={note}
          onChange={(e) => setNote(e.target.value)}
          disabled={busy}
        />
      )}

      <div className="approval-card-actions">
        {isAction ? (
          <button type="button" className="btn" disabled={busy} onClick={() => decide(false)}>
            Dismiss
          </button>
        ) : isInput ? (
          <>
            {inputOptions.map((opt) => (
              <button
                key={opt}
                type="button"
                className={`btn ${opt === inputDefault ? "approval-approve" : ""}`}
                disabled={busy}
                onClick={() => decide(true, { answer: opt })}
              >
                {opt}
              </button>
            ))}
            {inputOptions.length === 0 && (
              <>
                <input
                  className="approval-note"
                  type="text"
                  placeholder={inputDefault ? `Answer (default: ${inputDefault})` : "Answer"}
                  value={answer}
                  onChange={(e) => setAnswer(e.target.value)}
                  disabled={busy}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && answer.trim() !== "")
                      decide(true, { answer: answer.trim() });
                  }}
                />
                <button
                  type="button"
                  className="btn approval-approve"
                  disabled={busy || answer.trim() === ""}
                  onClick={() => decide(true, { answer: answer.trim() })}
                >
                  Answer
                </button>
              </>
            )}
            <button type="button" className="btn" disabled={busy} onClick={() => decide(false)}>
              Dismiss
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="btn approval-approve"
              disabled={busy}
              onClick={() =>
                decide(true, isAgentApproval && note.trim() !== "" ? { note: note.trim() } : undefined)
              }
            >
              Approve
            </button>
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() =>
                decide(false, isAgentApproval && note.trim() !== "" ? { note: note.trim() } : undefined)
              }
            >
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
  push("action", payload.action);
  push("agent", payload.agent);
  push("default", payload.default);
  if (payload.effects_diff && typeof payload.effects_diff === "object") {
    const diff = payload.effects_diff as Record<string, unknown>;
    for (const key of ["added", "removed"]) {
      const list = diff[key];
      if (Array.isArray(list) && list.length > 0)
        rows.push([`effects ${key}`, list.map((v) => String(v)).join(", ")]);
    }
  }
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
    case "agent_input":
      return "agent question";
    case "agent_approval":
      return "approval request";
  }
}

function pillFor(kind: ApprovalKind): string {
  if (kind === "trust_consent" || kind === "agent_approval") return "bad";
  if (kind === "reauth_required") return "warn";
  return "warn";
}

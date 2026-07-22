import { useCallback, useState } from "react";
import {
  connectCapability,
  isIpcError,
  listCapabilities,
  type CapabilityInfo,
} from "../lib/api";

export function meta() {
  return [{ title: "Capabilities — Cori" }];
}

export async function clientLoader(): Promise<CapabilityInfo[]> {
  return listCapabilities();
}

export default function Capabilities({
  loaderData,
}: {
  loaderData: CapabilityInfo[];
}) {
  const [caps, setCaps] = useState<CapabilityInfo[]>(loaderData);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});

  const connect = useCallback(
    async (id: string) => {
      setConnecting(id);
      setErrors((e) => ({ ...e, [id]: "" }));
      try {
        const updated = await connectCapability({ id });
        setCaps((cs) => cs.map((c) => (c.id === id ? updated : c)));
      } catch (e) {
        const message = isIpcError(e) ? e.message : String(e);
        setErrors((errs) => ({ ...errs, [id]: message }));
        // Connect may still have installed the binary — refresh state.
        listCapabilities().then(setCaps).catch(() => {});
      } finally {
        setConnecting(null);
      }
    },
    [],
  );

  return (
    <>
      <p className="hint" style={{ marginTop: 0 }}>
        Tools Cori can install and sign in to for you. Connecting opens
        your browser for the provider's consent screen — nothing else to
        set up.
      </p>

      {caps.length === 0 ? (
        <div className="empty">No connectable capabilities in this build.</div>
      ) : (
        caps.map((c) => (
          <div className="card" key={c.id}>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <div style={{ flex: 1, minWidth: 0 }}>
                <h3 style={{ margin: 0, display: "flex", alignItems: "baseline", gap: 8 }}>
                  {c.display_name}
                  <StatePill cap={c} busy={connecting === c.id} />
                </h3>
                <div className="hint" style={{ marginTop: 4 }}>
                  <code>{c.id}</code>
                  {c.path ? <> — <code>{c.path}</code></> : " — not installed yet"}
                </div>
              </div>
              <ConnectButton
                cap={c}
                busy={connecting === c.id}
                anyBusy={connecting !== null}
                onConnect={() => connect(c.id)}
              />
            </div>
            {connecting === c.id && (
              <p className="hint" style={{ marginBottom: 0 }}>
                Waiting for the browser sign-in to finish… complete the
                consent screen, then come back here.
              </p>
            )}
            {errors[c.id] ? (
              <p className="hint" style={{ marginBottom: 0, color: "var(--err, #c0392b)" }}>
                {errors[c.id]}
              </p>
            ) : null}
          </div>
        ))
      )}
    </>
  );
}

function StatePill({ cap, busy }: { cap: CapabilityInfo; busy: boolean }) {
  if (busy) return <span className="pill warn">connecting…</span>;
  if (!cap.installed) return <span className="pill muted">not installed</span>;
  if (cap.authed === true) return <span className="pill ok">connected</span>;
  if (cap.authed === false) return <span className="pill warn">signed out</span>;
  return <span className="pill muted">installed</span>;
}

function ConnectButton({
  cap,
  busy,
  anyBusy,
  onConnect,
}: {
  cap: CapabilityInfo;
  busy: boolean;
  anyBusy: boolean;
  onConnect: () => void;
}) {
  if (cap.authed === true) {
    return (
      <button className="btn" disabled={anyBusy} onClick={onConnect}>
        Reconnect
      </button>
    );
  }
  if (!cap.connectable) {
    return (
      <button className="btn" disabled title="No Cori-provisioned OAuth client in this build">
        Connect
      </button>
    );
  }
  return (
    <button className="btn primary" disabled={anyBusy} onClick={onConnect}>
      {busy ? "Connecting…" : cap.installed ? "Connect" : "Install & connect"}
    </button>
  );
}

// The AI Providers tab.
//
// One idea drives the whole page: **an ordered list of places an `llm`
// step can run**. Subscriptions and API keys are rows in the same list,
// because the user's real question is "which one gets used first", and
// that question has one answer, not two. The old version split them into
// three stacked sections plus a mode radio, which meant the ordering was
// implied by a setting somewhere else on the page — you had to hold the
// rule in your head to predict what would happen.
//
// So: rank, reorder, toggle, and (per row, on demand) pick the model for
// each capability tier. The banner at the top states the outcome in a
// sentence, so the page answers its own question without being read
// top-to-bottom.

import { useCallback, useState } from "react";
import { BackendLogo } from "../components/provider-icons";
import { ProviderKeyForm } from "../components/provider-key-form";
import {
  getLlmSettings,
  isIpcError,
  listLlmProviders,
  MODEL_TIERS,
  refreshLlmSettings,
  setLlmBackendEnabled,
  setLlmBackendModel,
  setLlmPriority,
  type LlmBackendInfo,
  type LlmProviderInfo,
  type LlmSettings,
  type ModelTier,
} from "../lib/api";

export function meta() {
  return [{ title: "AI Providers — Cori" }];
}

interface ProvidersData {
  settings: LlmSettings;
  /** API-key state, keyed by provider id, for the inline key form. */
  providers: LlmProviderInfo[];
}

export async function clientLoader(): Promise<ProvidersData> {
  const [settings, providers] = await Promise.all([
    getLlmSettings(),
    listLlmProviders(),
  ]);
  return { settings, providers };
}

const TIER_HELP: Record<ModelTier, string> = {
  fast: "Classification, extraction, short rewrites.",
  balanced: "The default for steps that don't say otherwise.",
  deep: "Multi-constraint reasoning and long synthesis.",
};

export default function Providers({ loaderData }: { loaderData: ProvidersData }) {
  const [settings, setSettings] = useState(loaderData.settings);
  const [providers, setProviders] = useState(loaderData.providers);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Every mutation returns the whole settings object, so the list, the
  // banner and the status pills can never drift out of sync with each
  // other — there is one server-rendered truth per interaction.
  const apply = useCallback(
    async (key: string, action: () => Promise<LlmSettings>) => {
      setBusy(key);
      setError(null);
      try {
        setSettings(await action());
      } catch (e) {
        setError(isIpcError(e) ? e.message : String(e));
      } finally {
        setBusy(null);
      }
    },
    [],
  );

  const move = useCallback(
    (id: string, delta: number) => {
      const order = settings.backends.map((b) => b.id);
      const from = order.indexOf(id);
      const to = from + delta;
      if (from < 0 || to < 0 || to >= order.length) return;
      [order[from], order[to]] = [order[to], order[from]];
      void apply(`move:${id}`, () => setLlmPriority({ order }));
    },
    [settings.backends, apply],
  );

  return (
    <>
      <ActiveBanner settings={settings} />

      {error ? (
        <p className="hint" style={{ color: "var(--red)" }}>
          {error}
        </p>
      ) : null}

      <div className="section-head">
        <h2>Priority</h2>
        <p className="hint">
          Each <code>llm</code> step runs on the first provider here that's
          ready. Drag order with the arrows; turn one off to skip it entirely.
        </p>
        <button
          className="btn"
          disabled={busy !== null}
          onClick={() => apply("refresh", refreshLlmSettings)}
        >
          {busy === "refresh" ? "Checking…" : "Re-check"}
        </button>
      </div>

      <ol className="backend-list">
        {settings.backends.map((b, i) => (
          <BackendRow
            key={b.id}
            backend={b}
            isFirst={i === 0}
            isLast={i === settings.backends.length - 1}
            expanded={expanded === b.id}
            busy={busy}
            provider={providers.find((p) => p.id === b.id)}
            onToggleExpand={() =>
              setExpanded((cur) => (cur === b.id ? null : b.id))
            }
            onMove={(delta) => move(b.id, delta)}
            onSetEnabled={(enabled) =>
              apply(`enable:${b.id}`, () =>
                setLlmBackendEnabled({ backend: b.id, enabled }),
              )
            }
            onSetModel={(tier, model) =>
              apply(`model:${b.id}:${tier}`, () =>
                setLlmBackendModel({ backend: b.id, tier, model }),
              )
            }
            onProviderChanged={(updated) => {
              setProviders((ps) =>
                ps.map((p) => (p.id === updated.id ? updated : p)),
              );
              // A key that just appeared changes readiness and therefore
              // which backend the banner names.
              void apply(`key:${b.id}`, getLlmSettings);
            }}
          />
        ))}
      </ol>

      <p className="hint">
        Settings apply to runs on <strong>this machine</strong>. A shared
        worker (<code>cori work --shared</code>) always uses API keys — it
        can't spend one person's personal subscription on everyone's behalf.
      </p>
    </>
  );
}

/** The one-sentence answer to "what happens when I run a workflow?" */
function ActiveBanner({ settings }: { settings: LlmSettings }) {
  if (!settings.active) {
    return (
      <div className="active-banner is-blocked">
        <span className="pill warn">not ready</span>
        <div>
          <strong>No provider is ready.</strong>
          <p className="hint">
            {settings.blocked_reason ??
              "Sign in to a subscription or add an API key below."}
          </p>
        </div>
      </div>
    );
  }
  const a = settings.active;
  return (
    <div className="active-banner">
      <span className="pill ok">active</span>
      <BackendLogo backendId={a.backend_id} size={16} />
      <div>
        <strong>{a.display_name}</strong> runs your <code>llm</code> steps,
        using <code>{a.model}</code> for <code>{a.tier}</code> work.
        <p className="hint">
          {a.kind === "subscription"
            ? `Paid for by your ${a.subscription_name} plan — no per-token cost.`
            : "Billed per token to your API key."}{" "}
          Steps that ask for a different tier use that row's other models.
        </p>
      </div>
    </div>
  );
}

function BackendRow({
  backend,
  isFirst,
  isLast,
  expanded,
  busy,
  provider,
  onToggleExpand,
  onMove,
  onSetEnabled,
  onSetModel,
  onProviderChanged,
}: {
  backend: LlmBackendInfo;
  isFirst: boolean;
  isLast: boolean;
  expanded: boolean;
  busy: string | null;
  provider?: LlmProviderInfo;
  onToggleExpand: () => void;
  onMove: (delta: number) => void;
  onSetEnabled: (enabled: boolean) => void;
  onSetModel: (tier: ModelTier, model: string) => void;
  onProviderChanged: (updated: LlmProviderInfo) => void;
}) {
  const anyBusy = busy !== null;
  const dim = !backend.enabled;

  return (
    <li className={"backend-row" + (dim ? " is-off" : "")}>
      <div className="backend-main">
        <div className="backend-rank" aria-hidden="true">
          {String(backend.rank).padStart(2, "0")}
        </div>

        <div className="backend-reorder">
          <button
            className="icon-btn"
            aria-label={`Move ${backend.display_name} up`}
            disabled={isFirst || anyBusy}
            onClick={() => onMove(-1)}
          >
            ↑
          </button>
          <button
            className="icon-btn"
            aria-label={`Move ${backend.display_name} down`}
            disabled={isLast || anyBusy}
            onClick={() => onMove(1)}
          >
            ↓
          </button>
        </div>

        <div className="backend-identity">
          <div className="backend-name">
            <BackendLogo backendId={backend.id} />
            {backend.display_name}
            <span className="pill muted">
              {backend.kind === "subscription" ? "subscription" : "API key"}
            </span>
            <StatusPill backend={backend} />
          </div>
          <p className="hint">
            {backend.kind === "subscription"
              ? `${backend.subscription_name} · ${backend.binary}`
              : backend.key_env_override
                ? "Key set by an environment variable"
                : backend.key_configured
                  ? "Key stored in your OS keychain"
                  : "No key yet"}
            {backend.remedy ? ` — ${backend.remedy}` : ""}
          </p>

          {/* The models this provider uses, readable without expanding —
              "which model does each provider run?" is the second question
              this page exists to answer, so it shouldn't need a click.
              On its own line under the name so a full default list (a
              vendor's own model names can run 60+ characters) has the
              row's width to wrap into instead of being squeezed beside
              the toggle and ellipsised. */}
          <p className="backend-models">
            {backend.models.map((m, i) => (
              <span key={m.tier}>
                {i > 0 ? <span className="sep"> · </span> : null}
                <span className={m.overridden ? "is-custom" : undefined}>
                  {m.model}
                </span>
              </span>
            ))}
          </p>
        </div>

        <label className="backend-toggle">
          <input
            type="checkbox"
            checked={backend.enabled}
            disabled={anyBusy}
            onChange={(e) => onSetEnabled(e.target.checked)}
          />
          <span>{backend.enabled ? "On" : "Off"}</span>
        </label>

        <button
          className="icon-btn"
          aria-expanded={expanded}
          aria-label={`${expanded ? "Hide" : "Show"} ${backend.display_name} models`}
          onClick={onToggleExpand}
        >
          {expanded ? "▴" : "▾"}
        </button>
      </div>

      {expanded ? (
        <div className="backend-detail">
          <div className="tier-grid">
            {MODEL_TIERS.map((tier) => {
              const m = backend.models.find((x) => x.tier === tier);
              if (!m) return null;
              return (
                <label key={tier} className="tier-field">
                  <span className="tier-label">
                    {tier}
                    {m.overridden ? <em> · custom</em> : null}
                  </span>
                  <input
                    type="text"
                    list={`${backend.id}-${tier}-models`}
                    defaultValue={m.model}
                    placeholder={m.default_model}
                    disabled={anyBusy}
                    aria-label={`${backend.display_name} model for ${tier} steps`}
                    onBlur={(e) => {
                      const next = e.target.value.trim();
                      if (next !== m.model) onSetModel(tier, next);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") e.currentTarget.blur();
                    }}
                  />
                  <datalist id={`${backend.id}-${tier}-models`}>
                    {backend.model_suggestions.map((s) => (
                      <option key={s} value={s} />
                    ))}
                  </datalist>
                  <span className="hint">{TIER_HELP[tier]}</span>
                </label>
              );
            })}
          </div>
          <p className="hint">
            Any model name works, not just the suggestions — leave a field
            empty to go back to Cori's default.
          </p>

          {backend.kind === "api" && provider ? (
            <div className="backend-key">
              <ProviderKeyForm provider={provider} onChanged={onProviderChanged} />
            </div>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

function StatusPill({ backend }: { backend: LlmBackendInfo }) {
  if (!backend.enabled) return <span className="pill muted">off</span>;
  switch (backend.status) {
    case "ready":
      return <span className="pill ok">ready</span>;
    case "signed_out":
      return <span className="pill warn">signed out</span>;
    case "no_key":
      return <span className="pill muted">no key</span>;
    default:
      return <span className="pill muted">not installed</span>;
  }
}

import { useCallback, useState } from "react";
import { BackendLogo } from "../components/provider-icons";
import { ProviderKeyForm } from "../components/provider-key-form";
import {
  getLlmSettings,
  isIpcError,
  listLlmProviders,
  MODEL_LEVELS,
  refreshLlmSettings,
  setLlmActiveBackend,
  setLlmLevelModel,
  type LlmBackendInfo,
  type LlmProviderInfo,
  type LlmSettings,
  type ModelLevel,
} from "../lib/api";

export function meta() {
  return [{ title: "AI Providers — Cori" }];
}

interface ProvidersData {
  settings: LlmSettings;
  providers: LlmProviderInfo[];
}

export async function clientLoader(): Promise<ProvidersData> {
  const [settings, providers] = await Promise.all([
    getLlmSettings(),
    listLlmProviders(),
  ]);
  return { settings, providers };
}

const LEVEL_HELP: Record<ModelLevel, string> = {
  low: "Fast extraction, classification, and short rewrites.",
  medium: "The default for everyday workflow steps.",
  high: "Complex reasoning and long synthesis.",
};

export default function Providers({ loaderData }: { loaderData: ProvidersData }) {
  const [settings, setSettings] = useState(loaderData.settings);
  const [providers, setProviders] = useState(loaderData.providers);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const apply = useCallback(
    async (key: string, action: () => Promise<LlmSettings>) => {
      setBusy(key);
      setError(null);
      try {
        setSettings(await action());
      } catch (cause) {
        setError(isIpcError(cause) ? cause.message : String(cause));
      } finally {
        setBusy(null);
      }
    },
    [],
  );

  const renderGroup = (kind: "subscription" | "api") =>
    settings.backends
      .filter((backend) => backend.kind === kind)
      .map((backend) => (
        <ProviderCard
          key={backend.id}
          backend={backend}
          provider={providers.find((provider) => provider.id === backend.id)}
          busy={busy}
          onActivate={() =>
            apply(`active:${backend.id}`, () =>
              setLlmActiveBackend({ backend: backend.id }),
            )
          }
          onDeactivate={() =>
            apply(`active:${backend.id}`, () => setLlmActiveBackend({}))
          }
          onRefresh={() => apply(`refresh:${backend.id}`, refreshLlmSettings)}
          onSetModel={(level, model) =>
            apply(`model:${backend.id}:${level}`, () =>
              setLlmLevelModel({ backend: backend.id, level, model }),
            )
          }
          onProviderChanged={(updated) => {
            setProviders((current) =>
              current.map((provider) =>
                provider.id === updated.id ? updated : provider,
              ),
            );
            void apply(`key:${backend.id}`, getLlmSettings);
          }}
        />
      ));

  return (
    <>
      <ActiveBanner settings={settings} />

      {error ? (
        <p className="hint" role="alert" style={{ color: "var(--red)" }}>
          {error}
        </p>
      ) : null}

      <div className="provider-groups">
        <ProviderGroup
          title="Subscriptions"
          hint="Use a plan you already have. Sign-in happens in the vendor CLI."
        >
          {renderGroup("subscription")}
        </ProviderGroup>

        <ProviderGroup
          title="API keys"
          hint="Use a metered provider key stored securely on this machine."
        >
          {renderGroup("api")}
        </ProviderGroup>
      </div>

      <p className="hint">
        Connections are kept when you switch. Cori uses only the provider you
        explicitly activate and never falls back to another one.
      </p>
    </>
  );
}

function ProviderGroup({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <section className="provider-group">
      <div className="section-head">
        <h2>{title}</h2>
        <p className="hint">{hint}</p>
      </div>
      <div className="backend-list">{children}</div>
    </section>
  );
}

function ActiveBanner({ settings }: { settings: LlmSettings }) {
  if (!settings.selected_backend) {
    return (
      <div className="active-banner is-blocked">
        <span className="pill muted">inactive</span>
        <div>
          <strong>No active AI provider</strong>
          <p className="hint">Choose a ready provider below to enable LLM steps.</p>
        </div>
      </div>
    );
  }

  if (!settings.active) {
    const selected = settings.backends.find((backend) => backend.active);
    return (
      <div className="active-banner is-blocked">
        <span className="pill warn">needs attention</span>
        {selected ? <BackendLogo backendId={selected.id} size={16} /> : null}
        <div>
          <strong>{selected?.display_name ?? settings.selected_backend}</strong>
          <p className="hint">{settings.blocked_reason}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="active-banner">
      <span className="pill ok">active</span>
      <BackendLogo backendId={settings.active.backend_id} size={16} />
      <div>
        <strong>{settings.active.display_name}</strong>
        <p className="hint">
          Medium workflow steps use <code>{settings.active.model}</code>.
        </p>
      </div>
    </div>
  );
}

function ProviderCard({
  backend,
  provider,
  busy,
  onActivate,
  onDeactivate,
  onRefresh,
  onSetModel,
  onProviderChanged,
}: {
  backend: LlmBackendInfo;
  provider?: LlmProviderInfo;
  busy: string | null;
  onActivate: () => void;
  onDeactivate: () => void;
  onRefresh: () => void;
  onSetModel: (level: ModelLevel, model: string) => void;
  onProviderChanged: (provider: LlmProviderInfo) => void;
}) {
  const ready = backend.status === "ready";
  const anyBusy = busy !== null;

  return (
    <article className={`backend-row${backend.active ? " is-active" : ""}`}>
      <div className="backend-main provider-card-main">
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
                  ? "Key stored securely"
                  : "No API key connected"}
          </p>
        </div>

        <div className="provider-card-action">
          {backend.active ? (
            <>
              <span className="pill ok">Active</span>
              <button className="btn subtle" disabled={anyBusy} onClick={onDeactivate}>
                Deactivate
              </button>
            </>
          ) : ready ? (
            <button className="btn primary" disabled={anyBusy} onClick={onActivate}>
              {busy === `active:${backend.id}` ? "Activating…" : "Use this provider"}
            </button>
          ) : null}
        </div>
      </div>

      {!ready && backend.kind === "subscription" ? (
        <div className="provider-remedy">
          <p>{backend.remedy}</p>
          {backend.login_command ? (
            <div className="command-copy-row">
              <code>{backend.login_command}</code>
              <button
                className="btn subtle"
                onClick={() => void navigator.clipboard.writeText(backend.login_command ?? "")}
              >
                Copy
              </button>
              <button className="btn" disabled={anyBusy} onClick={onRefresh}>
                {busy === `refresh:${backend.id}` ? "Checking…" : "Re-check"}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {backend.kind === "api" && provider ? (
        <div className="backend-key">
          <ProviderKeyForm provider={provider} onChanged={onProviderChanged} />
        </div>
      ) : null}

      {backend.active ? (
        <details className="advanced-models">
          <summary>Advanced models</summary>
          <div className="tier-grid">
            {MODEL_LEVELS.map((level) => {
              const mapping = backend.models.find((item) => item.level === level);
              if (!mapping) return null;
              return (
                <label key={level} className="tier-field">
                  <span className="tier-label">
                    {level}
                    {mapping.overridden ? <em> · custom</em> : null}
                  </span>
                  <div className="model-input-row">
                    <input
                      key={mapping.model}
                      type="text"
                      list={`${backend.id}-${level}-models`}
                      defaultValue={mapping.overridden ? mapping.model : ""}
                      placeholder={mapping.default_model}
                      disabled={anyBusy}
                      aria-label={`${backend.display_name} model for ${level} steps`}
                      onBlur={(event) => {
                        const next = event.currentTarget.value.trim();
                        if (next !== (mapping.overridden ? mapping.model : "")) {
                          onSetModel(level, next);
                        }
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") event.currentTarget.blur();
                      }}
                    />
                    {mapping.overridden ? (
                      <button
                        type="button"
                        className="btn subtle"
                        disabled={anyBusy}
                        onClick={() => onSetModel(level, "")}
                      >
                        Reset
                      </button>
                    ) : null}
                  </div>
                  <datalist id={`${backend.id}-${level}-models`}>
                    {backend.model_suggestions.map((suggestion) => (
                      <option key={suggestion} value={suggestion} />
                    ))}
                  </datalist>
                  <span className="hint">{LEVEL_HELP[level]}</span>
                </label>
              );
            })}
          </div>
          <p className="hint">
            Any model name is accepted. Empty fields use Cori’s provider defaults.
          </p>
        </details>
      ) : null}
    </article>
  );
}

function StatusPill({ backend }: { backend: LlmBackendInfo }) {
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

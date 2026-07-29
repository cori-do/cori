// Paste-an-API-key form for one LLM provider. Used by the Providers
// manage tab and inline in the launch window when a workflow needs a
// provider that has no key yet. The key is write-only: it goes to the
// Rust core (→ OS keychain) and never comes back to the UI.

import { useState } from "react";
import {
  isIpcError,
  removeLlmProviderKey,
  setLlmProviderKey,
  type LlmProviderInfo,
} from "../lib/api";

const KEY_PLACEHOLDERS: Record<string, string> = {
  openai: "sk-…",
  anthropic: "sk-ant-…",
  gemini: "AIza…",
};

const KEY_CONSOLE_URLS: Record<string, string> = {
  openai: "https://platform.openai.com/api-keys",
  anthropic: "https://console.anthropic.com/settings/keys",
  gemini: "https://aistudio.google.com/app/apikey",
};

export function ProviderKeyForm({
  provider,
  onChanged,
}: {
  provider: LlmProviderInfo;
  /** Fired after a successful save or remove, with the fresh state. */
  onChanged: (updated: LlmProviderInfo) => void;
}) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState<"save" | "remove" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Replace mode: provider is configured but the user wants a new key.
  const [replacing, setReplacing] = useState(false);

  const showForm = !provider.configured || replacing;

  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (!key.trim()) return;
    setBusy("save");
    setError(null);
    try {
      const updated = await setLlmProviderKey({
        provider: provider.id,
        api_key: key,
      });
      setKey("");
      setReplacing(false);
      onChanged(updated);
    } catch (err) {
      setError(isIpcError(err) ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function remove() {
    setBusy("remove");
    setError(null);
    try {
      const updated = await removeLlmProviderKey({ provider: provider.id });
      onChanged(updated);
    } catch (err) {
      setError(isIpcError(err) ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div>
      {showForm ? (
        <form onSubmit={save} style={{ display: "flex", gap: 8 }}>
          <input
            type="password"
            autoComplete="off"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={KEY_PLACEHOLDERS[provider.id] ?? "API key"}
            aria-label={`${provider.display_name} API key`}
            style={{
              flex: 1,
              padding: "6px 10px",
              border: "1px solid var(--rule)",
              borderRadius: 6,
              fontFamily: "var(--font-mono)",
              fontSize: 13,
            }}
          />
          <button
            type="submit"
            className="btn primary"
            disabled={busy !== null || !key.trim()}
          >
            {busy === "save" ? "Verifying…" : "Save"}
          </button>
          {replacing && (
            <button
              type="button"
              className="btn"
              disabled={busy !== null}
              onClick={() => {
                setReplacing(false);
                setKey("");
                setError(null);
              }}
            >
              Cancel
            </button>
          )}
        </form>
      ) : (
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span className="pill ok">configured</span>
          <span style={{ flex: 1 }} />
          <button
            className="btn"
            disabled={busy !== null}
            onClick={() => setReplacing(true)}
          >
            Replace
          </button>
          <button className="btn" disabled={busy !== null} onClick={remove}>
            {busy === "remove" ? "Removing…" : "Remove"}
          </button>
        </div>
      )}
      <p className="hint" style={{ margin: "6px 0 0" }}>
        {provider.keychain
          ? "Stored in your OS keychain — shared with the cori CLI."
          : "No OS keychain on this machine — stored in ~/.cori/credentials (file mode 0600)."}
        {KEY_CONSOLE_URLS[provider.id] && showForm ? (
          <>
            {" "}
            Get a key at{" "}
            <a href={KEY_CONSOLE_URLS[provider.id]} target="_blank" rel="noreferrer">
              {new URL(KEY_CONSOLE_URLS[provider.id]).host}
            </a>
            .
          </>
        ) : null}
      </p>
      {provider.env_override && (
        <p className="hint" style={{ margin: "6px 0 0", color: "var(--amber)" }}>
          An environment variable for {provider.display_name} is set and
          overrides the stored key at run time.
        </p>
      )}
      {error && (
        <p className="hint" style={{ margin: "6px 0 0", color: "var(--red)" }}>
          {error}
        </p>
      )}
    </div>
  );
}

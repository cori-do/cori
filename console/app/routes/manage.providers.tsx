import { useState } from "react";
import { ProviderKeyForm } from "../components/provider-key-form";
import { listLlmProviders, type LlmProviderInfo } from "../lib/api";

export function meta() {
  return [{ title: "AI Providers — Cori" }];
}

export async function clientLoader(): Promise<LlmProviderInfo[]> {
  return listLlmProviders();
}

export default function Providers({
  loaderData,
}: {
  loaderData: LlmProviderInfo[];
}) {
  const [providers, setProviders] = useState<LlmProviderInfo[]>(loaderData);

  return (
    <>
      <p className="hint" style={{ marginTop: 0 }}>
        API keys for the model providers your workflows' LLM steps use.
        Keys are verified against the provider, then stored in your OS
        keychain — the <code>cori</code> CLI reads the same entries, so a
        key saved here works everywhere (and <code>cori login</code> shows
        up here).
      </p>

      {providers.map((p) => (
        <div className="card" key={p.id}>
          <h3 style={{ margin: "0 0 8px" }}>{p.display_name}</h3>
          <ProviderKeyForm
            provider={p}
            onChanged={(updated) =>
              setProviders((ps) =>
                ps.map((x) => (x.id === updated.id ? updated : x)),
              )
            }
          />
        </div>
      ))}
    </>
  );
}

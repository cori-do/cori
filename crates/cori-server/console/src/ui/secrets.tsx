import { useState } from "react";
import { KeyRound } from "lucide-react";
import { ago } from "../lib/format";
import { href } from "../lib/router";
import type { Secret } from "../lib/types";
import { Badge, Button, Empty, Field, Input, List, Time } from "./core";
import { Modal } from "./dialog";
import { keyLabel } from "./identity";

// Credentials: names and what seals them, never values. A run looks an app's up first, then the deployment's.

export function SecretsList({ secrets, showApp, empty }: { secrets: Secret[]; showApp?: boolean; empty: React.ReactNode }) {
  if (!secrets.length) return <Empty compact icon={<KeyRound />} title="No credential yet">{empty}</Empty>;
  return (
    <List>
      {secrets.map((s) => {
        const k = keyLabel(s.key_by);
        return (
          <div key={`${s.app || ""}/${s.name}`} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3">
            <div className="min-w-0 flex-1">
              <code className="font-medium text-fg">{s.name}</code>
              <div className="text-[13px] text-fg-3">
                {s.kind === "store_login" ? "Database login" : s.kind === "generic" || !s.kind ? "Secret" : s.kind}
                {showApp && <> · {s.app ? <a href={href("app", s.app, "settings")} className="link">{s.app}</a> : "every app"}</>}
                {" · "}version {s.version} · updated <Time at={s.updated_at}>{ago(s.updated_at)}</Time>
              </div>
            </div>
            <Badge tone={s.key_by && s.key_by !== "master" ? "brand" : "neutral"} title={k.title}>{k.label}</Badge>
          </div>
        );
      })}
    </List>
  );
}

/** A credential's name and value; the value is sealed and never shown again. */
export function SecretForm({ scope, close }: { scope: string; close: (v: { name: string; value: string } | null) => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const valid = /^[A-Za-z0-9_.-]+$/.test(name.trim()) && !!value;
  return (
    <Modal title="Add a credential" description={`For ${scope}. Setting a name again makes a new version.`} onClose={() => close(null)} onSubmit={() => valid && close({ name: name.trim(), value })}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary" disabled={!valid}>Save</Button></>}>
      <Field label="Name" hint="The name a connector's secret, a webhook or a schedule refers to.">
        <Input className="font-mono" autoFocus value={name} placeholder="crm.token" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field label="Value" hint="Sealed, never shown again. A run gets it for one call, when it needs it.">
        <Input className="font-mono" type="password" autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} />
      </Field>
    </Modal>
  );
}

import { useState } from "react";
import { devToken, getToken, MOCK, setToken } from "../lib/api";
import { Button, cx, Field, Input } from "../ui/core";
import { Modal } from "../ui/dialog";

// Dev auth only: act as someone else to see the console as they do. A dev token is `dev:<user>:<role,role>`; the server
// reads the roles as given (platform roles such as it_admin or builder, and app roles such as manager).

const PRESETS: { token: string; name: string; description: string }[] = [
  { token: "dev:admin:it_admin", name: "Admin", description: "People, settings and every app" },
  { token: "dev:bo:builder", name: "Builder", description: "Builds apps: releases, previews and their data" },
  { token: "dev:sam:manager", name: "Manager", description: "Uses Team Requests and decides its requests" },
  { token: "dev:alice:member", name: "Member", description: "Uses the apps their roles open" },
  ...(MOCK ? [{ token: "dev:pat:guest,member", name: "Guest", description: "Outside the organization: one app shared with them" }] : []),
];

export function DevIdentity({ close }: { close: () => void }) {
  const current = getToken();
  const [custom, setCustom] = useState(current);
  const parsed = devToken(custom);
  const use = (t: string) => { if (t !== getToken()) setToken(t); close(); };
  return (
    <Modal title="View as" description="Dev mode: every call is made with the token you choose here." onClose={close} onSubmit={() => parsed && use(custom)}
      footer={<><Button variant="ghost" onClick={close}>Cancel</Button><Button type="submit" variant="primary" disabled={!parsed}>Use this token</Button></>}>
      <div className="grid gap-2">
        {PRESETS.map((p) => (
          <button key={p.token} type="button" onClick={() => use(p.token)}
            className={cx("flex cursor-pointer items-start gap-3 rounded-xl p-3 text-left ring-1 ring-inset transition-colors",
              p.token === current ? "bg-brand-50/60 ring-brand-500 dark:bg-brand-500/10" : "ring-line hover:bg-surface-2")}>
            <span className="min-w-0 flex-1">
              <span className="block font-medium text-fg">{p.name}</span>
              <span className="block text-[13px] text-fg-3">{p.description}</span>
            </span>
            <code className="mt-0.5 shrink-0 text-xs text-fg-3">{p.token}</code>
          </button>
        ))}
      </div>
      <Field label="Or a token of your own" hint="dev:<user>:<role,role> — for example dev:alice:support_agent" error={custom && !parsed ? "Not a dev token" : undefined}>
        <Input className="font-mono text-[13px]" value={custom} spellCheck={false} onChange={(e) => setCustom(e.target.value)} />
      </Field>
    </Modal>
  );
}

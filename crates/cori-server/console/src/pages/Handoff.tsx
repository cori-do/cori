import { useEffect, useState } from "react";
import { api, authMode, errMsg, post, signInTo } from "../lib/api";
import { brand } from "../lib/brand";
import { href } from "../lib/router";
import type { Me } from "../lib/types";
import { BrandLogo } from "../shell/Shell";
import { Card, Spinner } from "../ui/core";

// A release's own host (`<app>--v<n>`) and the organization's apps portal do not sign their people in with the identity
// provider: they ask the console who is signed in, at `#/preview/<app>/<release>?…` and `#/portal?…` (`redirect_uri`,
// `code_challenge`, `state`, and the `organization` they belong to). The console gets a code for that host from the server
// (which checks that the person may open it and that the callback is that host's), then sends the browser back there with
// the code; the page trades it for a session of that host's own. A front host whose files are for the people who may open
// it asks the same way for a pass, at `#/pass/<app>[/<release>]?…`: the host trades the code for a cookie of its own.
// Nothing of the console shows around it.

/** Whether the address is a hand-off: the console's own pages are not drawn then. */
export const isHandoff = (hash = location.hash) => /^#\/((preview|pass)\/[^?]+|portal)(\?.*)?$/.test(hash);

export function Handoff() {
  const kind = location.hash.startsWith("#/portal") ? "portal" : location.hash.startsWith("#/pass/") ? "pass" : "preview";
  const portal = kind === "portal";
  const [app = "", release = ""] = location.hash.replace(/^#\/(preview|pass)\//, "").split("?")[0].split("/").map((p) => { try { return decodeURIComponent(p); } catch { return p; } });
  const what = portal ? "your apps" : kind === "pass" && !release ? app : `${release} of ${app}`;
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    document.title = `Opening ${what} — ${brand.name}`;
    const ask = portal ? "/v1/portal-codes"
      : kind === "pass" ? `/v1/apps/${encodeURIComponent(app)}/pass-codes`
      : `/v1/apps/${encodeURIComponent(app)}/versions/${encodeURIComponent(release)}/preview-codes`;
    const extra: Record<string, string> = kind === "pass" && release ? { release } : {};
    handoff(ask, portal ? "the apps portal" : kind === "pass" && !release ? "this app" : "this release", extra).catch((e) => setError(errMsg(e)));
  }, [kind, portal, app, release, what]);
  return (
    <main className="flex min-h-dvh items-center justify-center px-4">
      <div className="w-full max-w-sm">
        <div className="mb-6 flex justify-center"><BrandLogo /></div>
        <Card className="px-6 py-5">
          {error ? (
            <div className="space-y-3">
              <h1 className="text-base font-semibold">{portal ? "Your apps cannot open" : `${what} cannot open`}</h1>
              <p className="text-[14px] text-fg-2">{error}</p>
              <p className="text-[14px]"><a href={href()} className="link">Open the console</a></p>
            </div>
          ) : <Spinner className="justify-center py-2" label={`Opening ${what}`} />}
        </Card>
      </div>
    </main>
  );
}

const tried = {
  get: () => { try { return sessionStorage.getItem("cori.handoff"); } catch { return null; } },
  set: (v: string | null) => { try { if (v === null) sessionStorage.removeItem("cori.handoff"); else sessionStorage.setItem("cori.handoff", v); } catch { /* storage blocked */ } },
};

async function handoff(ask: string, what: string, extra: Record<string, string> = {}) {
  const q = new URLSearchParams(location.hash.split("?")[1] ?? "");
  const want = q.get("organization");
  if (authMode() === "builtin" && want) {
    const me = await api<Me>("/v1/me");
    if (me.organization?.slug && me.organization.slug !== want) {
      // once: someone outside that organization comes back signed in to another, and is told so
      if (tried.get() !== want) { tried.set(want); await signInTo(want); return; }
      tried.set(null);
      throw new Error(`You are signed in to ${me.organization.slug}, and ${what} is ${want}'s: switch organization in the console, or ask ${want}'s admins for access.`);
    }
  }
  tried.set(null);
  const out = await post<{ code: string; redirect_uri: string }>(ask, { code_challenge: q.get("code_challenge") ?? "", redirect_uri: q.get("redirect_uri") ?? "", ...extra });
  // the callback the server checked, not the one the link named
  const to = new URL(out.redirect_uri);
  to.searchParams.set("code", out.code);
  const state = q.get("state");
  if (state) to.searchParams.set("state", state);
  location.replace(to.toString());
}

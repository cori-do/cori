import { useSyncExternalStore } from "react";

// Hash routes `#/<part>/<part>…`. The canonical ones:
//   #/                                   Home
//   #/inbox[/<app>[/<item>]]             what waits for a decision, all of it or one app's
//   #/app/<app>[/<tab>…]                 an app: releases[/<release>], runs[/<run>], data[/<target>/<store>/<table>], access, settings
//   #/settings[/<section>]               the organization: general, members, models, connectors, credentials, system, audit
//   #/connect                            a coding agent and the command line
//   #/connections                        the accounts a person connected (Google), which their runs act with
//   #/switch/<slug>                      sign in again to another organization (an accepted invitation's link)
//   #/preview/<app>/v<n>?…, #/portal?…   a release's host or the apps portal signing its person in (pages/Handoff.tsx)
// Slack and email notifications, cori-auth, the cli and the API's `console_url` link to the previous console's routes
// (`#/app/<app>`, `#/run/<app>/<run>`, `#/approvals/<app>`, `#/versions/<app>`, `#/people`…): `legacy` maps every one.

const read = () => location.hash;
const subscribe = (l: () => void) => { window.addEventListener("hashchange", l); return () => window.removeEventListener("hashchange", l); };

// a page's own query (`#/portal?…`, `#/preview/<app>/v<n>?…`) is not part of its parts
const split = (hash: string) => hash.replace(/^#\/?/, "").split("?")[0].split("/").filter(Boolean).map((p) => { try { return decodeURIComponent(p); } catch { return p; } });

/** The current route's parts, decoded. */
export function useRoute(): string[] {
  return split(useSyncExternalStore(subscribe, read));
}

/** `#/<parts…>`, each part encoded, empty parts skipped. */
export const href = (...parts: (string | undefined | null | false)[]) =>
  `#/${parts.filter((p): p is string => !!p).map(encodeURIComponent).join("/")}`;

export function navigate(to: string, opts: { replace?: boolean } = {}) {
  if (opts.replace) {
    history.replaceState(null, "", `${location.pathname}${location.search}${to}`);
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  } else location.hash = to;
}

/** Where a route of the previous console lives now; null when it is a canonical route already. */
export function legacy(parts: string[]): string | null {
  const [view, a, ...rest] = parts;
  switch (view) {
    case "home": case "build": case "start": return href();
    case "versions": return a ? href("app", a, "releases") : href();
    case "runs": return a ? href("app", a, "runs") : href();
    case "run": return a ? href("app", a, "runs", rest.join("/")) : href();
    case "data": return a ? href("app", a, "data", ...rest) : href();
    case "approvals": return href("inbox", a);
    case "people": return href("settings", "members");
    case "organization": return href("settings", "general");
    case "operate": return href("settings", "system");
    case "llm": return href("settings", "models");
    case "audit": return href("settings", "audit");
    default: return null;
  }
}

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowUpRight, Building2, Hammer, LayoutGrid, LogOut, Search, UserRound } from "lucide-react";
import { oidcSession, type OidcSession } from "@cori-do/client";
import { brand } from "../lib/brand";
import { Avatar, Badge, Button, Card, cx, Spinner } from "../ui/core";
import { Menu } from "../ui/menu";

// The organization's apps portal: where its people find the apps they may use (`GET /v1/me/apps`), each opened on its own
// host. It is served at the root of the portal's own host and signs its person in there: the host is its own issuer, which
// asks the console who is signed in (cori-server's `api::sessions`), and the session reaches the person and their apps only.

type App = {
  id: string; name: string; description?: string | null; icon?: string | null; front_url?: string | null;
  roles: string[]; build: boolean; waiting_for_me: number; promoted_release?: string | null;
};
type Me = {
  sub: string; email?: string | null; guest?: boolean; console_url?: string;
  organization?: { slug?: string | null; name?: string | null };
  can?: { build?: boolean; admin?: boolean };
};
type AuthConfig = { mode?: "builtin" | "dev" | "off"; account_url?: string };
type Loaded = { me: Me; apps: App[]; config: AuthConfig };

const origin = location.origin;
let session: OidcSession | null = null;
let devToken = "";

/** The bearer the portal calls with: its own session (builtin auth; a sign-in through the console when it has none), a dev
 *  token, or none. Null when nobody is signed in and no sign-in may start now (one was just given up, or signed out). */
async function signIn(config: AuthConfig): Promise<string | null> {
  if (config.mode === "builtin") {
    session = oidcSession({ issuer: `${origin}/__cori/oidc`, client_id: "portal", resource: origin, redirect_uri: `${origin}/` });
    await session.completeLogin().catch(() => false);
    const t = await session.token().catch(() => null);
    if (!t && session.mayAutoLogin()) await session.login();
    return t;
  }
  if (config.mode === "off") return "";
  devToken = (() => { try { return localStorage.getItem("cori.tok") || "dev:admin:it_admin"; } catch { return "dev:admin:it_admin"; } })();
  return devToken;
}

async function call<T>(path: string): Promise<T> {
  const t = session ? await session.token() : devToken;
  const r = await fetch(path, { headers: t ? { authorization: `Bearer ${t}` } : {} });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || r.statusText);
  return j as T;
}

// the apps opened last from this browser come first (this browser only)
const RECENT = "cori.portal.recent";
const recent = (): string[] => { try { return JSON.parse(localStorage.getItem(RECENT) || "[]"); } catch { return []; } };
const opened = (id: string) => { try { localStorage.setItem(RECENT, JSON.stringify([id, ...recent().filter((r) => r !== id)].slice(0, 12))); } catch { /* private mode */ } };

export function Portal() {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [failure, setFailure] = useState<{ message: string; signIn?: boolean } | null>(null);
  const [filter, setFilter] = useState("");
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    (async () => {
      const config: AuthConfig = await fetch("/v1/auth/config").then((r) => r.json()).catch(() => ({}));
      if ((await signIn(config)) === null) { setFailure({ message: "You are signed out.", signIn: true }); return; }
      const [me, apps] = await Promise.all([call<Me>("/v1/me"), call<App[]>("/v1/me/apps")]);
      setLoaded({ me, apps, config });
    })().catch((e) => setFailure({ message: e instanceof Error ? e.message : String(e), signIn: !!session }));
  }, []);

  // `/` searches, as in the console
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "/" && document.activeElement !== search.current) { e.preventDefault(); search.current?.focus(); } };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, []);

  const org = loaded?.me.organization?.name || loaded?.me.organization?.slug || "";
  useEffect(() => { document.title = `${org ? `${org} · ` : ""}${brand.name} apps`; }, [org]);

  return (
    <div className="flex min-h-dvh flex-col">
      <header className="sticky top-0 z-30 border-b border-line bg-canvas/90 backdrop-blur">
        <div className="mx-auto flex h-14 max-w-6xl items-center justify-between gap-3 px-4 sm:px-6">
          <div className="flex min-w-0 items-center gap-2.5">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface shadow-xs ring-1 ring-line"><img src={brand.mark} alt="" className="size-5" /></span>
            <span className="truncate text-[15px] font-semibold">{org || brand.name}</span>
            <span className="hidden text-fg-4 sm:inline">apps</span>
          </div>
          {loaded && <Person me={loaded.me} config={loaded.config} />}
        </div>
      </header>
      <main className="mx-auto w-full max-w-6xl flex-1 px-4 py-8 sm:px-6">
        {failure ? <Failure {...failure} />
          : !loaded ? <Spinner label="Opening your apps" />
          : <Apps apps={loaded.apps} me={loaded.me} filter={filter} setFilter={setFilter} search={search} />}
      </main>
      {brand.custom && (
        <footer className="flex items-center justify-center gap-1.5 px-4 py-6 text-xs text-fg-4">
          <img src={brand.coriMark} alt="" className="size-3.5 opacity-60 grayscale dark:invert" />
          Powered by Cori
        </footer>
      )}
    </div>
  );
}

function Apps({ apps, me, filter, setFilter, search }: { apps: App[]; me: Me; filter: string; setFilter: (f: string) => void; search: React.RefObject<HTMLInputElement | null> }) {
  const order = useMemo(recent, []);
  const q = filter.trim().toLowerCase();
  const shown = apps.filter((a) => !q || a.name.toLowerCase().includes(q) || a.id.includes(q) || (a.description || "").toLowerCase().includes(q));
  const rank = (a: App) => { const i = order.indexOf(a.id); return i < 0 ? Number.MAX_SAFE_INTEGER : i; };
  const all = [...shown].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
  const waiting = all.filter((a) => a.waiting_for_me > 0 && a.front_url);
  const builds = me.can?.build || me.can?.admin;
  if (apps.length === 0) {
    return (
      <div className="mx-auto max-w-md py-16 text-center">
        <div className="mx-auto flex size-12 items-center justify-center rounded-full bg-surface-2 text-fg-3"><LayoutGrid className="size-6" aria-hidden /></div>
        <h1 className="mt-4 text-lg font-semibold">No app yet</h1>
        <p className="mt-2 text-fg-3">Apps show here once someone gives you a role in one.</p>
        {builds && me.console_url && <p className="mt-4"><a href={me.console_url} className="link">Build one in the console</a></p>}
      </div>
    );
  }
  return (
    <div className="space-y-8">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <h1 className="text-[22px] leading-tight font-semibold tracking-tight">Your apps</h1>
        {apps.length > 8 && (
          <label className="relative block w-full sm:w-72">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
            <input ref={search} value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Find an app  /" aria-label="Find an app"
              className="h-9 w-full rounded-lg border-0 bg-surface pr-3 pl-9 text-[14px] text-fg shadow-xs ring-1 ring-line-2 ring-inset outline-none placeholder:text-fg-4 focus:ring-2 focus:ring-brand-500" />
          </label>
        )}
      </div>
      {waiting.length > 0 && <Group title="Waiting for you">{waiting.map((a) => <AppCard key={a.id} app={a} consoleUrl={me.console_url} />)}</Group>}
      <Group title={waiting.length > 0 ? "All apps" : undefined}>
        {all.map((a) => <AppCard key={a.id} app={a} consoleUrl={me.console_url} />)}
        {all.length === 0 && <p className="text-fg-3">No app matches “{filter}”.</p>}
      </Group>
    </div>
  );
}

function Group({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section>
      {title && <h2 className="mb-3 text-[15px] font-semibold text-fg">{title}</h2>}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">{children}</div>
    </section>
  );
}

/** One app: a link to its front, or, for its builders, to its page in the console until a release is live. */
function AppCard({ app, consoleUrl }: { app: App; consoleUrl?: string }) {
  const href = app.front_url || (consoleUrl ? `${consoleUrl}#/app/${encodeURIComponent(app.id)}` : undefined);
  return (
    <a href={href} onClick={() => opened(app.id)}
      className="group flex flex-col rounded-xl bg-surface p-4 shadow-xs ring-1 ring-line transition-shadow hover:shadow-md hover:ring-line-2 focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:outline-none">
      <div className="flex items-start gap-3">
        <AppIcon app={app} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate font-semibold text-fg">{app.name}</span>
            <ArrowUpRight className="size-3.5 shrink-0 text-fg-4 transition-colors group-hover:text-brand-500" aria-hidden />
          </div>
          <p className="mt-0.5 line-clamp-2 text-[13px] text-fg-3">{app.description || (app.front_url ? " " : "Not live yet: its builders open it in the console")}</p>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        {app.waiting_for_me > 0 && <Badge tone="warning">{app.waiting_for_me} waiting for you</Badge>}
        {app.roles.map((r) => <Badge key={r}>{r}</Badge>)}
        {app.build && <Badge tone="brand" title="you build this app"><Hammer className="size-3" aria-hidden />builder</Badge>}
      </div>
    </a>
  );
}

/** The live release's favicon (`/__cori/icons/<app>`), else the app's initial on a colour drawn from its id. */
function AppIcon({ app }: { app: App }) {
  const [failed, setFailed] = useState(!app.icon);
  const hue = [...app.id].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7);
  if (!failed && app.icon) return <img src={app.icon} alt="" onError={() => setFailed(true)} className="size-10 shrink-0 rounded-xl bg-surface-2 object-contain p-1 ring-1 ring-line" />;
  return (
    <span aria-hidden className="flex size-10 shrink-0 items-center justify-center rounded-xl text-base font-semibold text-white" style={{ background: `hsl(${hue} 55% 48%)` }}>
      {(app.name || app.id).trim().charAt(0).toUpperCase()}
    </span>
  );
}

function Person({ me, config }: { me: Me; config: AuthConfig }) {
  const who = me.email || me.sub;
  return (
    <Menu width="w-56" items={[
      { heading: <span className="block truncate">{who}</span> },
      (me.can?.build || me.can?.admin) && me.console_url && { label: "Console", icon: <Hammer />, href: me.console_url },
      config.account_url && { label: "Your account", icon: <UserRound />, href: config.account_url, external: true },
      me.organization && me.console_url && { label: "Switch organization", icon: <Building2 />, href: `${me.console_url}#/switch` },
      session && "sep",
      session && { label: "Sign out", icon: <LogOut />, onSelect: () => { void session?.logout(); } },
    ]}>
      {(p) => (
        <button {...p} type="button" className="flex cursor-pointer items-center gap-2 rounded-full py-1 pr-2.5 pl-1 transition-colors hover:bg-surface-2">
          <Avatar name={who} size="sm" />
          <span className="hidden max-w-48 truncate text-[13.5px] sm:inline">{who}</span>
        </button>
      )}
    </Menu>
  );
}

function Failure({ message, signIn }: { message: string; signIn?: boolean }) {
  return (
    <Card className={cx("mx-auto max-w-md px-6 py-5")}>
      <p className="text-fg-2">{message}</p>
      {signIn && session && <div className="mt-4"><Button variant="primary" onClick={() => session?.login()}>Sign in</Button></div>}
    </Card>
  );
}

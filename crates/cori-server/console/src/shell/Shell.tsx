import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeftRight, ChevronsUpDown, DoorOpen, ExternalLink, Home, Inbox, LayoutGrid, Link2, LogIn, LogOut, Menu as MenuIcon, Monitor, Moon, Plus, Search, Settings, Sun, Terminal, UserRound, X } from "lucide-react";
import { api, authConfig, authMode, signIn, signOut, switchOrganization, useToken } from "../lib/api";
import { useWaitingIn } from "../lib/decisions";
import { personOf, usePeople } from "../lib/people";
import { brand } from "../lib/brand";
import { useQuery } from "../lib/query";
import { href, useRoute } from "../lib/router";
import { useSession } from "../lib/session";
import { setTheme, useTheme } from "../lib/theme";
import type { MyApp, MyOrganization, MyOrganizations } from "../lib/types";
import { leaveOrganization } from "../ui/access";
import { Avatar, cx, Kbd } from "../ui/core";
import { openDialog } from "../ui/dialog";
import { Menu } from "../ui/menu";
import { openPalette } from "./CommandPalette";
import { DevIdentity } from "./DevIdentity";
import { settingsSections } from "./nav";

// The frame of every page: a sidebar with the few places there are (Home, Inbox, your apps, Settings) on a wide screen,
// a top bar with the same in a drawer on a phone.

export function Shell({ children, wide }: { children: ReactNode; wide?: boolean }) {
  const [open, setOpen] = useState(false);
  const route = useRoute();
  useEffect(() => setOpen(false), [route.join("/")]);
  return (
    <div className="min-h-dvh lg:grid lg:grid-cols-[256px_minmax(0,1fr)]">
      <button type="button" onClick={() => document.getElementById("main")?.focus()}
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-[70] focus:rounded-lg focus:bg-surface focus:px-3 focus:py-2 focus:shadow-lg focus:ring-2 focus:ring-brand-500">
        Skip to content
      </button>
      <aside className="sticky top-0 hidden h-dvh border-r border-line lg:block"><Sidebar /></aside>
      <MobileBar onMenu={() => setOpen(true)} />
      {open && <MobileNav onClose={() => setOpen(false)} />}
      <main id="main" tabIndex={-1} className={cx("w-full min-w-0 outline-none", wide ? "px-4 pt-5 pb-4 sm:px-6" : "mx-auto max-w-[1120px] px-4 pt-6 pb-16 sm:px-8 sm:pt-9")}>
        {children}
      </main>
      {/* on a phone, where the sidebar is a drawer, every page ends with the line the sidebar ends with */}
      <PoweredBy as="footer" className="justify-center px-4 pb-6 lg:hidden" />
    </div>
  );
}

function MobileBar({ onMenu }: { onMenu: () => void }) {
  const { waiting } = useSession();
  return (
    <div className="sticky top-0 z-30 flex h-13 items-center gap-2 border-b border-line bg-canvas/90 px-3 backdrop-blur lg:hidden">
      <button type="button" onClick={onMenu} aria-label="Open navigation" className="relative flex size-9 cursor-pointer items-center justify-center rounded-lg text-fg-2 hover:bg-surface-2">
        <MenuIcon className="size-5" />
        {waiting > 0 && <span className="absolute top-1.5 right-1.5 size-2 rounded-full bg-amber-500 ring-2 ring-canvas" aria-label={`${waiting} waiting`} />}
      </button>
      <a href={href()} className="flex items-center" aria-label={`${brand.name} home`}><BrandLogo /></a>
      <button type="button" onClick={openPalette} aria-label="Search" className="ml-auto flex size-9 cursor-pointer items-center justify-center rounded-lg text-fg-2 hover:bg-surface-2"><Search className="size-5" /></button>
    </div>
  );
}

function MobileNav({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { ref.current?.showModal(); }, []);
  return (
    <dialog ref={ref} onCancel={(e) => { e.preventDefault(); onClose(); }} onClick={(e) => { if (e.target === ref.current) onClose(); }}
      className="fixed inset-y-0 left-0 m-0 h-dvh max-h-dvh w-[84vw] max-w-[300px] bg-canvas p-0 text-fg shadow-2xl backdrop:animate-fade">
      <div className="relative h-full">
        <button type="button" onClick={onClose} aria-label="Close navigation" className="absolute top-4 right-3 z-10 flex size-8 cursor-pointer items-center justify-center rounded-lg text-fg-3 hover:bg-surface-2"><X className="size-4" /></button>
        <Sidebar inDrawer />
      </div>
    </dialog>
  );
}

export function BrandLogo({ className }: { className?: string }) {
  return (
    <>
      <img src={brand.logo} alt={brand.name} className={cx("h-6 w-auto dark:hidden", className)} />
      <img src={brand.logoDark} alt={brand.name} className={cx("hidden h-6 w-auto dark:block", className)} />
    </>
  );
}

/** Under another brand, the console says what it runs on; no brand setting removes it. */
export function PoweredBy({ as: As = "p", className }: { as?: "p" | "footer"; className?: string }) {
  if (!brand.custom) return null;
  return (
    <As className={cx("flex items-center gap-1.5 text-xs text-fg-4", className)}>
      <img src={brand.coriMark} alt="" className="size-3.5 opacity-60 grayscale dark:invert" />
      Powered by Cori
    </As>
  );
}

function Sidebar({ inDrawer }: { inDrawer?: boolean }) {
  const route = useRoute();
  const s = useSession();
  const [view, app] = route;
  const sections = settingsSections(s.me, s.builds);
  const waiting = useWaitingIn().total(s.waiting);
  const signedOut = authMode() === "builtin" && !s.me && !s.loading;
  const item = (active: boolean) => cx(
    "group flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[14px] transition-colors",
    active ? "bg-surface font-medium text-fg shadow-xs ring-1 ring-line" : "text-fg-2 hover:bg-surface-3/60 hover:text-fg",
  );
  return (
    <nav aria-label="Main" className="flex h-full flex-col gap-1 overflow-y-auto px-3 pt-3 pb-3">
      <div className={cx(inDrawer && "pr-10")}><OrgSwitcher /></div>
      <button type="button" onClick={openPalette}
        className="mt-2 mb-2 flex h-8 cursor-pointer items-center gap-2.5 rounded-lg bg-surface px-2.5 text-[14px] text-fg-3 shadow-xs ring-1 ring-line transition-colors hover:text-fg-2">
        <Search className="size-4" aria-hidden />Search or jump to…{!inDrawer && <Kbd className="ml-auto">⌘K</Kbd>}
      </button>
      {signedOut ? (
        <button type="button" onClick={() => signIn()} className={cx(item(false), "cursor-pointer")}><LogIn className="size-4 text-fg-3" />Sign in</button>
      ) : (
        <>
          <a href={href()} className={item(!view)} aria-current={!view ? "page" : undefined}><Home className="size-4 text-fg-3" aria-hidden />Home</a>
          <a href={href("inbox")} className={item(view === "inbox")} aria-current={view === "inbox" ? "page" : undefined}>
            <Inbox className="size-4 text-fg-3" aria-hidden />Inbox
            {waiting > 0 && <span className="ml-auto rounded-full bg-amber-500 px-1.5 text-[11px] leading-[18px] font-semibold text-white tabular-nums" aria-label={`${waiting} waiting`}>{waiting}</span>}
          </a>
          <AppsNav apps={s.apps} current={view === "app" ? app : undefined} canCreate={s.builds} loading={s.loading} />
        </>
      )}
      <div className="mt-auto space-y-0.5 pt-3">
        {s.builds && <a href={href("connect")} className={item(view === "connect")}><Terminal className="size-4 text-fg-3" aria-hidden />Connect your agent</a>}
        {sections.length > 0 && <a href={href("settings")} className={item(view === "settings")}><Settings className="size-4 text-fg-3" aria-hidden />Settings</a>}
        <AccountMenu />
        <PoweredBy className="h-7 px-2.5" />
      </div>
    </nav>
  );
}

const SHOWN_APPS = 12;

function AppsNav({ apps, current, canCreate, loading }: { apps: MyApp[]; current?: string; canCreate: boolean; loading: boolean }) {
  const [all, setAll] = useState(false);
  const shown = all ? apps : apps.slice(0, SHOWN_APPS);
  const waiting = useWaitingIn().inApp;
  return (
    <div className="mt-5">
      <div className="mb-1 flex h-6 items-center justify-between px-2.5">
        <span className="text-xs font-medium text-fg-3">Apps</span>
        {canCreate && <a href={href("connect", "new")} aria-label="Create an app" title="Create an app" className="flex size-6 items-center justify-center rounded-md text-fg-3 hover:bg-surface-3/60 hover:text-fg"><Plus className="size-3.5" /></a>}
      </div>
      <div className="space-y-0.5">
        {shown.map((a) => {
          const active = a.id === current;
          return (
            <a key={a.id} href={href("app", a.id)} aria-current={active ? "page" : undefined} title={a.name || a.id}
              className={cx("flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[14px] transition-colors",
                active ? "bg-surface font-medium text-fg shadow-xs ring-1 ring-line" : "text-fg-2 hover:bg-surface-3/60 hover:text-fg")}>
              <AppMark app={a} />
              <span className="min-w-0 flex-1 truncate">{a.name || a.id}</span>
              {waiting(a) > 0 && <span className="rounded-full bg-amber-500/15 px-1.5 text-[11px] leading-[18px] font-semibold text-amber-700 tabular-nums dark:text-amber-300" aria-label={`${waiting(a)} waiting`}>{waiting(a)}</span>}
            </a>
          );
        })}
        {!loading && apps.length === 0 && <p className="px-2.5 py-1 text-[13px] text-fg-4">{canCreate ? "No app yet" : "No app shared with you yet"}</p>}
        {apps.length > SHOWN_APPS && (
          <button type="button" onClick={() => setAll(!all)} className="flex h-8 w-full cursor-pointer items-center rounded-lg px-2.5 text-[13px] text-fg-3 hover:bg-surface-3/60 hover:text-fg">
            {all ? "Show fewer" : `Show all ${apps.length}`}
          </button>
        )}
      </div>
    </div>
  );
}

/** An app's letter tile; its corner dot says whether a release is live. */
export function AppMark({ app, size = "sm" }: { app: Pick<MyApp, "id" | "name" | "promoted">; size?: "sm" | "lg" }) {
  const label = (app.name || app.id).replace(/[^a-z0-9]/gi, "");
  let h = 0;
  for (const ch of app.id) h = (h * 33 + ch.charCodeAt(0)) >>> 0;
  const hues = ["from-violet-500 to-indigo-500", "from-sky-500 to-cyan-500", "from-emerald-500 to-teal-500", "from-amber-500 to-orange-500", "from-rose-500 to-pink-500", "from-fuchsia-500 to-purple-500", "from-lime-500 to-green-600"];
  return (
    <span className={cx("relative flex shrink-0 items-center justify-center rounded-md bg-gradient-to-br font-semibold text-white", hues[h % hues.length], size === "lg" ? "size-10 rounded-xl text-base" : "size-5 text-[10px]")} aria-hidden>
      {(label[0] || "?").toUpperCase()}
      <span className={cx("absolute rounded-full ring-2 ring-canvas", size === "lg" ? "-right-0.5 -bottom-0.5 size-3" : "-right-0.5 -bottom-0.5 size-2", app.promoted ? "bg-emerald-500" : "bg-fg-4")} />
    </span>
  );
}

/** The organization the console works in; its menu opens another of the person's organizations. */
function OrgSwitcher() {
  const { me } = useSession();
  const org = me?.organization;
  const [asked, setAsked] = useState(false);
  const orgs = useQuery<MyOrganizations>(asked && org ? "/v1/me/organizations" : null, () => api("/v1/me/organizations"));
  if (!org) return <a href={href()} className="flex h-10 items-center px-2.5" aria-label={`${brand.name} home`}><BrandLogo /></a>;
  const label = org.name || org.slug || "Your organization";
  const list = orgs.data?.organizations ?? [];
  const builtin = authMode() === "builtin";
  // an organization of their own, where the identity provider lets them start one (its page, then back here signed in to it)
  const start = orgs.data?.start?.url;
  return (
    <Menu align="start" width="w-[232px]" items={[
      { heading: "Organizations" },
      ...(list.length ? list : [{ id: org.id, name: label, slug: org.slug, current: true } as MyOrganization]).map((o) => ({
        label: o.name || o.slug || o.id, checked: !!o.current, disabled: !!o.current || !builtin,
        onSelect: () => switchOrganization(o.slug || undefined),
      })),
      builtin && { label: "Another organization…", icon: <ArrowLeftRight />, onSelect: () => switchOrganization() },
      start && { label: "Start an organization…", icon: <Plus />, description: "To build apps of your own", href: start },
      "sep",
      me?.can?.people && { label: "Organization settings", icon: <Settings />, href: href("settings", "general") },
    ]}>
      {(p) => (
        <button {...p} type="button" onClick={() => { setAsked(true); p.onClick(); }}
          className="flex h-10 w-full cursor-pointer items-center gap-2.5 rounded-lg px-1.5 text-left transition-colors hover:bg-surface-3/60">
          <span className="flex size-7 shrink-0 items-center justify-center overflow-hidden rounded-lg bg-surface shadow-xs ring-1 ring-line"><img src={brand.mark} alt="" className="size-4.5" /></span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[14px] leading-tight font-semibold text-fg">{label}</span>
            {org.slug && org.name && <span className="block truncate text-xs leading-tight text-fg-3">{org.slug}</span>}
          </span>
          <ChevronsUpDown className="size-4 shrink-0 text-fg-4" aria-hidden />
        </button>
      )}
    </Menu>
  );
}

function AccountMenu() {
  const { me } = useSession();
  useToken();
  usePeople();
  const theme = useTheme();
  const mode = authMode();
  if (mode === "builtin" && !me) return null;
  const who = me?.email || me?.sub || "You";
  const name = personOf(me?.sub)?.name || (me?.email ? who.replace(/@.*/, "") : who);
  const account = authConfig().account_url;
  // who reaches their apps only has no settings: a member of an organization among others leaves it from here
  const org = me?.organization;
  const leave = mode === "builtin" && !!me?.apps_only && !me.guest && !!org;
  const orgLabel = org?.name || org?.slug || "this organization";
  return (
    <Menu side="top" align="start" width="w-[232px]" items={[
      { heading: <span className="block truncate">{who}</span> },
      mode === "dev" && { label: "View as…", icon: <UserRound />, description: "Act as another person (dev)", onSelect: () => openDialog((close) => <DevIdentity close={() => close(null)} />) },
      me?.portal_url && { label: "Apps portal", icon: <LayoutGrid />, description: "Where your colleagues open their apps", href: me.portal_url, external: true },
      account && { label: "Your account", icon: <ExternalLink />, href: account, external: true },
      !!me?.providers?.length && { label: "Connected accounts", icon: <Link2 />, description: `${me.providers.map((p) => p.label).join(", ")}: what your runs act with`, href: href("connections") },
      leave && { label: `Leave ${orgLabel}…`, icon: <DoorOpen />, onSelect: () => leaveOrganization(orgLabel) },
      "sep",
      { heading: "Theme" },
      { label: <span className="inline-flex items-center gap-2"><Monitor className="size-4 text-fg-3" />System</span>, checked: theme === "system", onSelect: () => setTheme("system") },
      { label: <span className="inline-flex items-center gap-2"><Sun className="size-4 text-fg-3" />Light</span>, checked: theme === "light", onSelect: () => setTheme("light") },
      { label: <span className="inline-flex items-center gap-2"><Moon className="size-4 text-fg-3" />Dark</span>, checked: theme === "dark", onSelect: () => setTheme("dark") },
      mode === "builtin" && "sep",
      mode === "builtin" && { label: "Sign out", icon: <LogOut />, onSelect: () => signOut() },
    ]}>
      {(p) => (
        <button {...p} type="button" className="mt-1 flex h-11 w-full cursor-pointer items-center gap-2.5 rounded-lg px-1.5 text-left transition-colors hover:bg-surface-3/60">
          <Avatar name={name} size="sm" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[13.5px] leading-tight font-medium text-fg">{name}</span>
            <span className="block truncate text-xs leading-tight text-fg-3">{me?.guest ? "Guest" : mode === "dev" ? (me?.roles || []).join(", ") || "dev" : me?.email ?? ""}</span>
          </span>
          {mode === "dev" ? <span className="rounded bg-amber-100 px-1 text-[10px] font-semibold tracking-wide text-amber-800 uppercase dark:bg-amber-500/15 dark:text-amber-300">dev</span> : null}
        </button>
      )}
    </Menu>
  );
}

import { useEffect } from "react";
import { LogIn } from "lucide-react";
import { authMode, getToken, rememberOrganization, signIn, switchOrganization, useToken } from "./lib/api";
import { useTitle } from "./lib/title";
import { brand } from "./lib/brand";
import { href, legacy, navigate, useRoute } from "./lib/router";
import { useSession } from "./lib/session";
import { BrandLogo, PoweredBy, Shell } from "./shell/Shell";
import { PaletteHost } from "./shell/CommandPalette";
import { Button, Empty, Spinner } from "./ui/core";
import { DialogHost } from "./ui/dialog";
import { Toaster } from "./ui/toast";
import { Handoff, isHandoff } from "./pages/Handoff";
import { Home } from "./pages/Home";
import { InboxPage } from "./pages/Inbox";
import { Connect } from "./pages/Connect";
import { ConnectionsPage } from "./pages/Connections";
import { AppPage } from "./pages/app/AppPage";
import { SettingsPage } from "./pages/settings/SettingsPage";

export function App() {
  useRoute();
  // a release's host or the apps portal signing its person in through the console: nothing of the console around it
  if (isHandoff()) return <Handoff />;
  return (
    <>
      <Routes />
      <Toaster />
      <DialogHost />
      <PaletteHost />
    </>
  );
}

function Routes() {
  const route = useRoute();
  useToken();
  const s = useSession();
  const moved = legacy(route);
  useEffect(() => { if (moved) navigate(moved, { replace: true }); }, [moved]);
  // a new tab signs in to the organization this browser last worked in, without asking again
  useEffect(() => rememberOrganization(s.me?.organization?.slug), [s.me?.organization?.slug]);

  // a new page starts at its top (hash routes keep the previous scroll); opening a release or a decision over a list does not
  const [view, a, ...rest] = route;
  const page = view === "app" ? `app/${a}/${rest[0] ?? ""}/${rest[0] === "runs" ? rest[1] ?? "" : ""}` : view === "inbox" ? "inbox" : `${view ?? ""}/${a ?? ""}`;
  useEffect(() => { window.scrollTo(0, 0); }, [page]);

  if (moved) return null;
  if (authMode() === "builtin" && !getToken()) return <SignInScreen />;
  if (view === "switch") return <SwitchOrganization slug={a} />;
  const wide = view === "app" && rest[0] === "data";
  return (
    <Shell wide={wide}>
      {s.error && !s.me ? <Empty title="The console could not load">{s.error}</Empty>
        : !view ? <Home />
        : view === "inbox" ? <InboxPage app={a} item={rest[0]} />
        : view === "app" && a ? <AppPage app={a} rest={rest} />
        : view === "settings" ? <SettingsPage section={a} />
        : view === "connect" ? <Connect focus={a} />
        : view === "connections" ? <ConnectionsPage />
        : <NotFound />}
    </Shell>
  );
}

function NotFound() {
  useTitle("Not found");
  return <Empty title="This page does not exist" action={<a href={href()} className="link">Go home</a>}>The link may be old, or mistyped.</Empty>;
}

/** Builtin auth without a session: one button to the identity provider. */
function SignInScreen() {
  useTitle("Sign in");
  return (
    <div className="flex min-h-dvh flex-col px-4">
      <div className="flex flex-1 items-center justify-center">
        <div className="w-full max-w-sm animate-rise text-center">
          <div className="mb-8 flex justify-center"><BrandLogo className="h-8" /></div>
          <h1 className="text-xl font-semibold">Sign in to {brand.name}</h1>
          <p className="mt-2 text-fg-3">Your organization's apps, decisions and data.</p>
          <Button variant="primary" size="md" className="mt-6 w-full" icon={<LogIn />} onClick={() => signIn()}>Sign in</Button>
        </div>
      </div>
      <PoweredBy as="footer" className="justify-center py-6" />
    </div>
  );
}

/** `#/switch/<slug>` (an accepted invitation's link): signs in again to that organization, then comes back home. */
function SwitchOrganization({ slug }: { slug?: string }) {
  useEffect(() => { if (authMode() === "builtin") switchOrganization(slug || undefined); else navigate(href(), { replace: true }); }, [slug]);
  return <div className="flex min-h-dvh items-center justify-center"><Spinner label={slug ? `Opening ${slug}` : "Switching organization"} /></div>;
}

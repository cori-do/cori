import { useUser } from "@cori-do/client/react";
import { LayoutGrid, Lock } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Card, EmptyState, Spinner, cx } from "./components/ui";
import { coriConfig } from "./lib/api";
import { APP_NAME } from "./lib/app";
import { cori } from "./lib/cori";
import { VIEWS } from "./views";

function useHashView(): string {
  const read = () => location.hash.replace(/^#\/?/, "").split("/")[0] || VIEWS[0].id;
  const [view, setView] = useState(read);
  useEffect(() => {
    const on = () => setView(read());
    addEventListener("hashchange", on);
    return () => removeEventListener("hashchange", on);
  }, []);
  return view;
}

export function App() {
  const user = useUser(cori);
  const cfg = coriConfig();
  const allowed = VIEWS.filter((v) => !v.roles || v.roles.some((r) => user.roles.includes(r)));
  // a hook runs on every render, never inside a callback (`find` would call it once per view visited)
  const view = useHashView();
  const current = allowed.find((v) => v.id === view) ?? allowed[0];
  const View = current?.component;
  return (
    <div className="min-h-dvh">
      <header className="sticky top-0 z-10 border-b border-zinc-200 bg-white/80 backdrop-blur dark:border-zinc-800 dark:bg-zinc-950/80">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-3 px-4 sm:gap-4 sm:px-6">
          <a href="#/" className="flex shrink-0 items-center gap-2 font-semibold">
            <span className="grid size-7 place-items-center rounded-lg bg-brand-600 text-sm text-white">◆</span>
            <span className="hidden sm:inline">{APP_NAME}</span>
          </a>
          {cfg.preview && <Badge tone="amber" className="shrink-0 whitespace-nowrap">Preview {cfg.release ?? cfg.version?.replace(/^sha256:/, "").slice(0, 12)}</Badge>}
          <nav className="flex min-w-0 flex-1 gap-1 overflow-x-auto" aria-label="Main">
            {allowed.map((v) => (
              <a
                key={v.id}
                href={`#/${v.id}`}
                aria-current={v.id === current?.id ? "page" : undefined}
                className={cx(
                  "flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium whitespace-nowrap",
                  v.id === current?.id ? "bg-zinc-100 text-zinc-900 dark:bg-zinc-800 dark:text-zinc-50" : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100",
                )}
              >
                {v.icon}
                {v.label}
              </a>
            ))}
          </nav>
          {cfg.portal_url && (
            <a href={cfg.portal_url} className="hidden shrink-0 items-center gap-1.5 text-sm text-zinc-600 hover:text-zinc-900 sm:flex dark:text-zinc-400 dark:hover:text-zinc-100">
              <LayoutGrid className="size-4" aria-hidden />
              All apps
            </a>
          )}
          {!user.loading && user.sub && (
            <div className="hidden shrink-0 items-center gap-2 text-sm sm:flex">
              <span className="hidden text-zinc-600 md:inline dark:text-zinc-300">{user.email ?? user.sub}</span>
              {user.roles.map((r) => <Badge key={r}>{r}</Badge>)}
            </div>
          )}
        </div>
      </header>
      <main className="mx-auto max-w-6xl px-4 py-8 sm:px-6">
        {user.error ? <p className="text-sm text-rose-600">Not signed in: {user.error}</p> : user.loading ? <Spinner /> : View ? <View /> : <NoScreen />}
      </main>
    </div>
  );
}

/** Signed in, but none of the screens is for the caller's roles: say which roles open them. */
function NoScreen() {
  const roles = [...new Set(VIEWS.flatMap((v) => v.roles ?? []))];
  return (
    <Card>
      <EmptyState
        icon={<Lock className="size-5" aria-hidden />}
        title="Nothing here for your roles yet"
        description={roles.length ? `This app's screens are for ${roles.join(" or ")}. Ask an administrator of your organization for one of these roles in ${APP_NAME}.` : undefined}
      />
    </Card>
  );
}

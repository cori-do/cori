import { api } from "./api";
import { useQuery } from "./query";
import type { AppSummary, Me, MyApp } from "./types";

// Who the console acts as and the apps they can open. The console shapes itself on it (a tab, a button shows when the
// person's roles allow it); the server checks every call anyway.

export type Session = {
  me?: Me;
  /** the apps the person can open: those they build and those where they hold a role */
  apps: MyApp[];
  /** the apps they build, with how each ships and runs (builders only) */
  built: AppSummary[];
  /** they build apps (or may create one): they see releases, previews, schema and SQL */
  builds: boolean;
  /** they reach their apps and nothing else of the organization (a guest, or a person with no power who builds no app) */
  appsOnly: boolean;
  /** decisions waiting for them: their apps' approvals, and for admins the shares outside the organization */
  waiting: number;
  loading: boolean;
  error?: string;
};

const EMPTY: never[] = [];

export function useSession(): Session {
  const me = useQuery<Me>("/v1/me", () => api<Me>("/v1/me"));
  const mine = useQuery<MyApp[]>("/v1/me/apps", () => api<MyApp[]>("/v1/me/apps"));
  const builds = !!me.data && !me.data.guest && (me.data.persona !== "user" || !!me.data.can?.create_apps);
  const built = useQuery<AppSummary[]>(builds ? "/v1/apps" : null, () => api<AppSummary[]>("/v1/apps"));
  const apps = mine.data ?? EMPTY;
  const waiting = apps.reduce((n, a) => n + (a.waiting_for_me || 0), 0) + (me.data?.access_waiting || 0);
  return {
    me: me.data, apps, built: built.data ?? EMPTY, builds, appsOnly: !!me.data?.apps_only || !!me.data?.guest, waiting,
    loading: (me.loading && !me.data) || (mine.loading && !mine.data), error: me.error,
  };
}

/** The person's relation to one app: null when it is not among their apps. */
export function useMyApp(app: string | null): MyApp | null {
  const { apps } = useSession();
  return app ? apps.find((a) => a.id === app) ?? null : null;
}

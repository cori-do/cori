import { useCallback, useEffect, useSyncExternalStore } from "react";
import { errMsg, onIdentityChange } from "./api";

// A small cache of what the server answered, by key (usually the path): a page shows what it had at once and asks again in
// the background (stale-while-revalidate), concurrent asks share one request, and a change invalidates the keys it
// touches (`invalidate("/v1/apps/x")`). A new identity starts from nothing.

type Snap = { data?: unknown; error?: string; loading: boolean };
type Entry = { snap: Snap; at: number; inflight?: Promise<void>; fn?: () => Promise<unknown>; subs: Set<() => void> };
const entries = new Map<string, Entry>();
const IDLE: Snap = { loading: false };
/** How long an answer counts as fresh: a page mounted within it does not ask again. */
const FRESH_MS = 3000;
/** Bumped by a new identity: answers asked for by the previous one are dropped. */
let generation = 0;

function entry(key: string): Entry {
  let e = entries.get(key);
  if (!e) { e = { snap: { loading: false }, at: 0, subs: new Set() }; entries.set(key, e); }
  return e;
}
const set = (e: Entry, s: Partial<Snap>) => { e.snap = { ...e.snap, ...s }; e.subs.forEach((l) => l()); };

function run(key: string): Promise<void> {
  const e = entry(key);
  if (e.inflight) return e.inflight;
  if (!e.fn) return Promise.resolve();
  const g = generation;
  set(e, { loading: true });
  const p = e.fn()
    .then((data) => { if (g === generation) set(e, { data, error: undefined, loading: false }); },
      (err) => { if (g === generation) set(e, { error: errMsg(err), loading: false }); })
    .finally(() => { if (g === generation) { e.at = Date.now(); e.inflight = undefined; } });
  e.inflight = p;
  return p;
}

export type Query<T> = { data?: T; error?: string; loading: boolean; reload: () => Promise<void> };

/** The answer of `fn` under `key` (null: nothing to ask yet). `every`: ask again every so many ms while the tab shows. */
export function useQuery<T>(key: string | null, fn: () => Promise<T>, opts: { every?: number } = {}): Query<T> {
  const subscribe = useCallback((l: () => void) => {
    if (!key) return () => {};
    const e = entry(key);
    e.subs.add(l);
    return () => { e.subs.delete(l); };
  }, [key]);
  const snap = useSyncExternalStore(subscribe, () => (key ? entry(key).snap : IDLE));
  if (key) entry(key).fn = fn; // the latest closure asks
  useEffect(() => {
    if (!key) return;
    const e = entry(key);
    if (!e.inflight && Date.now() - e.at > FRESH_MS) void run(key);
  }, [key]);
  useEffect(() => {
    if (!key || !opts.every) return;
    const t = setInterval(() => { if (document.visibilityState === "visible") void run(key); }, opts.every);
    return () => clearInterval(t);
  }, [key, opts.every]);
  const reload = useCallback(() => (key ? run(key) : Promise.resolve()), [key]);
  return { data: snap.data as T | undefined, error: snap.error, loading: snap.loading || (!!key && snap.data === undefined && !snap.error), reload };
}

/** Asks again for every key starting with one of `prefixes` (all keys without one): shown ones now, others when next shown. */
export function invalidate(...prefixes: string[]) {
  for (const [k, e] of entries) {
    if (prefixes.length && !prefixes.some((p) => k.startsWith(p))) continue;
    e.at = 0;
    if (e.subs.size) void run(k);
  }
}

/** What `key` holds now, without asking. */
export const peek = <T,>(key: string) => entries.get(key)?.snap.data as T | undefined;

// a person coming back to the tab sees fresh data; a new identity sees nothing of the previous one's
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    for (const [k, e] of entries) if (e.subs.size && Date.now() - e.at > 15_000) void run(k);
  });
}
onIdentityChange(() => {
  generation++;
  for (const [k, e] of entries) {
    e.at = 0;
    e.inflight = undefined;
    set(e, { data: undefined, error: undefined, loading: false });
    if (e.subs.size) void run(k);
  }
});

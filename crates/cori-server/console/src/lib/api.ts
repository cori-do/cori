import { useSyncExternalStore } from "react";
import { oidcSession, type OidcSession } from "@cori-do/client";
import type { AuthConfig } from "./types";

// The identity every call is made as, by auth mode (`/v1/auth/config`, read once by `initAuth` before the first render):
// - dev: a dev token `dev:<user>:<role,role>`, chosen in the account menu and kept in localStorage `cori.tok`;
// - builtin: the signed-in person's access token from the issuer (authorization code + PKCE with the public web client),
//   kept in memory and refreshed before it expires; the refresh token lives in sessionStorage (this tab only);
// - off: no token.
export type AuthMode = "dev" | "builtin" | "off";

/** Mock mode (`npm run mock`, `npm run build:demo`): src/mock answers every call in the browser, no server needed. In a
 *  production build the constant folds to false and the mock is not bundled. */
export const MOCK = import.meta.env.MODE === "mock";

const KEY = "cori.tok";
const DEFAULT_TOKEN = "dev:admin:it_admin";
const listeners = new Set<() => void>();
const identityListeners = new Set<() => void>();
let mode: AuthMode = "dev";
let session: OidcSession | null = null;
let token = "";
let config: AuthConfig = {};
let send: (path: string, init: RequestInit) => Promise<Response> = (path, init) => fetch(path, init);
const notify = () => listeners.forEach((l) => l());
/** Another person (dev "View as", sign-out): what was loaded for the previous one goes. A refreshed token is not one. */
const newIdentity = () => { notify(); identityListeners.forEach((l) => l()); };

export const authMode = () => mode;
export const getToken = () => token;
/** What `/v1/auth/config` said: the MCP endpoint, the deployment's plugin for coding agents, the account page. */
export const authConfig = () => config;
export const onTokenChange = (l: () => void) => { listeners.add(l); return () => { listeners.delete(l); }; };
export const onIdentityChange = (l: () => void) => { identityListeners.add(l); return () => { identityListeners.delete(l); }; };

/** Claude Code's command adding this server: with the deployment's own client for MCP hosts when its identity provider
 *  registers none for them (and the callback's port that client allows). */
export function claudeMcpAdd(mcp: string, c: AuthConfig = config): string {
  const arg = (v: string) => (/^[\w.:@/-]+$/.test(v) ? v : `'${v.replace(/'/g, `'\\''`)}'`);
  const client = c.mcp_client_id ? ` --client-id ${arg(c.mcp_client_id)}${c.mcp_callback_port ? ` --callback-port ${c.mcp_callback_port}` : ""}` : "";
  return `claude mcp add --transport http${client} cori ${mcp}`;
}

const stored = (k: string) => { try { return localStorage.getItem(k); } catch { return null; } };
const store = (k: string, v: string | null) => { try { if (v === null) localStorage.removeItem(k); else localStorage.setItem(k, v); } catch { /* private mode */ } };

/** Reads the server's auth mode and, under builtin auth, finishes a sign-in coming back from the issuer. */
export async function initAuth(): Promise<void> {
  if (MOCK) {
    send = (await import("../mock/server")).handle;
    // the demo opens as someone given in its address: `?as=dev:sam:manager`
    const as = new URLSearchParams(location.search).get("as");
    if (as) store(KEY, as);
  }
  const c: AuthConfig = await send("/v1/auth/config", {}).then((r) => r.json()).catch(() => ({}));
  config = c;
  mode = c.mode === "builtin" || c.mode === "off" ? c.mode : "dev";
  if (mode === "dev") { token = stored(KEY) || DEFAULT_TOKEN; return; }
  store(KEY, null); // a JWT pasted by an older console must not linger
  if (mode === "builtin" && c.issuer && c.web_client_id) {
    session = oidcSession({ issuer: c.issuer, client_id: c.web_client_id, resource: c.resource, scopes: c.scopes });
    await session.completeLogin().catch(() => false);
    token = (await session.token().catch(() => null)) ?? "";
    // a new tab has no refresh token (sessionStorage is per tab): bounce through the issuer, which answers at once when its
    // own session is alive, in the organization this browser last worked in; not again within a minute of a sign-in that
    // gave no token (no redirect loop)
    if (!token && session.mayAutoLogin()) { const org = handoffOrganization(); await session.login(org ? { organization: org } : lastOrganization()); }
  }
}

/** A release's host, the apps portal or a front's pass signing its person in through the console (`#/preview/<app>/v<n>?…`,
 *  `#/portal?…`, `#/pass/<app>?…`): the organization it belongs to, which the console's sign-in goes to. */
export function handoffOrganization(): string | undefined {
  const m = /^#\/(?:(?:preview|pass)\/[^?]*|portal)\?(.*)$/.exec(location.hash);
  return (m && new URLSearchParams(m[1]).get("organization")) || undefined;
}

/** Builtin auth: signs in again to the organization `slug`, back to this page (its hash kept). */
export async function signInTo(slug: string) { await session?.login({ organization: slug }); }

/** Builtin auth: sends the browser to the issuer's sign-in page (and back here). */
export const signIn = () => session?.login();

// The organization this browser last worked in, so that a new tab's sign-in goes back to it without asking which one.
const ORG_KEY = "cori.org";
const lastOrganization = (): Record<string, string> | undefined => { const slug = stored(ORG_KEY); return slug ? { organization: slug } : undefined; };
export function rememberOrganization(slug?: string | null) { if (slug) store(ORG_KEY, slug); }

/** Builtin auth: signs in again to choose the organization (the issuer asks which one), or to `slug`. Back to Home after. */
export async function switchOrganization(slug?: string) {
  if (!session) return;
  history.replaceState(null, "", `${location.pathname}${location.search}#/`);
  await session.login(slug ? { organization: slug } : undefined);
}
/** Builtin auth: forgets the tokens and ends the issuer's session. */
export async function signOut() { await session?.logout(); token = ""; newIdentity(); }

/** Dev auth only: act as another dev token (the account menu's "View as"). */
export function setToken(t: string) {
  if (mode !== "dev") return;
  token = t.trim();
  store(KEY, token);
  newIdentity();
}
export const useToken = () => useSyncExternalStore(onTokenChange, getToken);

/** The parts of a dev token, or null for a JWT. */
export function devToken(t = token): { user: string; roles: string[] } | null {
  const p = t.split(":");
  return p[0] === "dev" && p.length >= 3 ? { user: p[1], roles: p[2].split(",").filter(Boolean) } : null;
}

/** An error the server answered, with its status and its answer (a run refused for an account names what to connect). */
export class ApiError extends Error {
  constructor(message: string, readonly status: number, readonly body: Record<string, unknown> = {}) { super(message); }
}

async function fresh() {
  if (!session) return;
  // a fresh access token for every call (refreshed a little before it expires)
  const t = (await session.token().catch(() => null)) ?? "";
  if (t !== token) { token = t; notify(); }
}

export async function api<T = unknown>(path: string, opt: RequestInit = {}): Promise<T> {
  await fresh();
  const auth: Record<string, string> = token ? { authorization: `Bearer ${token}` } : {};
  const r = await send(path, { ...opt, headers: { "content-type": "application/json", ...auth, ...(opt.headers || {}) } });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new ApiError(r.status === 401 && mode === "builtin" ? "Sign in to continue" : j.error || r.statusText || `HTTP ${r.status}`, r.status, j);
  return j as T;
}

const body = (method: string, b: unknown): RequestInit => ({ method, body: JSON.stringify(b ?? {}) });
export const post = <T = unknown>(path: string, b?: unknown) => api<T>(path, body("POST", b));
export const put = <T = unknown>(path: string, b?: unknown) => api<T>(path, body("PUT", b));
export const patch = <T = unknown>(path: string, b?: unknown) => api<T>(path, body("PATCH", b));
export const del = <T = unknown>(path: string, b?: unknown) => api<T>(path, body("DELETE", b));

/** A file the server sends (an export, a file a run was given): fetched with the caller's token, then saved under the
 *  name the server gives (its UTF-8 name when it sends one). */
export async function download(path: string, fallbackName: string) {
  await fresh();
  const r = await send(path, { headers: token ? { authorization: `Bearer ${token}` } : {} });
  if (!r.ok) { const j = await r.json().catch(() => ({})); throw new ApiError(j.error || r.statusText, r.status); }
  const cd = r.headers.get("content-disposition") || "";
  const utf8 = /filename\*=UTF-8''([^;]+)/i.exec(cd)?.[1];
  const name = (utf8 ? (() => { try { return decodeURIComponent(utf8); } catch { return null; } })() : null) || /filename="([^"]+)"/.exec(cd)?.[1] || fallbackName;
  const url = URL.createObjectURL(await r.blob());
  const a = document.createElement("a"); a.href = url; a.download = name; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** A file for a workflow's file parameter (a run the console starts): its upload announced, then its bytes sent with the
 *  caller's token. Answers the file's reference. */
export async function uploadFile(app: string, workflow: string, parameter: string, file: File): Promise<{ id: string; name: string; size: number; content_type: string }> {
  const a = await post<{ id: string; upload?: { method?: string; url?: string } }>(`/v1/apps/${app}/files`, { workflow, parameter, name: file.name, size: file.size, content_type: file.type || undefined });
  await fresh();
  const r = await send(a.upload?.url || `/v1/apps/${app}/files/${a.id}/content`, { method: a.upload?.method || "PUT", body: file, headers: { "content-type": "application/octet-stream", ...(token ? { authorization: `Bearer ${token}` } : {}) } });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new ApiError(j.error || r.statusText || `HTTP ${r.status}`, r.status, j);
  return j;
}

export const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

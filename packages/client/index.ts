// SPDX-License-Identifier: MIT-0
/**
 * @cori-do/client — runtime for generated `cori.ts` files. Framework-agnostic; see ./react.ts for hooks.
 *
 * Under `cori up` (auth mode `dev`) the hosted config carries a `dev` block and the client signs its calls with a
 * `dev:<sub>:<roles>` token kept per app in localStorage (`cori.tok.<app>`, the key the dev widget writes too), else
 * the app's first `[[dev.personas]]`. The server injects its dev widget into every hosted front; a front served
 * elsewhere (a Vite dev server) gets it loaded from the gateway by this client. Neither exists under `builtin` or `off` auth.
 */
export interface ClientOptions { baseUrl?: string; app?: string; getToken?: () => Promise<string | null> | string | null; fetch?: typeof fetch; pollMs?: number;
  /** `false` stops this client from loading the dev widget when the front is not served by Cori (the widget only exists under dev auth). */
  devWidget?: boolean;
  /** Builtin auth: `false` stops a call without a signed-in user from sending the browser to the issuer (the app then
   *  calls `login()` itself, e.g. from a button). Default: sign in automatically. */
  autoLogin?: boolean }
/** Per-call options of `workflow(...).start` / `.run`. */
export interface StartOptions { /** Sent as `Idempotency-Key`; only read by endpoints with `idempotency: header` (24 h window, per caller). */ idempotencyKey?: string;
  /** Progress of the files the call uploads first (a `File` given for a file parameter), as bytes sent of bytes in all. */
  onUploadProgress?: (sent: number, total: number) => void }
/** A file a run was given (a `file` or `files` parameter): its reference, as the server keeps it. */
export interface CoriFile { id: string; name: string; size: number; content_type: string; sha256: string; status?: "pending" | "receiving" | "ready" | string; created_at?: string; expires_at?: string | null }
/** What a front passes for a file parameter: a browser `File` (or `Blob`), uploaded before the run starts, or a file
 *  uploaded already (its reference, or its id). */
export type CoriFileInput = Blob | CoriFile | string;
/** `upload()`: the workflow and the file parameter the file is for (their `accept` and `max_size` apply). */
export interface UploadOptions { workflow: string; parameter: string; /** The name when the Blob is no File. */ name?: string;
  /** Bytes sent of bytes in all, as the upload goes. */ onProgress?: (sent: number, total: number) => void; signal?: AbortSignal }
export interface StepRow { name: string; kind: string; status: "ok" | "failed" | "not_taken" | string; ms: number; cost_eur: number; notes: string[]; error?: string | null }
export interface RunView<V = Record<string, unknown>, O = Record<string, unknown>> { runId: string; workflow?: string; status: "running" | "succeeded" | "failed" | string; error?: string | null; errorDetail?: { step?: string | null; type?: string | null; message: string } | null; result?: { headline?: string; fields: { label: string; value: unknown }[]; sections: { label: string; value: unknown }[]; values?: V } | null; outputs?: O; steps?: StepRow[]; startedAt?: string; endedAt?: string | null ;
  /** While the run waits for a person: the `human` step, the role (or `user:initiator`) it is routed to, since when. */
  pendingApproval?: { id: string; step: string; to: string; since: string } | null }

/** `/__cori/config.json` as the server injects it next to the hosted front. */
export interface CoriConfig { app?: string; baseUrl?: string; auth?: "dev" | "builtin" | "off"; version?: string | null; preview?: boolean; dev?: DevInfo; oidc?: OidcConfig;
  /** A release's host: its number (`v12`). */
  release?: string | null;
  /** Where the organization's people find their apps (its apps portal), when it has one. */
  portal_url?: string | null }
/** The `dev` block of the config: only present under dev auth. */
export interface DevInfo { roles: string[]; front_roles: string[]; personas?: DevPersona[]; owner?: string | null; builders?: string[]; workflows: { id: string; description?: string; endpoint: { mode: string; auth: string; roles: string[] } | null; schedule?: string | null }[]; mocks: string[]; hint?: string }
/** The signed-in user as the server sees it (`GET /v1/me`). */
export interface User { sub: string; roles: string[]; email?: string | null; kind?: "human" | "agent" | "service" | string; app?: string | null;
  /** Inside an app: the organization roles (admin, builder, approver), apart from the app `roles`. */
  platform_roles?: string[] }
export interface DevIdentity { sub: string; roles: string[] }
/** One `[[dev.personas]]` entry of cori.toml: a user of the app's seed, offered by the dev widget. */
export interface DevPersona extends DevIdentity { name: string; title?: string | null }

// ---- dev identity: one per app (`cori.tok.<app>`), shared with the dev widget so both switch together ----
// Apps on one origin (`cori up` serves them all) never see each other's identity. The `cori_dev` cookie carries the
// current page's identity for fronts that send no bearer; the console keeps its own `cori.tok`.
const DEV_TOKEN_KEY = "cori.tok";
const IDENTITY_EVENT = "cori-dev-identity";     // window event the widget and this client raise on a switch
const memoryTokens = new Map<string, string | null>();
const listeners = new Set<() => void>();
let scope: string | null = null;                // the page's app: set by the first client that knows it
function storage(): Storage | null { try { return typeof localStorage === "undefined" ? null : localStorage; } catch { return null; } }
/** The localStorage key of an app's dev identity (`cori.tok.<app>`; the console's own `cori.tok` without an app). */
export function devTokenKey(app: string | null | undefined = scope): string { return app ? `${DEV_TOKEN_KEY}.${app}` : DEV_TOKEN_KEY; }
/** `dev:<sub>:<role,role>` → identity, or null for anything else (a JWT, an empty field). */
export function parseDevToken(token: string | null | undefined): DevIdentity | null {
  if (!token || !token.startsWith("dev:")) return null;
  const [sub, roles = ""] = token.slice(4).split(":", 2);
  return { sub: sub || "dev", roles: roles.split(",").map((r) => r.trim()).filter(Boolean) };
}
export function devToken(id: DevIdentity): string { return `dev:${id.sub || "dev"}:${id.roles.join(",")}`; }
/** The identity chosen for `app` (default: the page's app) on this browser, or null when none was chosen yet. */
export function getDevIdentity(app: string | null = scope): DevIdentity | null {
  const s = storage(), key = devTokenKey(app);
  return parseDevToken(s ? s.getItem(key) : memoryTokens.get(key));
}
/** Choose (or clear, with null) the dev identity of `app` (default: the page's app); every client and hook on the page refreshes. */
export function setDevIdentity(id: DevIdentity | null, app: string | null = scope): void {
  const tok = id ? devToken({ sub: id.sub, roles: id.roles }) : null, s = storage(), key = devTokenKey(app);
  if (s) { if (tok) s.setItem(key, tok); else s.removeItem(key); } else memoryTokens.set(key, tok);
  // the cookie is what the gateway reads for calls that carry no bearer (a hand-written page on the same origin)
  if (typeof document !== "undefined") document.cookie = `cori_dev=${tok ? encodeURIComponent(tok) : ""}; path=/; SameSite=Lax${tok ? "" : "; max-age=0"}`;
  if (typeof dispatchEvent !== "undefined") dispatchEvent(new CustomEvent(IDENTITY_EVENT, { cancelable: true })); else for (const l of listeners) l();
}
/** Subscribe to identity changes (this client, the dev widget, another tab of the app); returns the unsubscribe function. */
export function onDevIdentityChange(cb: () => void): () => void { listeners.add(cb); return () => { listeners.delete(cb); }; }
if (typeof addEventListener !== "undefined") {
  // cancelling tells the dev widget a client is refreshing its hooks, so it does not reload the page
  addEventListener(IDENTITY_EVENT, (e) => { e.preventDefault(); for (const l of listeners) l(); });
  addEventListener("storage", (e) => { if ((e as StorageEvent).key === devTokenKey()) for (const l of listeners) l(); });
}
/** The personas of the app (`[[dev.personas]]` in cori.toml), empty outside dev auth. */
export function devPersonas(c: CoriConfig): DevPersona[] { return c.dev?.personas ?? []; }
/** Without a chosen identity: the app's first persona, else `dev` with the front's roles (every role the app knows when the front declares none). */
export function defaultDevIdentity(c: CoriConfig): DevIdentity {
  const p = devPersonas(c)[0]; if (p) return { sub: p.sub, roles: p.roles };
  const d = c.dev; return { sub: "dev", roles: d?.front_roles?.length ? d.front_roles : d?.roles ?? [] };
}
/** The identity calls carry under dev auth: the one chosen for the app, else its default. */
export function currentDevIdentity(c: CoriConfig): DevIdentity { return getDevIdentity(c.app ?? scope) ?? defaultDevIdentity(c); }

/** Where the hosted config lives: next to the app when served under `/apps/<app>/` or a preview's `/apps/<app>/@<hash>/`, else at the site root; a front
 *  served elsewhere (a Vite dev server) reaches it through the gateway when it knows `baseUrl` and `app`. */
function configUrls(opts: ClientOptions): string[] {
  const urls: string[] = [];
  if (typeof location !== "undefined") {
    const m = location.pathname.match(/^\/apps\/[^/]+\/(@[^/]+\/)?/);
    if (m) urls.push(`${m[0]}__cori/config.json`);
    urls.push("/__cori/config.json");
  }
  if (opts.baseUrl && opts.app) urls.push(`${opts.baseUrl.replace(/\/$/, "")}/apps/${opts.app}/__cori/config.json`);
  return urls;
}

async function loadConfig(opts: ClientOptions, f: typeof fetch): Promise<CoriConfig> {
  for (const url of configUrls(opts)) {
    try { const r = await f(url); if (r.ok) return await r.json(); } catch { /* not hosted by cori */ }
  }
  return {};
}

export function createCoriClient(opts: ClientOptions = {}) {
  const f = opts.fetch ?? fetch;
  let cfg: Promise<CoriConfig> | null = null;
  if (opts.app && !scope) scope = opts.app;
  const config = () => { if (!cfg) cfg = loadConfig(opts, f).then((c) => { if (!scope && c.app) scope = c.app; return c; }); return cfg; };
  // a front served from `/apps/<app>/@<hash>/` (the preview of a deployed version) runs that version's workflows, on its sandbox stores
  const conf = async () => { const c = await config(); return { app: opts.app ?? c.app ?? "", baseUrl: (opts.baseUrl ?? c.baseUrl ?? "").replace(/\/$/, ""), version: c.preview ? c.version ?? null : null }; };
  const wfPath = (app: string, version: string | null, id: string) => version ? `/v1/apps/${app}/versions/${version}/wf/${id}/runs` : `/v1/apps/${app}/wf/${id}/runs`;
  /** The bearer for a call: the app's `getToken`, else (dev auth only) the chosen or default dev identity. */
  const token = async (): Promise<string | null> => {
    if (opts.getToken) return await opts.getToken();
    const c = await config();
    if (c.auth === "dev") return devToken(getDevIdentity(opts.app ?? c.app ?? scope) ?? defaultDevIdentity(c));
    if (c.auth === "builtin" && c.oidc) {
      const s = oidcSession(c.oidc); await s.completeLogin();
      const t = await s.token();
      // a sign-in the issuer refused: its sentence, for the page to show, rather than another round to the issuer
      const refused = t ? null : s.error();
      if (refused) throw new CoriError(401, refused);
      // nobody signed in: go to the issuer and come back here, unless the app opted out or a sign-in just failed
      if (!t && opts.autoLogin !== false && s.mayAutoLogin()) await s.login();
      return t;
    }
    return null;
  };
  /** The sign-in session under builtin auth (the issuer and public client the server announces), else null. */
  const session = async (): Promise<OidcSession | null> => { const c = await config(); return c.auth === "builtin" && c.oidc ? oidcSession(c.oidc) : null; };
  async function req(method: string, path: string, body?: unknown, extra: Record<string, string> = {}): Promise<{ status: number; json: any }> {
    const { baseUrl } = await conf();
    const headers: Record<string, string> = { "content-type": "application/json", ...extra };
    const tok = await token();
    if (tok) headers["authorization"] = `Bearer ${tok}`;
    const r = await f(baseUrl + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    const json = await r.json().catch(() => null);
    if (r.status >= 400) throw new CoriError(r.status, json?.error ?? r.statusText);
    return { status: r.status, json };
  }
  /** A file's bytes to where the server said (`upload.url`): this server with the caller's bearer, else (an address
   *  of another host, a signed one) without it. XMLHttpRequest when progress is wanted, fetch otherwise. */
  async function send(url: string, method: string, file: Blob, o: UploadOptions): Promise<any> {
    const { baseUrl } = await conf();
    const own = !/^https?:\/\//.test(url);
    const headers: Record<string, string> = { "content-type": "application/octet-stream" };
    if (own) { const tok = await token(); if (tok) headers["authorization"] = `Bearer ${tok}`; }
    const target = own ? baseUrl + url : url;
    if (o.onProgress && typeof XMLHttpRequest !== "undefined") {
      return await new Promise((resolve, reject) => {
        const x = new XMLHttpRequest();
        x.open(method, target);
        for (const [k, v] of Object.entries(headers)) x.setRequestHeader(k, v);
        x.upload.onprogress = (e) => o.onProgress?.(e.loaded, e.lengthComputable ? e.total : file.size);
        x.onload = () => { let j: any = null; try { j = JSON.parse(x.responseText); } catch { /* not JSON */ } if (x.status >= 400) reject(new CoriError(x.status, j?.error ?? x.statusText)); else resolve(j); };
        x.onerror = () => reject(new CoriError(0, "the upload did not reach the server"));
        x.onabort = () => reject(new CoriError(0, "aborted"));
        o.signal?.addEventListener("abort", () => x.abort());
        x.send(file);
      });
    }
    const r = await f(target, { method, headers, body: file, signal: o.signal });
    const j = await r.json().catch(() => null);
    if (r.status >= 400) throw new CoriError(r.status, j?.error ?? r.statusText);
    o.onProgress?.(file.size, file.size);
    return j;
  }
  /** Uploads a file for a workflow's file parameter: announced (`POST …/files`), then its bytes sent; returns its
   *  reference, which `start()` / `run()` take (they also take the File itself and upload it first). */
  async function upload(file: Blob, o: UploadOptions): Promise<CoriFile> {
    const { app, version } = await conf();
    const name = o.name ?? (file as File).name ?? "file";
    const path = version ? `/v1/apps/${app}/versions/${version}/files` : `/v1/apps/${app}/files`;
    const a = (await req("POST", path, { workflow: o.workflow, parameter: o.parameter, name, size: file.size, content_type: file.type || undefined })).json;
    const done = await send(a.upload?.url ?? `/v1/apps/${app}/files/${a.id}/content`, a.upload?.method ?? "PUT", file, o);
    return (done && done.id ? done : { ...a, status: "ready" }) as CoriFile;
  }
  const fileId = (file: CoriFile | string) => typeof file === "string" ? file : file.id;
  /** A file's bytes, fetched with the caller's bearer (never a link that carries it). */
  async function fileBlob(file: CoriFile | string): Promise<Blob> {
    const { app, baseUrl } = await conf();
    const headers: Record<string, string> = {};
    const tok = await token(); if (tok) headers["authorization"] = `Bearer ${tok}`;
    const r = await f(`${baseUrl}/v1/apps/${app}/files/${encodeURIComponent(fileId(file))}/content`, { headers });
    if (r.status >= 400) { const j = await r.json().catch(() => null); throw new CoriError(r.status, j?.error ?? r.statusText); }
    return await r.blob();
  }
  /** The files a run's input names as browser Files (or Blobs), uploaded for workflow `id` (four at a time), replaced
   *  by their ids; every other value as it is. */
  async function withFiles(id: string, input: object, s?: StartOptions): Promise<object> {
    if (typeof Blob === "undefined") return input;
    const entries = Object.entries(input as Record<string, unknown>);
    const blobs: { key: string; index: number | null; blob: Blob }[] = [];
    for (const [k, v] of entries) {
      if (v instanceof Blob) blobs.push({ key: k, index: null, blob: v });
      else if (Array.isArray(v)) v.forEach((x, i) => { if (x instanceof Blob) blobs.push({ key: k, index: i, blob: x }); });
    }
    if (!blobs.length) return input;
    const total = blobs.reduce((n, b) => n + b.blob.size, 0);
    const sent = new Map<number, number>();
    const report = () => s?.onUploadProgress?.([...sent.values()].reduce((a, b) => a + b, 0), total);
    const ids: string[] = new Array(blobs.length);
    let next = 0;
    const worker = async () => {
      for (let i = next++; i < blobs.length; i = next++) {
        const b = blobs[i];
        ids[i] = (await upload(b.blob, { workflow: id, parameter: b.key, onProgress: s?.onUploadProgress ? (n) => { sent.set(i, n); report(); } : undefined })).id;
      }
    };
    await Promise.all(Array.from({ length: Math.min(4, blobs.length) }, worker));
    const out: Record<string, unknown> = Object.fromEntries(entries.map(([k, v]) => [k, Array.isArray(v) ? [...v] : v]));
    blobs.forEach((b, i) => { if (b.index === null) out[b.key] = ids[i]; else (out[b.key] as unknown[])[b.index] = ids[i]; });
    return out;
  }
  const toView = (j: any): RunView => ({ runId: j.run_id, workflow: j.workflow, status: j.status, error: j.error, errorDetail: j.error_detail ?? null, result: j.result ?? null, outputs: j.outputs, steps: j.steps, startedAt: j.started_at, endedAt: j.ended_at, pendingApproval: j.pending_approval ?? null });
  const runs = {
    async get(runId: string): Promise<RunView> { const { app } = await conf(); return toView((await req("GET", `/v1/apps/${app}/runs/${runId}`)).json); },
    async list(): Promise<RunView[]> { const { app } = await conf(); return ((await req("GET", `/v1/apps/${app}/runs`)).json as any[]).map(toView); },
    async decide(runId: string, step: string, value: unknown): Promise<void> { const { app } = await conf(); await req("POST", `/v1/apps/${app}/runs/${runId}/decisions/${step}`, value); },
    async wait(runId: string, signal?: AbortSignal): Promise<RunView> {
      for (;;) { const v = await runs.get(runId); if (v.status !== "running") return v; if (signal?.aborted) throw new Error("aborted"); await new Promise((r) => setTimeout(r, opts.pollMs ?? 500)); }
    },
  };
  const approvals = {
    /** Pending approvals routed to the caller's roles (or to the caller as the run initiator). */
    async list(): Promise<Approval[]> { const { app } = await conf(); return (await req("GET", `/v1/apps/${app}/approvals`)).json; },
    async decide(id: string, value: unknown): Promise<void> { const { app } = await conf(); await req("POST", `/v1/apps/${app}/approvals/${id}`, value); },
  };
  const client = {
    runs,
    approvals,
    /** The hosted config (`/__cori/config.json`), `{}` when the front is not served by Cori and no `baseUrl`/`app` were given. */
    config,
    /** The signed-in user as the server sees it. */
    /** The signed-in user in this app: `roles` are the app's roles (its client roles under builtin auth). */
    async me(): Promise<User> { const { app } = await conf(); return (await req("GET", app ? `/v1/apps/${app}/me` : "/v1/me")).json; },
    /** The bearer the next call would carry (a dev token under dev auth), for debugging. */
    token,
    /** Builtin auth: whether a user is signed in on this tab (after `completeLogin`). */
    async signedIn(): Promise<boolean> { const s = await session(); if (!s) return true; await s.completeLogin(); return (await s.token()) !== null; },
    /** Builtin auth: why the last sign-in on this tab was refused (the issuer's `error_description`), else null. */
    async signInError(): Promise<string | null> { const s = await session(); if (!s) return null; await s.completeLogin(); return s.error(); },
    /** Builtin auth: sends the browser to the issuer's sign-in page, and back to this page. */
    async login(): Promise<void> { const s = await session(); if (s) await s.login(); },
    /** Builtin auth: forgets the tokens and ends the issuer's session. */
    async logout(): Promise<void> { const s = await session(); if (s) await s.logout(); },
    async app() { const { app } = await conf(); return (await req("GET", `/v1/apps/${app}`)).json; },
    /** Uploads a file for a workflow's file parameter and returns its reference (`run()` / `start()` do it themselves
     *  for a File they are given). */
    upload,
    /** A file's reference and status. */
    async file(file: CoriFile | string): Promise<CoriFile> { const { app } = await conf(); return (await req("GET", `/v1/apps/${app}/files/${encodeURIComponent(fileId(file))}`)).json; },
    /** A file's bytes, as a Blob (to show an image, read a text). */
    fileBlob,
    /** Saves a file in the browser's downloads, under its name. */
    async download(file: CoriFile | string, name?: string): Promise<void> {
      const blob = await fileBlob(file);
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = name ?? (typeof file === "string" ? "file" : file.name);
      document.body.appendChild(a); a.click(); a.remove();
      setTimeout(() => URL.revokeObjectURL(a.href), 30_000);
    },
    workflow<I extends object, V = Record<string, unknown>, O = Record<string, unknown>>(id: string, o: { mode?: "sync" | "async" } = {}) {
      const startHeaders = (s?: StartOptions): Record<string, string> => (s?.idempotencyKey ? { "idempotency-key": s.idempotencyKey } : {});
      return {
        id,
        /** Start and return immediately with the run id. With `endpoint.idempotency: header`, pass the same `idempotencyKey`
         *  for retries of one submission (a double click, a reload) and the server replays the first run. */
        async start(input: I, s?: StartOptions): Promise<{ runId: string; replayed?: boolean }> { const { app, version } = await conf(); const r = await req("POST", wfPath(app, version, id), await withFiles(id, input, s), startHeaders(s)); return { runId: r.json.run_id, replayed: r.json.idempotent_replay === true || undefined }; },
        /** Start and wait for the result (sync endpoints answer in the same call; async ones are polled). */
        async run(input: I, s?: StartOptions): Promise<RunView<V, O>> {
          const { app, version } = await conf(); const r = await req("POST", wfPath(app, version, id), await withFiles(id, input, s), startHeaders(s));
          if (r.status === 200) return toView(r.json) as RunView<V, O>;
          return (await runs.wait(r.json.run_id)) as RunView<V, O>;
        },
        get: (runId: string) => runs.get(runId) as Promise<RunView<V, O>>,
        mode: o.mode ?? "async",
      };
    },
  };
  if (opts.devWidget !== false && typeof document !== "undefined") config().then((c) => { if (c.auth === "dev") loadDevWidget(c); }).catch(() => {});
  return client;
}

/** Loads the gateway's dev widget when the page does not already carry it (the server injects it into hosted fronts;
 *  a front served by its own dev server gets it from here). */
export function loadDevWidget(c: CoriConfig): void {
  if (!c.app || !c.baseUrl || document.querySelector("script[data-cori-dev-widget]") || document.getElementById("cori-dev-widget")) return;
  const s = document.createElement("script");
  s.src = `${c.baseUrl.replace(/\/$/, "")}/apps/${c.app}/__cori/dev-widget.js`; s.dataset.coriDevWidget = "";
  document.body.appendChild(s);
}

export interface Approval { id: string; run_id: string; workflow: string; step: string; to: string; requested_by: string; created_at: string; payload: { kind?: string; timeout?: string; payload: unknown } }

export class CoriError extends Error { constructor(public status: number, message: string) { super(message); this.name = "CoriError"; } }
export type CoriClient = ReturnType<typeof createCoriClient>;

// ---- sign-in under builtin auth: OpenID Connect authorization code with PKCE, as a public client --------------------

/** The issuer and the public client a browser signs in with (`oidc` in `/__cori/config.json`, `/v1/auth/config`).
 *  `resource` is the server's URL (RFC 8707): without it some issuers return an opaque token the server cannot verify. */
export interface OidcConfig { issuer: string; client_id: string; resource?: string;
  /** The app's fixed callback (`/apps/<app>/callback`) when it has its own client: the issuer answers there and the server
   *  sends the browser back to the page named in `state`. Without it the page itself is the redirect. */
  redirect_uri?: string;
  /** Scopes to ask for besides `openid offline_access`: the deployment's own (an issuer that ignores `resource` gives the
   *  token the audience of the scopes asked for). */
  scopes?: string[] }

export interface OidcSession {
  /** A valid access token (refreshed when about to expire), or null when nobody is signed in. */
  token(): Promise<string | null>;
  /** Redirects to the issuer's sign-in page; the browser comes back to this page (its hash kept). `params` go to the
   *  issuer with the authorization request (an issuer that knows organizations reads `organization`, the one to sign in
   *  to; it ignores what it does not know). */
  login(params?: Record<string, string>): Promise<void>;
  /** Finishes a sign-in when the URL carries the issuer's answer (`?code=…&state=…`); true when it did. A refusal
   *  (`?error=…&error_description=…&state=…`, RFC 6749 §4.1.2.1) is kept for `error`, and false. */
  completeLogin(): Promise<boolean>;
  /** Why the last sign-in on this tab was refused (its `error_description`, else its `error`), until the next one
   *  starts; null when it was not. */
  error(): string | null;
  /** Whether an automatic sign-in may start now: not when one started in the last minute and still gave no token
   *  (a refused or failed sign-in must not bounce the tab between the page and the issuer). Records the attempt. */
  mayAutoLogin(): boolean;
  logout(): Promise<void>;
}

const sessions = new Map<string, OidcSession>();
const b64url = (b: ArrayBuffer | Uint8Array) => btoa(String.fromCharCode(...new Uint8Array(b instanceof Uint8Array ? b : new Uint8Array(b)))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const randomString = () => b64url(crypto.getRandomValues(new Uint8Array(32)));

/**
 * One sign-in session per (issuer, client) and tab. The access token stays in memory; the refresh token in
 * sessionStorage (this tab only, gone when it closes), never in localStorage.
 */
export function oidcSession(o: OidcConfig): OidcSession {
  const id = `${o.issuer}|${o.client_id}`;
  const existing = sessions.get(id);
  if (existing) return existing;
  const key = `cori.oidc.${o.client_id}`;
  let access: { token: string; exp: number } | null = null;
  let discovery: Promise<any> | null = null;
  let completing: Promise<boolean> | null = null;
  // one refresh at a time: the issuer rotates refresh tokens and takes a reused one for a stolen one (it revokes them all)
  let refreshing: Promise<boolean> | null = null;
  const store = { get: (k: string) => { try { return sessionStorage.getItem(k); } catch { return null; } }, set: (k: string, v: string | null) => { try { if (v === null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch { /* storage blocked: memory only */ } } };
  const disco = () => (discovery ??= fetch(`${o.issuer.replace(/\/$/, "")}/.well-known/openid-configuration`).then((r) => { if (!r.ok) throw new Error(`issuer discovery: ${r.status}`); return r.json(); }));
  const here = () => `${location.origin}${location.pathname}`;
  const expOf = (t: string, expiresIn?: number) => { try { return JSON.parse(atob(t.split(".")[1].replace(/-/g, "+").replace(/_/g, "/"))).exp * 1000; } catch { return Date.now() + (expiresIn ?? 60) * 1000; } };
  // offline_access: a refresh token (sessionStorage), so a reload does not send the user through the issuer again
  const scope = [...new Set(["openid", "offline_access", ...(o.scopes ?? [])])].join(" ");
  async function grant(form: Record<string, string>): Promise<boolean> {
    const d = await disco();
    // a refresh asks for the deployment's scopes again, when it has some: the new token keeps its audience
    const again: Record<string, string> = form.grant_type === "refresh_token" && o.scopes?.length ? { scope } : {};
    const r = await fetch(d.token_endpoint, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ client_id: o.client_id, ...(o.resource ? { resource: o.resource } : {}), ...again, ...form }) });
    if (!r.ok) {
      // a refresh clears the stored refresh token only if it is still the one it presented (another call may have rotated it)
      if (!form.refresh_token || store.get(`${key}.refresh`) === form.refresh_token) { store.set(`${key}.refresh`, null); access = null; }
      return false;
    }
    const j = await r.json();
    access = { token: j.access_token, exp: expOf(j.access_token, j.expires_in) };
    if (j.refresh_token) store.set(`${key}.refresh`, j.refresh_token);
    if (j.id_token) store.set(`${key}.id`, j.id_token);   // id_token_hint: the issuer ends its session without asking again
    return true;
  }
  const session: OidcSession = {
    async token() {
      await session.completeLogin();
      if (access && access.exp - 30_000 > Date.now()) return access.token;
      const refresh = store.get(`${key}.refresh`);
      if (refresh && await (refreshing ??= grant({ grant_type: "refresh_token", refresh_token: refresh }).finally(() => { refreshing = null; }))) return access!.token;
      return null;
    },
    async login(params?: Record<string, string>) {
      const d = await disco();
      const verifier = randomString();
      const redirect = o.redirect_uri ?? here();
      // with a fixed callback, `state` also names the page to come back to (the server only accepts this app's pages)
      const state = o.redirect_uri ? `${randomString()}.${b64url(new TextEncoder().encode(location.pathname + location.search))}` : randomString();
      const challenge = b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)));
      store.set(`${key}.pkce`, JSON.stringify({ verifier, state, redirect, back: here(), hash: location.hash }));
      store.set(`${key}.error`, null);
      const q = new URLSearchParams({ ...(params ?? {}), response_type: "code", client_id: o.client_id, redirect_uri: redirect, scope, state, code_challenge: challenge, code_challenge_method: "S256", ...(o.resource ? { resource: o.resource } : {}) });
      location.assign(`${d.authorization_endpoint}?${q}`);
      await new Promise(() => {});   // the page is leaving
    },
    completeLogin() {
      return (completing ??= (async () => {
        if (typeof location === "undefined") return false;
        const q = new URLSearchParams(location.search);
        const code = q.get("code"), state = q.get("state"), refused = q.get("error");
        const pending = store.get(`${key}.pkce`);
        if (!(code || refused) || !state || !pending) return false;
        const p = JSON.parse(pending);
        store.set(`${key}.pkce`, null);
        if (p.state !== state) return false;
        if (!code) {
          // the issuer (or the page it sent the sign-in to) refused: its sentence, shown as text only
          store.set(`${key}.error`, (q.get("error_description") || refused!).slice(0, 500));
          history.replaceState(null, "", (p.back ?? p.redirect) + (p.hash || ""));
          return false;
        }
        const ok = await grant({ grant_type: "authorization_code", code, redirect_uri: p.redirect, code_verifier: p.verifier });
        history.replaceState(null, "", (p.back ?? p.redirect) + (p.hash || ""));   // drop ?code=… from the address bar
        if (ok) store.set(`${key}.auto`, null);
        return ok;
      })());
    },
    error() { return store.get(`${key}.error`); },
    mayAutoLogin() {
      if (typeof location === "undefined") return false;
      const last = Number(store.get(`${key}.auto`) ?? 0);
      if (Date.now() - last < 60_000) return false;
      store.set(`${key}.auto`, String(Date.now()));
      return true;
    },
    async logout() {
      const d = await disco().catch(() => ({}));
      const hint = store.get(`${key}.id`);
      access = null; store.set(`${key}.refresh`, null); store.set(`${key}.id`, null);
      store.set(`${key}.auto`, String(Date.now()));   // signed out on purpose: no automatic sign-in on the way back
      const back = o.redirect_uri ? o.redirect_uri.replace(/callback$/, "") : here();   // an app front: its registered root
      if (d.end_session_endpoint) location.assign(`${d.end_session_endpoint}?${new URLSearchParams({ client_id: o.client_id, post_logout_redirect_uri: back, ...(hint ? { id_token_hint: hint } : {}) })}`);
    },
  };
  sessions.set(id, session);
  return session;
}


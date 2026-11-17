// SPDX-License-Identifier: MIT-0
import { assert, assertEquals } from "jsr:@std/assert@1";
import { createCoriClient, currentDevIdentity, defaultDevIdentity, devToken, devTokenKey, getDevIdentity, oidcSession, onDevIdentityChange, parseDevToken, setDevIdentity } from "./index.ts";

Deno.test("dev tokens round-trip and tolerate JWTs", () => {
  assertEquals(parseDevToken("dev:alice:support_agent,support_lead"), { sub: "alice", roles: ["support_agent", "support_lead"] });
  assertEquals(parseDevToken("dev:bob:"), { sub: "bob", roles: [] });
  assertEquals(parseDevToken("dev::x"), { sub: "dev", roles: ["x"] });
  assertEquals(parseDevToken("eyJhbGciOi.eyJzdWIiOi.sig"), null);
  assertEquals(parseDevToken(null), null);
  assertEquals(devToken({ sub: "alice", roles: ["a", "b"] }), "dev:alice:a,b");
});

Deno.test("default identity prefers the front's roles", () => {
  assertEquals(defaultDevIdentity({ dev: { roles: ["a", "b", "c"], front_roles: ["a"], workflows: [], mocks: [] } }).roles, ["a"]);
  assertEquals(defaultDevIdentity({ dev: { roles: ["a", "b"], front_roles: [], workflows: [], mocks: [] } }).roles, ["a", "b"]);
  assertEquals(defaultDevIdentity({}), { sub: "dev", roles: [] });
});

Deno.test("the first persona is the default identity", () => {
  const personas = [{ sub: "alice", roles: ["member"], name: "Alice" }, { sub: "maya", roles: ["manager"], name: "Maya" }];
  const c = { app: "ts", dev: { roles: ["member", "manager"], front_roles: ["member", "manager"], personas, workflows: [], mocks: [] } };
  assertEquals(defaultDevIdentity(c), { sub: "alice", roles: ["member"] });
  assertEquals(currentDevIdentity(c), { sub: "alice", roles: ["member"] });
  setDevIdentity({ sub: "maya", roles: ["manager"] }, "ts");
  assertEquals(currentDevIdentity(c), { sub: "maya", roles: ["manager"] });
  setDevIdentity(null, "ts");
});

Deno.test("each app keeps its own identity", () => {
  setDevIdentity({ sub: "alice", roles: ["learner"] }, "academy");
  setDevIdentity({ sub: "maya", roles: ["manager"] }, "timesheets");
  assertEquals(localStorage.getItem(devTokenKey("academy")), "dev:alice:learner");
  assertEquals(getDevIdentity("academy"), { sub: "alice", roles: ["learner"] });
  assertEquals(getDevIdentity("timesheets"), { sub: "maya", roles: ["manager"] });
  setDevIdentity(null, "academy"); setDevIdentity(null, "timesheets");
});

Deno.test("chosen identity is kept under cori.tok.<app> and notifies listeners", () => {
  let n = 0; const off = onDevIdentityChange(() => n++);
  setDevIdentity({ sub: "sam", roles: ["support_lead"] }, "demo");
  assertEquals(localStorage.getItem("cori.tok.demo"), "dev:sam:support_lead");
  assertEquals(getDevIdentity("demo"), { sub: "sam", roles: ["support_lead"] });
  setDevIdentity(null, "demo");
  assertEquals(getDevIdentity("demo"), null);
  assertEquals(n, 2); off();
});

Deno.test("under dev auth the client signs calls with the chosen identity; never otherwise", async () => {
  const seen: string[] = [];
  const mk = (auth: string) => createCoriClient({ baseUrl: "http://gw", app: "demo", devWidget: false, fetch: (async (url: string | URL | Request, init?: RequestInit) => {
    const u = String(url);
    if (u.endsWith("/__cori/config.json")) return new Response(JSON.stringify({ app: "demo", baseUrl: "http://gw", auth, dev: { roles: ["r1"], front_roles: [], workflows: [], mocks: [] } }));
    seen.push(String((init?.headers as Record<string, string>)?.authorization ?? "")); return new Response(JSON.stringify({ sub: "x", roles: [] }));
  }) as typeof fetch });
  setDevIdentity({ sub: "sam", roles: ["support_lead"] }, "demo");
  setDevIdentity({ sub: "zed", roles: ["other"] }, "another-app");
  await mk("dev").me(); assertEquals(seen.pop(), "Bearer dev:sam:support_lead");
  setDevIdentity(null, "demo");
  await mk("dev").me(); assertEquals(seen.pop(), "Bearer dev:dev:r1");
  await mk("builtin").me(); assertEquals(seen.pop(), "");
});

Deno.test("a run under way shows its steps so far and the person it waits for", async () => {
  const c = createCoriClient({ baseUrl: "http://gw", app: "demo", devWidget: false, fetch: (async (url: string | URL | Request) => {
    if (String(url).endsWith("/__cori/config.json")) return new Response(JSON.stringify({ app: "demo", baseUrl: "http://gw", auth: "off" }));
    return new Response(JSON.stringify({ run_id: "r1", workflow: "w", status: "running", steps: [{ name: "record", kind: "store", status: "ok", ms: 3, cost_eur: 0, notes: [] }],
      pending_approval: { id: "a1", step: "approve", to: "approver", since: "2025-10-11T10:00:00Z" } }));
  }) as typeof fetch });
  const v = await c.runs.get("r1");
  assertEquals(v.steps?.map((s) => s.name), ["record"]);
  assertEquals(v.pendingApproval, { id: "a1", step: "approve", to: "approver", since: "2025-10-11T10:00:00Z" });
});

Deno.test("concurrent callers share one refresh: the issuer takes a reused refresh token for a stolen one", async () => {
  const realFetch = globalThis.fetch;
  let refreshes = 0, current = "r1";
  const jwt = (n: number) => `h.${btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600, n })).replace(/=+$/, "")}.s`;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input).endsWith("/.well-known/openid-configuration")) return Response.json({ token_endpoint: "https://idp.test/token", authorization_endpoint: "https://idp.test/authorize" });
    const form = new URLSearchParams(String(init?.body));
    if (form.get("grant_type") !== "refresh_token") return new Response("{}", { status: 400 });
    refreshes++;
    await new Promise((r) => setTimeout(r, 10));
    if (form.get("refresh_token") !== current) return new Response("{}", { status: 400 });   // reused: the family is revoked
    current = `r${refreshes + 1}`;
    return Response.json({ access_token: jwt(refreshes), refresh_token: current, expires_in: 3600 });
  }) as typeof fetch;
  try {
    sessionStorage.setItem("cori.oidc.single-flight.refresh", "r1");
    const s = oidcSession({ issuer: "https://idp.test", client_id: "single-flight" });
    const tokens = await Promise.all([s.token(), s.token(), s.token()]);
    assertEquals(refreshes, 1);
    assert(tokens[0] !== null && tokens.every((t) => t === tokens[0]), `${tokens}`);
    assertEquals(sessionStorage.getItem("cori.oidc.single-flight.refresh"), current);
  } finally {
    globalThis.fetch = realFetch;
  }
});

Deno.test("the deployment's scopes go with the sign-in and each refresh", async () => {
  const realFetch = globalThis.fetch, had = Object.getOwnPropertyDescriptor(globalThis, "location");
  const forms: URLSearchParams[] = [];
  let assigned = "";
  const jwt = `h.${btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600 })).replace(/=+$/, "")}.s`;
  Object.defineProperty(globalThis, "location", { value: { origin: "https://desk.test", pathname: "/", search: "", hash: "", assign: (u: string) => { assigned = u; } }, configurable: true, writable: true });
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input).endsWith("/.well-known/openid-configuration")) return Response.json({ token_endpoint: "https://idp.test/token", authorization_endpoint: "https://idp.test/authorize" });
    forms.push(new URLSearchParams(String(init?.body)));
    return Response.json({ access_token: jwt, refresh_token: "r2", expires_in: 3600 });
  }) as typeof fetch;
  try {
    // an issuer that gives the token the audience of the scopes asked for: Cori's own API scope, after the client's
    const s = oidcSession({ issuer: "https://idp.test", client_id: "scoped", resource: "https://cori.test", scopes: ["api://cori/access", "openid"] });
    s.login();   // the page leaves: it never settles
    for (let i = 0; i < 100 && !assigned; i++) await new Promise((r) => setTimeout(r, 1));
    assertEquals(new URL(assigned).searchParams.get("scope"), "openid offline_access api://cori/access");
    sessionStorage.setItem("cori.oidc.scoped.refresh", "r1");
    assert(await s.token());
    assertEquals(forms.pop()?.get("scope"), "openid offline_access api://cori/access");
    // none: a refresh names no scope, as before
    sessionStorage.setItem("cori.oidc.plain.refresh", "r1");
    assert(await oidcSession({ issuer: "https://idp.test", client_id: "plain" }).token());
    assertEquals(forms.pop()?.get("scope"), null);
  } finally {
    globalThis.fetch = realFetch;
    if (had) Object.defineProperty(globalThis, "location", had); else delete (globalThis as { location?: unknown }).location;
  }
});

Deno.test("a refused sign-in keeps its sentence for the page, and no other state's", async () => {
  const had = Object.getOwnPropertyDescriptor(globalThis, "location"), hadHistory = Object.getOwnPropertyDescriptor(globalThis, "history");
  let replaced = "";
  const at = (search: string) => Object.defineProperty(globalThis, "location", { value: { origin: "https://desk--v3.test", pathname: "/apps/desk/callback", search, hash: "" }, configurable: true, writable: true });
  Object.defineProperty(globalThis, "history", { value: { replaceState: (_: unknown, __: string, u: string) => { replaced = u; } }, configurable: true, writable: true });
  try {
    const s = oidcSession({ issuer: "https://desk--v3.test/__cori/oidc", client_id: "refused" });
    const pending = JSON.stringify({ verifier: "v", state: "st", redirect: "https://desk--v3.test/apps/desk/callback", back: "https://desk--v3.test/", hash: "#/x" });
    // another sign-in's state: nothing kept
    sessionStorage.setItem("cori.oidc.refused.pkce", pending);
    at("?error=access_denied&error_description=Nope&state=other");
    assertEquals(await s.completeLogin(), false);
    assertEquals(s.error(), null);
    // this one's: the sentence, the address cleaned
    const t = oidcSession({ issuer: "https://desk--v3.test/__cori/oidc", client_id: "refused-2" });
    sessionStorage.setItem("cori.oidc.refused-2.pkce", pending);
    at("?error=access_denied&error_description=You%20do%20not%20build%20desk.&state=st");
    assertEquals(await t.completeLogin(), false);
    assertEquals(t.error(), "You do not build desk.");
    assertEquals(replaced, "https://desk--v3.test/#/x");
    assertEquals(sessionStorage.getItem("cori.oidc.refused-2.pkce"), null);
  } finally {
    if (had) Object.defineProperty(globalThis, "location", had); else delete (globalThis as { location?: unknown }).location;
    if (hadHistory) Object.defineProperty(globalThis, "history", hadHistory); else delete (globalThis as { history?: unknown }).history;
  }
});

Deno.test("a File given for a file parameter is uploaded first, and the run gets its id", async () => {
  const calls: { method: string; url: string; body?: unknown; auth?: string | null }[] = [];
  const c = createCoriClient({ baseUrl: "http://gw", app: "desk", devWidget: false, getToken: () => "tok", fetch: (async (url: string | URL | Request, init?: RequestInit) => {
    const u = String(url), method = init?.method ?? "GET", h = new Headers(init?.headers);
    if (u.endsWith("/__cori/config.json")) return Response.json({ app: "desk", baseUrl: "http://gw", auth: "off" });
    calls.push({ method, url: u, auth: h.get("authorization"), body: init?.body instanceof Blob ? await init.body.text() : init?.body ? JSON.parse(String(init.body)) : undefined });
    if (method === "POST" && u.endsWith("/v1/apps/desk/files")) {
      const b = JSON.parse(String(init!.body)); const id = `file_${b.name.replace(/\W/g, "")}`;
      return Response.json({ id, name: b.name, size: b.size, content_type: b.content_type, status: "pending", upload: { method: "PUT", url: `/v1/apps/desk/files/${id}/content` } }, { status: 201 });
    }
    if (method === "PUT") { const id = u.split("/")[7]; return Response.json({ id, name: "x", size: 3, content_type: "application/pdf", sha256: "ab", status: "ready" }); }
    if (u.endsWith("/wf/claim/runs")) return Response.json({ run_id: "r1", status: "succeeded", result: null });
    return new Response("{}", { status: 404 });
  }) as typeof fetch });
  const progress: number[] = [];
  const w = c.workflow<{ title: string; receipt: unknown; extra?: unknown[] }>("claim");
  const v = await w.run({ title: "Taxi", receipt: new File(["%PDF"], "receipt.pdf", { type: "application/pdf" }), extra: [new File(["a"], "a.png"), "file_kept"] }, { onUploadProgress: (sent) => progress.push(sent) });
  assertEquals(v.status, "succeeded");
  const run = calls.find((x) => x.url.endsWith("/wf/claim/runs"))!;
  assertEquals(run.body, { title: "Taxi", receipt: "file_receiptpdf", extra: ["file_apng", "file_kept"] });
  const announced = calls.filter((x) => x.method === "POST" && x.url.endsWith("/files")).map((x) => x.body);
  assertEquals(announced, [{ workflow: "claim", parameter: "receipt", name: "receipt.pdf", size: 4, content_type: "application/pdf" }, { workflow: "claim", parameter: "extra", name: "a.png", size: 1 }]);
  const puts = calls.filter((x) => x.method === "PUT");
  assertEquals(puts.map((x) => [x.url, x.body, x.auth]), [["http://gw/v1/apps/desk/files/file_receiptpdf/content", "%PDF", "Bearer tok"], ["http://gw/v1/apps/desk/files/file_apng/content", "a", "Bearer tok"]]);
  assert(progress.length > 0 && progress[progress.length - 1] === 5, `${progress}`);
});

Deno.test("an upload to another host never carries the bearer", async () => {
  const seen: (string | null)[] = [];
  const c = createCoriClient({ baseUrl: "http://gw", app: "desk", devWidget: false, getToken: () => "tok", fetch: (async (url: string | URL | Request, init?: RequestInit) => {
    const u = String(url);
    if (u.endsWith("/__cori/config.json")) return Response.json({ app: "desk", baseUrl: "http://gw", auth: "off" });
    if (init?.method === "POST") return Response.json({ id: "file_1", name: "a.pdf", size: 1, content_type: "application/pdf", status: "pending", upload: { method: "PUT", url: "https://bucket.example/signed?x=1" } }, { status: 201 });
    seen.push(new Headers(init?.headers).get("authorization"));
    return new Response(null, { status: 200 });
  }) as typeof fetch });
  const f = await c.upload(new Blob(["a"]), { workflow: "w", parameter: "p", name: "a.pdf" });
  assertEquals(seen, [null]);
  assertEquals(f.id, "file_1");
});

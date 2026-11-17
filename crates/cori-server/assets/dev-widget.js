// SPDX-License-Identifier: Apache-2.0
// The dev widget. Under dev auth (`cori up`) the server injects this script into every hosted front's index.html and
// serves it at `<front>/__cori/dev-widget.js`; @cori-do/client loads it from the gateway when the front is served
// elsewhere (a Vite dev server). It switches the dev identity (`dev:<sub>:<roles>` in localStorage `cori.tok.<app>`,
// the key @cori-do/client reads: one identity per app, so apps on one origin never see each other's), offers the app's
// `[[dev.personas]]` and shows the app as the server sees it. Plain script in a shadow root: no framework.
(() => {
  if (typeof document === "undefined") return;
  if (document.querySelector('meta[name="cori-dev-widget"][content="off"]')) return;
  const script = document.currentScript;
  // the app this widget serves: the tag the server writes names it (a front at the root of its own host), else its url
  // does (`<gateway>/apps/<app>/[@<hash>/]__cori/dev-widget.js`)
  const APP = (script && script.dataset && script.dataset.coriApp) || (((script && script.src) || location.href).match(/\/apps\/([^/@][^/]*)\//) || [])[1] || "";
  const HOST_ID = "cori-dev-widget", TOKEN_KEY = APP ? `cori.tok.${APP}` : "cori.tok", AUTO_KEY = `cori.dev.auto.${APP}`, OPEN_KEY = "cori.dev.open", EVENT = "cori-dev-identity";
  const configUrl = new URL("config.json", script && script.src ? script.src : location.href).href;
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const flag = (key, v) => { try { if (v !== undefined) localStorage.setItem(key, v ? "1" : "0"); return localStorage.getItem(key) === "1"; } catch { return false; } };
  const parse = (t) => { if (!t || !t.startsWith("dev:")) return null; const i = t.indexOf(":", 4); const sub = i < 0 ? t.slice(4) : t.slice(4, i); const roles = i < 0 ? "" : t.slice(i + 1); return { sub: sub || "dev", roles: roles.split(",").map((r) => r.trim()).filter(Boolean) }; };
  const token = (id) => `dev:${id.sub || "dev"}:${id.roles.join(",")}`;
  // a token the widget wrote itself (the default identity, materialised) is not a choice: AUTO_KEY remembers it
  const chosen = () => { try { const t = localStorage.getItem(TOKEN_KEY); return t && t !== localStorage.getItem(AUTO_KEY) ? parse(t) : null; } catch { return null; } };
  // the identity also travels as a cookie, for fronts that send no Authorization header at all (the gateway reads it under dev auth)
  const cookie = (id) => { document.cookie = `cori_dev=${id ? encodeURIComponent(token(id)) : ""}; path=/; SameSite=Lax${id ? "" : "; max-age=0"}`; };
  // the default identity goes to `cori.tok` too, so a page reading it and the widget agree on who you are; a
  // hand-written page that already called with another token is reloaded once (the value is then stable: no loop)
  const materialise = (id, isChosen) => {
    cookie(id);
    if (isChosen) return;
    const t = token(id);
    try {
      const before = localStorage.getItem(TOKEN_KEY);
      if (before === t) { localStorage.setItem(AUTO_KEY, t); return; }
      localStorage.setItem(TOKEN_KEY, t); localStorage.setItem(AUTO_KEY, t);
      if (localStorage.getItem(TOKEN_KEY) !== t) return;
    } catch { return; }
    if (window.dispatchEvent(new CustomEvent(EVENT, { cancelable: true }))) location.reload();
  };
  const choose = (id) => {
    try { localStorage.removeItem(AUTO_KEY); if (id) localStorage.setItem(TOKEN_KEY, token(id)); else localStorage.removeItem(TOKEN_KEY); } catch {}
    cookie(id);
    // a page with @cori-do/client refreshes its hooks and cancels the event; a hand-written page needs a reload to call again
    if (window.dispatchEvent(new CustomEvent(EVENT, { cancelable: true }))) location.reload();
  };

  const CSS = `
:host { all: initial; }
.pill, .panel { position: fixed; right: 16px; bottom: 16px; z-index: 2147483000; font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; color: #e6e6e6; }
.pill { background: #1f2430; border: 1px solid #3b4252; border-radius: 999px; padding: 6px 12px; cursor: pointer; box-shadow: 0 4px 16px rgba(0,0,0,.35); display: flex; gap: 8px; align-items: center; }
.pill b { color: #8fd3ff; }
.panel { width: min(420px, calc(100vw - 32px)); max-height: min(80vh, 640px); overflow: auto; background: #1f2430; border: 1px solid #3b4252; border-radius: 10px; box-shadow: 0 8px 32px rgba(0,0,0,.45); padding: 12px 14px; box-sizing: border-box; }
.hd { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
.hd b { color: #8fd3ff; font-size: 13px; }
button { background: #2e3440; color: #e6e6e6; border: 1px solid #4c566a; border-radius: 6px; padding: 3px 9px; cursor: pointer; font: inherit; }
button.primary { background: #3b6ea5; border-color: #3b6ea5; }
h4 { margin: 12px 0 4px; font: 600 11px/1.2 inherit; color: #a3be8c; text-transform: uppercase; letter-spacing: .06em; }
.muted { color: #8a93a6; } .warn { color: #ebcb8b; } .err { color: #ff8a8a; } .ok { color: #a3be8c; }
input[type=text] { background: #2e3440; color: #e6e6e6; border: 1px solid #4c566a; border-radius: 6px; padding: 3px 6px; font: inherit; }
label.role { display: inline-flex; align-items: center; gap: 4px; margin: 2px 10px 2px 0; cursor: pointer; }
.roles { margin: 4px 0; }
.row { display: flex; gap: 6px; align-items: center; margin: 6px 0; flex-wrap: wrap; }
table { border-collapse: collapse; width: 100%; } td { padding: 2px 6px 2px 0; vertical-align: top; } td:first-child { white-space: nowrap; }
code { background: #2e3440; padding: 1px 4px; border-radius: 4px; word-break: break-all; }
`;

  function mount() {
    if (document.getElementById(HOST_ID)) return;
    const host = document.createElement("div"); host.id = HOST_ID;
    const root = host.attachShadow({ mode: "open" });
    const style = document.createElement("style"); style.textContent = CSS; root.appendChild(style);
    const el = document.createElement("div"); root.appendChild(el);
    document.body.appendChild(host);

    let cfg = {}, me = {}, approvals = null, approvalsError = null, lastError = null, draft = null, extraRoles = [];
    // the default identity, as @cori-do/client computes it: the app's first persona, else `dev` with the front's roles
    const personas = () => (cfg.dev && cfg.dev.personas) || [];
    const dflt = () => { const p = personas()[0]; if (p) return { sub: p.sub, roles: p.roles }; const d = cfg.dev; return { sub: "dev", roles: d && d.front_roles && d.front_roles.length ? d.front_roles : (d && d.roles) || [] }; };
    const same = (a, b) => a.sub === b.sub && a.roles.length === b.roles.length && a.roles.every((r) => b.roles.includes(r));
    const persona = (id) => personas().find((p) => same(p, id));
    const effective = () => chosen() || dflt();
    const may = (roles, id) => roles.length === 0 || roles.some((r) => id.roles.includes(r));
    const rawFetch = window.fetch.bind(window);                     // the widget's own calls: not reported as the page's refusals
    const api = async (path) => { const r = await rawFetch(`${(cfg.baseUrl || "").replace(/\/$/, "")}${path}`, { headers: { authorization: `Bearer ${token(effective())}` } }); const j = await r.json().catch(() => null); if (!r.ok) throw new Error((j && j.error) || r.statusText); return j; };

    async function load() {
      try { cfg = await (await fetch(configUrl)).json(); } catch { cfg = {}; }
      if (cfg.auth !== "dev") { host.remove(); return; }           // never under builtin or off auth
      materialise(effective(), chosen() !== null);                  // the identity (chosen or default): cookie for token-less fronts, `cori.tok` for the rest
      try { const m = await api("/v1/me"); me = { sub: m.sub, roles: m.roles, kind: m.kind }; } catch (e) { me = { error: e.message }; }
      try { approvals = (await api(`/v1/apps/${cfg.app}/approvals`)).length; approvalsError = null; } catch (e) { approvals = null; approvalsError = e.message; }
      render();
    }

    // the page's own calls to the gateway: a refusal (401/403) shows its reason here, since the page may swallow it
    const base = () => (cfg.baseUrl || location.origin).replace(/\/$/, "");
    if (window.fetch && !window.fetch.__coriDev) {
      const orig = window.fetch;
      const wrapped = async function (input, init) {
        const r = await orig.apply(this, arguments);
        try {
          const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
          const abs = new URL(url, location.href).href;
          if ((r.status === 401 || r.status === 403) && abs.startsWith(base()) && new URL(abs).pathname.startsWith("/v1/")) {
            const j = await r.clone().json().catch(() => null);
            lastError = { status: r.status, path: new URL(abs).pathname, error: (j && j.error) || r.statusText };
            render();
          }
        } catch {}
        return r;
      };
      wrapped.__coriDev = true;
      window.fetch = wrapped;
    }

    function render() {
      const id = effective(), isChosen = chosen() !== null, as = persona(id);
      if (!flag(OPEN_KEY)) {
        el.innerHTML = `<div class="pill" title="${esc(lastError ? `${lastError.status} ${lastError.path}: ${lastError.error}` : "Cori dev mode — click to open")}"><b>cori</b> dev · ${esc(as ? as.name : id.sub)} · ${esc(id.roles.join(",") || "no role")}${lastError ? ` · <span class="err">${esc(lastError.status)}</span>` : ""}</div>`;
        el.querySelector(".pill").onclick = () => { flag(OPEN_KEY, true); render(); };
        return;
      }
      const d = draft || { sub: id.sub, roles: [...id.roles] };
      const dev = cfg.dev || {};
      const known = Array.from(new Set([...(dev.roles || []), ...extraRoles, ...d.roles]));
      const front = dev.front_roles || [];
      const frontWarn = front.length > 0 && !may(front, id) ? `<div class="warn">this identity has none of the front's roles (${esc(front.join(", "))}); the gateway will refuse it once front permissions are enforced</div>` : "";
      const wfs = (dev.workflows || []).map((w) => {
        const ep = w.endpoint, callable = ep ? (ep.auth === "sso" ? may(ep.roles, id) : true) : false;
        return `<tr><td><b>${esc(w.id)}</b></td><td>${ep ? `${esc(ep.mode)} · ${esc(ep.auth)} · <span class="muted">${esc(ep.roles.join(", ") || "any role")}</span>` : `<span class="muted">${w.schedule ? `schedule ${esc(w.schedule)}` : "not exposed"}</span>`}</td><td>${ep ? (callable ? `<span class="ok">✓</span>` : `<span class="err">✗</span>`) : ""}</td></tr>`;
      }).join("");
      el.innerHTML = `<div class="panel">
        <div class="hd"><b>cori · dev mode</b><span class="muted">${esc(cfg.app || "")}${cfg.preview ? ` · preview ${esc(String(cfg.version || "").replace(/^sha256:/, "").slice(0, 12))}` : ""}</span><button id="close" title="collapse">–</button></div>
        <h4>Impersonate</h4>
        ${personas().length ? `<div class="row">${personas().map((p, i) => `<button data-persona="${i}" class="${same(p, id) ? "primary" : ""}" title="${esc(`${p.sub} · ${p.roles.join(",")}`)}">${esc(p.name)}${p.title ? ` <span class="muted">· ${esc(p.title)}</span>` : ""}</button>`).join("")}</div>` : `<div class="muted">no persona declared: add <code>[[dev.personas]] sub, roles, name</code> to cori.toml</div>`}
        <div class="row"><span class="muted">user</span> <input type="text" id="sub" value="${esc(d.sub)}" size="14"></div>
        <div class="roles">${known.map((r) => `<label class="role"><input type="checkbox" data-role="${esc(r)}" ${d.roles.includes(r) ? "checked" : ""}> ${esc(r)}</label>`).join("") || `<span class="muted">no role declared; add one below</span>`}</div>
        <div class="row"><input type="text" id="newrole" placeholder="add a role…" size="14"><button id="addrole">+</button></div>
        <div class="row"><button id="apply" class="primary">Apply</button><button id="reset" title="forget the chosen identity and use the default">Reset</button><span class="muted">${isChosen ? "chosen on this browser" : "default identity"}</span></div>
        <div class="muted">kept for this app only (<code>${esc(TOKEN_KEY)}</code>); the app's calls carry it as a bearer (@cori-do/client) or as the <code>cori_dev</code> cookie; Apply reloads a page without the client</div>
        <div class="muted">token <code>${esc(token(d))}</code></div>
        ${frontWarn}
        ${lastError ? `<div class="err">last refusal: ${esc(lastError.status)} <code>${esc(lastError.path)}</code> ${esc(lastError.error)}</div>` : ""}
        <h4>Seen by the server</h4>
        <div>${me.error ? `<span class="err">${esc(me.error)}</span>` : `<b>${esc(me.sub || "")}</b> · ${esc((me.roles || []).join(", ") || "no role")} <span class="muted">· ${esc(me.kind || "")}</span>`}</div>
        <div class="muted">auth ${esc(cfg.auth || "?")} · gateway <code>${esc(cfg.baseUrl || "")}</code></div>
        <h4>Workflows</h4>
        ${wfs ? `<table>${wfs}</table>` : `<div class="muted">none promoted</div>`}
        <h4>Approvals</h4>
        <div>${approvals === null ? `<span class="muted">not readable with this identity${approvalsError ? `: ${esc(approvalsError)}` : ""}</span>` : `${approvals} pending for these roles`} <button id="refresh" style="margin-left:6px">refresh</button></div>
        <h4>Mocked connectors</h4>
        <div>${(dev.mocks || []).length ? esc(dev.mocks.join(", ")) : `<span class="muted">none (real transports)</span>`}</div>
        <div class="muted" style="margin-top:10px">${esc(dev.hint || "Authorization: Bearer dev:<sub>:<role,role>")}</div>
      </div>`;
      const q = (sel) => el.querySelector(sel);
      const readDraft = () => ({ sub: q("#sub").value.trim() || "dev", roles: Array.from(el.querySelectorAll("input[data-role]")).filter((c) => c.checked).map((c) => c.dataset.role) });
      q("#close").onclick = () => { flag(OPEN_KEY, false); render(); };
      q("#apply").onclick = () => { draft = null; choose(readDraft()); };
      q("#reset").onclick = () => { draft = null; choose(null); };
      for (const b of el.querySelectorAll("button[data-persona]")) b.onclick = () => { const p = personas()[Number(b.dataset.persona)]; draft = null; choose({ sub: p.sub, roles: p.roles }); };
      q("#refresh").onclick = () => load();
      q("#addrole").onclick = () => { const r = q("#newrole").value.trim(); if (!r) return; draft = readDraft(); if (!extraRoles.includes(r)) extraRoles.push(r); if (!draft.roles.includes(r)) draft.roles.push(r); render(); };
      q("#newrole").onkeydown = (e) => { if (e.key === "Enter") q("#addrole").click(); };
      q("#sub").oninput = () => { draft = readDraft(); };
      for (const c of el.querySelectorAll("input[data-role]")) c.onchange = () => { draft = readDraft(); };
    }

    window.addEventListener(EVENT, () => { draft = null; lastError = null; load(); });
    window.addEventListener("storage", (e) => { if (e.key === TOKEN_KEY) { draft = null; load(); } });
    // the `cori_dev` cookie is one per origin: a tab of another app may have rewritten it, so this page takes it back on focus
    window.addEventListener("focus", () => { if (cfg.auth === "dev") cookie(effective()); });
    render();
    load();
  }

  if (document.body) mount(); else document.addEventListener("DOMContentLoaded", mount);
})();

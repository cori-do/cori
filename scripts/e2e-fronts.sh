#!/usr/bin/env bash
# Fronts on hosts of their own (CORI_FRONTS_URL), in a real browser. Two apps' fronts, each on its own host, and the console
# on the server's: beta keeps a secret in its storage, and a script on front alpha tries to reach it and the console
# (framing, reading, calling), and the platform's routes through alpha's own host. Everything but alpha's own app is out
# of its reach, and a release's page (`<app>--v<n>`) reads neither the live front's storage nor another release's. Then
# the control: the same fronts on the server's origin (no CORI_FRONTS_URL), where alpha does read beta's secret and the
# release the live front's mark, so the probes are known to see a leak when there is one. Headers are checked with curl, and the console
# renders under its content security policy with no violation.
# Needs a Temporal dev server on 127.0.0.1:7233, Postgres's pg_ctl (`cori up` starts its own), and Chrome or Chromium
# (CHROME names it; macOS: Google Chrome.app). `*.localhost` resolves to this machine in Chrome and curl.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)
P=${CORI_E2E_PORT:-8097}; S=http://127.0.0.1:$P
FA=http://alpha.localhost:$P; FB=http://beta.localhost:$P
CHROME=${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser || echo "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")}
[ -x "$CHROME" ] || { echo "e2e-fronts: no Chrome (set CHROME)" >&2; exit 1; }
if lsof -nP -iTCP:$P -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-fronts: port $P is taken" >&2; exit 1; fi
T=dev:jean:it_admin

# two apps, their fronts plain pages: beta keeps a secret in its storage; alpha probes what it can reach (keys of their own:
# under dev auth the widget keeps the identity in `cori.tok.<app>` and reloads the page when it changes)
app() {
  env -u CORI_SERVER -u CORI_TOKEN HOME="$W" "$B" init "$W/$1" --no-register --no-git --front plain >/dev/null
  "$B" workflow new ping --dir "$W/$1" >/dev/null   # a version needs a workflow: one code step
  sed_i 's/^steps: \[\]/steps: [pong]/' "$W/$1/workflows/ping/workflow.yaml"
  cat > "$W/$1/workflows/ping/steps/pong.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "pong", description: "Answer", input: (_ctx: Ctx<"pong">) => ({}), output: z.object({ ok: z.boolean() }), run: () => ({ ok: true }) });
TS
}
app alpha; app beta
cat > "$W/beta/dist/index.html" <<'HTML'
<!doctype html><html><head><meta charset="utf-8"><title>beta</title></head><body><p id="b">beta's page</p>
<script>localStorage.setItem("secret.beta", "beta-secret"); sessionStorage.setItem("secret.beta", "beta-session-secret");</script>
</body></html>
HTML
# the probe finds beta and the console where they are: on hosts of their own, or under the same origin as itself
cat > "$W/alpha/dist/index.html" <<HTML
<!doctype html><html><head><meta charset="utf-8"><title>alpha</title></head><body><pre id="out">running</pre>
<script>
const own = location.hostname.startsWith("alpha.");
const beta = own ? location.origin.replace("//alpha.", "//beta.") + "/" : "/apps/beta/";
const consoleUrl = own ? "$S/" : "/";
const r = {};
// "denied" when the browser refuses (an exception), "read:<value>" when it answers
const reach = async (f) => { try { return "read:" + String(await f()); } catch (e) { return "denied"; } };
const frame = (src) => new Promise((done) => { const f = document.createElement("iframe"); f.src = src; document.body.appendChild(f); setTimeout(() => done(f), 1500); });
(async () => {
  localStorage.setItem("probe.alpha", "alpha-own"); r.own_storage = localStorage.getItem("probe.alpha");
  r.own_api = (await fetch("/v1/auth/config")).status;
  r.config_base = (await (await fetch(own ? "/__cori/config.json" : "/apps/alpha/__cori/config.json")).json()).baseUrl;
  r.other_app_api = (await fetch("/v1/apps/beta")).status;
  r.other_app_front = (await fetch("/apps/beta/")).status;
  r.platform_route = (await fetch("/v1/secrets")).status;
  r.mcp_route = (await fetch("/mcp", { method: "POST" })).status;
  r.root = new URL((await fetch("/")).url).pathname;
  r.fetch_beta = await reach(async () => (await fetch(beta)).status);
  r.fetch_console_api = await reach(async () => (await fetch(consoleUrl + "v1/me", { credentials: "include" })).status);
  const fb = await frame(beta);
  r.beta_storage = await reach(() => fb.contentWindow.localStorage.getItem("secret.beta"));
  r.beta_session = await reach(() => fb.contentWindow.sessionStorage.getItem("secret.beta"));
  r.beta_dom = await reach(() => fb.contentDocument.getElementById("b").textContent);
  const fc = await frame(consoleUrl);
  r.console_storage = await reach(() => fc.contentWindow.localStorage.length);
  r.console_dom = await reach(() => fc.contentDocument.body.innerHTML.length);
  document.getElementById("out").textContent = JSON.stringify(r);
})().catch((e) => { document.getElementById("out").textContent = "probe failed: " + e; });
</script></body></html>
HTML

# the page's DOM once its scripts settled (virtual time), in <out>; Chrome may linger after printing it: it is stopped then
dump() {  # <profile> <url> <out> [chrome args…]
  local out=$3 pid; : > "$out"
  "$CHROME" --headless=new --disable-gpu --no-first-run --no-default-browser-check --disable-background-networking --disable-extensions \
    --disable-component-update --disable-sync --user-data-dir="$W/chrome-$1" --virtual-time-budget=10000 "${@:4}" --dump-dom "$2" >"$out" 2>>"$W/chrome-$1.log" & pid=$!
  for i in $(seq 1 120); do grep -q '</html>' "$out" && break; kill -0 $pid 2>/dev/null || break; sleep 0.5; done
  kill $pid 2>/dev/null || true; wait $pid 2>/dev/null || true
}
# beta's page first, in the profile alpha's probe then runs in: beta's secret is in the browser's storage for its origin
probe() {  # <profile> <beta's url> <alpha's url> <out>
  dump "$1" "$2" "$W/$1-beta.html"; has "beta's page" "$W/$1-beta.html"
  dump "$1" "$3" "$4"
}
up() {  # <data dir> [env…]: `cori up` and both apps deployed and promoted
  env "${@:2}" $B up --listen 127.0.0.1:$P --data-dir "$W/$1" --no-store >"$W/$1.log" 2>&1 & UP=$!
  for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
  curl -sf $S/healthz >/dev/null || { cat "$W/$1.log" >&2; exit 1; }
  for a in alpha beta; do
    $B apps create $a --server $S --token $T | has "created"
    H=$($B deploy "$W/$a" --server $S --token $T --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
    $B promote $a "$H" --server $S --token $T | has "promoted"
  done
}
down() { kill -INT $UP 2>/dev/null || true; wait $UP 2>/dev/null || true; }
trap down EXIT

up fronts CORI_FRONTS_URL="http://*.localhost:$P"
# an app whose front has a host of its own has an id that is a DNS label
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/apps -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{"id":"Not_A_Label"}')" = 400
# the console's links point at the root of each front's host
curl -sf $S/v1/apps/alpha -H "Authorization: Bearer $T" | has "\"front_url\":\"$FA/\""

# --- headers, and who serves what (curl names the host itself)
hdr() { curl -s -o /dev/null -D - "$@" | tr -d '\r'; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
# the server's own host: the console, never a front
hdr $S/ | has -i "content-security-policy: default-src 'self'; script-src 'sha256-"
hdr $S/ | has -i "frame-ancestors 'none'"
hdr $S/ | has -i "x-frame-options: DENY"
hdr $S/ | has -i "x-content-type-options: nosniff"
hdr $S/apps/alpha/tickets?t=1 | has -i "^location: $FA/tickets?t=1$"
test "$(code $S/apps/alpha/)" = 308
# a front's host: its front at the root (framed by itself only), its runtime API, nothing of the platform
hdr -H "Host: alpha.localhost:$P" $S/ | has -i "content-security-policy: frame-ancestors 'self'; base-uri 'self'; object-src 'none'"
hdr -H "Host: alpha.localhost:$P" $S/ | has -i "referrer-policy: strict-origin-when-cross-origin"
test "$(code -H "Host: alpha.localhost:$P" $S/)" = 200
curl -sf -H "Host: alpha.localhost:$P" $S/healthz | has "<title>alpha</title>"   # the app's path there, not the server's
# older links under /apps/<app>/ move to the root, the query kept; the sign-in callback keeps the path its issuer knows
hdr -H "Host: alpha.localhost:$P" "$S/apps/alpha/tickets/42?tab=notes" | has -i "^location: /tickets/42?tab=notes$"
test "$(code -H "Host: alpha.localhost:$P" $S/apps/alpha/)" = 308
hdr -H "Host: alpha.localhost:$P" "$S/apps/alpha/callback?code=c&state=s" | has -i "^location: /?code=c&state=s$"
test "$(code -H "Host: alpha.localhost:$P" $S/v1/auth/config)" = 200
for p in /v1/secrets /v1/apps /apps/beta/ /v1/apps/beta; do test "$(code -H "Host: alpha.localhost:$P" $S$p)" = 404 || { echo "e2e-fronts: $p served on alpha's host" >&2; exit 1; }; done
test "$(code -X POST -H "Host: alpha.localhost:$P" $S/mcp)" = 404
test "$(code -H "Host: nosuch.localhost:$P" $S/)" = 404
curl -sf -H "Host: alpha.localhost:$P" $S/__cori/config.json | has "\"baseUrl\":\"$FA\""
curl -sf -H "Host: alpha.localhost:$P" $S/ | has 'src="/__cori/dev-widget.js" data-cori-app="alpha"'

# --- each release at the root of a host of its own (`<app>--v<n>`), on its sandbox, whether or not it is live
RA=alpha--v1.localhost:$P
curl -sf $S/v1/apps/alpha/versions -H "Authorization: Bearer $T" | has "\"preview_url\":\"http://$RA/\""
test "$(code -H "Host: $RA" $S/)" = 200
hdr -H "Host: $RA" $S/ | has -i "^x-robots-tag: noindex"
curl -sf -H "Host: $RA" $S/__cori/config.json | python3 -c "import sys,json; c=json.load(sys.stdin); assert c['baseUrl'] == 'http://$RA' and c['preview'] is True and c['version'].startswith('sha256:'), c"
# a preview link of the live host or the server's goes on to the release's host, the path and query kept
hdr -H "Host: alpha.localhost:$P" "$S/apps/alpha/@v1/x?y=1" | has -i "^location: http://$RA/x?y=1$"
hdr "$S/apps/alpha/@v1/" | has -i "^location: http://$RA/$"
# a release's host serves its own release only, at its root
test "$(code -H "Host: $RA" $S/apps/alpha/@v1/)" = 404
test "$(code -H "Host: alpha--v9.localhost:$P" $S/)" = 404
hdr -H "Host: $RA" $S/apps/alpha/x | has -i "^location: /x$"
# its runs are its release's: a run started there is a preview, which the live host never lists, and the reverse
RUNR=$(curl -sf -X POST -H "Host: $RA" $S/v1/apps/alpha/wf/ping/runs -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
RUNL=$(curl -sf -X POST -H "Host: alpha.localhost:$P" $S/v1/apps/alpha/wf/ping/runs -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
curl -sf -H "Host: $RA" $S/v1/apps/alpha/runs -H "Authorization: Bearer $T" | has "$RUNR"
curl -sf -H "Host: $RA" $S/v1/apps/alpha/runs -H "Authorization: Bearer $T" | lacks "$RUNL"
curl -sf -H "Host: alpha.localhost:$P" $S/v1/apps/alpha/runs -H "Authorization: Bearer $T" | lacks "$RUNR"
test "$(code -H "Host: alpha.localhost:$P" $S/v1/apps/alpha/runs/$RUNR -H "Authorization: Bearer $T")" = 404
test "$(code -H "Host: $RA" $S/v1/apps/alpha/runs/$RUNL -H "Authorization: Bearer $T")" = 404
curl -sf $S/v1/apps/alpha/runs -H "Authorization: Bearer $T" | has "$RUNR"   # the console sees both
# the version route is a release's own, on its own host
test "$(code -X POST -H "Host: alpha.localhost:$P" "$S/v1/apps/alpha/versions/v1/wf/ping/runs" -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{}')" = 404

# --- in a browser: alpha's script reaches its own app only
probe isolated "$FB/" "$FA/" "$W/alpha.html"
result() { python3 - "$1" <<'PY'
import html, json, re, sys
m = re.search(r'<pre id="out">(.*?)</pre>', open(sys.argv[1]).read(), re.S)
assert m and m.group(1).startswith("{"), (m.group(1) if m else open(sys.argv[1]).read()[:1500])
print(html.unescape(m.group(1)))
PY
}
python3 - "$(result "$W/alpha.html")" "$FA" <<'PY'
import json, sys
r, fa = json.loads(sys.argv[1]), sys.argv[2]
want = {
  "own_storage": "alpha-own", "own_api": 200, "config_base": fa, "root": "/",
  "other_app_api": 404, "other_app_front": 404, "platform_route": 404, "mcp_route": 404,
  "fetch_beta": "denied", "fetch_console_api": "denied",
  "beta_storage": "denied", "beta_session": "denied", "beta_dom": "denied", "console_storage": "denied", "console_dom": "denied",
}
bad = {k: (r.get(k), v) for k, v in want.items() if r.get(k) != v}
assert not bad, f"front alpha reached more than its own app: {bad}"
PY

# --- in a browser: a release's page reads neither the live front's storage nor another release's (the live front and
# release v1's host keep alpha's mark in their storage; release v2's page looks for it)
release_probe() {  # deploys alpha's release v2, whose page probes the live front and release v1
  rm -rf "$W/alpha-v2"; cp -R "$W/alpha" "$W/alpha-v2"
  cat > "$W/alpha-v2/dist/index.html" <<'HTML'
<!doctype html><html><head><meta charset="utf-8"><title>alpha v2</title></head><body><pre id="out">running</pre>
<script>
const own = location.hostname.startsWith("alpha--v2.");
const at = (label, path) => own ? location.origin.replace("//alpha--v2.", "//" + label + ".") + "/" : path;
const r = {};
const reach = async (f) => { try { return "read:" + String(await f()); } catch (e) { return "denied"; } };
const frame = (src) => new Promise((done) => { const f = document.createElement("iframe"); f.src = src; document.body.appendChild(f); setTimeout(() => done(f), 1500); });
(async () => {
  r.own_storage = String(localStorage.getItem("probe.alpha"));
  r.fetch_live = await reach(async () => (await fetch(at("alpha", "/apps/alpha/"))).status);
  const live = await frame(at("alpha", "/apps/alpha/"));
  r.live_storage = await reach(() => live.contentWindow.localStorage.getItem("probe.alpha"));
  const v1 = await frame(at("alpha--v1", "/apps/alpha/@v1/"));
  r.v1_storage = await reach(() => v1.contentWindow.localStorage.getItem("probe.alpha"));
  document.getElementById("out").textContent = JSON.stringify(r);
})().catch((e) => { document.getElementById("out").textContent = "probe failed: " + e; });
</script></body></html>
HTML
  $B deploy "$W/alpha-v2" --server $S --token $T --json >/dev/null
}
release_probe
dump releases "$FA/" "$W/release-live.html"; dump releases "http://$RA/" "$W/release-v1.html"
dump releases "http://alpha--v2.localhost:$P/" "$W/release-v2.html"
python3 - "$(result "$W/release-v2.html")" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
want = {"own_storage": "null", "fetch_live": "denied", "live_storage": "denied", "v1_storage": "denied"}
assert r == want, f"release v2 reached the live front's or release v1's storage: {r}"
PY

# --- the console renders under its policy: no script refused
dump console "$S/" "$W/console.html" --enable-logging=stderr --v=0
python3 - "$W/console.html" <<'PY'
import re, sys
dom = open(sys.argv[1]).read()
root = re.search(r'<div id="root"[^>]*>(.*)</div>', dom, re.S)
assert root and len(root.group(1)) > 200, "the console did not render: " + dom[:1500]
PY
if grep -i "content security policy\|refused to" "$W/chrome-console.log"; then echo "e2e-fronts: the console broke its own policy" >&2; exit 1; fi

# --- the control: the same fronts under the server's origin, where alpha reads beta's secret
down
up shared
probe shared "$S/apps/beta/" "$S/apps/alpha/" "$W/alpha-shared.html"
python3 - "$(result "$W/alpha-shared.html")" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
assert r["beta_storage"] == "read:beta-secret" and r["beta_dom"] == "read:beta's page", f"the probe should see the leak of a shared origin: {r}"
PY
release_probe
dump shared "$S/apps/alpha/@v2/" "$W/release-shared.html"
python3 - "$(result "$W/release-shared.html")" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
assert r["own_storage"] == "alpha-own" and r["live_storage"] == "read:alpha-own", f"the release probe should see the leak of a shared origin: {r}"
PY
echo "e2e-fronts: each front at the root of its own host, older links moved there, each release on a host of its own with its own runs; alpha reaches neither beta's storage nor the console's, nor the platform through its host, and a release neither the live front's storage nor another release's (on a shared origin they read them); the console runs under its policy — passed"

#!/usr/bin/env bash
# The organization's apps portal (CORI_PORTAL_URL) end to end, with builtin auth and the dev IdP's directory standing in for
# the organization's (the grants each token carries):
# - in Chrome, alice opens the portal: it signs her in through the console (the dev IdP picks her: DEV_IDP_USER) and lists
#   her apps, what waits for her and their icons, under the page's policy with no violation;
# - each person's listing is the apps where they hold a role and those they build, an app with nothing live its builders'
#   only, and what waits for them there of the live runs (not a release's, not a go-live), where the console's lists all;
# - the portal's host serves its page, its sign-in, the person and their apps and the apps' icons, nothing else;
# - its session is heard on the portal's host only, and that host hears its own sessions only;
# - a front's files are for the people who may open it (CORI_FRONTS_PRIVATE): a page load without a pass of the front's
#   host goes through the console, which gives a code to whoever may open the app (or the release), and the host trades it
#   for its pass, a cookie of its own, with the verifier its browser kept; files and fetches without one get a 401, a pass
#   opens on its own host only, none of it is for a shared cache; in Chrome, alice opens desk that way;
# - a sign-in client whose tokens carry none of the apps' roles (Keycloak's "Full scope allowed" off: DEV_IDP_LIMITED_SCOPE):
#   a person's roles in an app are the receiver's answer to `access.list` about them, in the app and in a release's
#   session, which follows the receiver's grants at each refresh;
# - another tool frames a release (CORI_PREVIEW_SIGN_IN_URL): framed, its sign-in and its pass go to the tool's page, which
#   gets the code with its own client's token, for the release's builders and its own callback only; in a tab, and on a
#   live front, the console as before;
# - those who build an app try its releases with roles of their choosing (`preview-roles`), held by their releases'
#   sessions from the next sign-in or refresh and never by the app, until they take their own back or stop building it.
# Needs CORI_E2E_DATABASE_URL (or CORI_STORE_DATABASE_URL), a Postgres admin URL; a Temporal dev server on 127.0.0.1:7233;
# node 22+ (scripts/browse.mjs drives the browser); Chrome or Chromium (CHROME names it). `*.localhost` resolves to this
# machine in Chrome and curl.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q; cargo build -p cori-server --example dev_idp -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)
P=${CORI_E2E_PORTAL_PORT:-8187}; IP=$((P + 1)); S=http://127.0.0.1:$P; ISS=http://127.0.0.1:$IP/realms/acme
PH=apps.localhost:$P; PO=http://$PH   # the portal's host and origin
CHROME=${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser || echo "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")}
[ -x "$CHROME" ] || { echo "e2e-portal: no Chrome (set CHROME)" >&2; exit 1; }
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-portal: set CORI_E2E_DATABASE_URL to a Postgres admin URL" >&2; exit 1; }
for port in $P $IP; do if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-portal: port $port is taken (CORI_E2E_PORTAL_PORT)" >&2; exit 1; fi; done
${PSQL:-psql} "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_portal WITH (FORCE)" -c "CREATE DATABASE cori_e2e_portal"
DBURL="${ADMIN%/*}/cori_e2e_portal"; Q=cori.e2e-portal
DEV_IDP_PORT=$IP DEV_IDP_PEOPLE=manage DEV_IDP_USER=alice DEV_IDP_LIMITED_SCOPE=cori-agents CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret ./target/debug/examples/dev_idp >"$W/idp.log" 2>&1 & IDP=$!
CORI_FRONTS_URL="http://*.localhost:$P" CORI_FRONTS_PRIVATE=true CORI_PORTAL_URL=$PO CORI_PREVIEW_SIGN_IN_URL=http://embed.localhost:$P/cori/sign-in CORI_FRAME_ANCESTORS=http://embed.localhost:$P CORI_AUTH_CLIENT_HOOK=http://127.0.0.1:$IP/cori/clients CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret \
  CORI_AUTH_ISSUER=$ISS CORI_AUTH_ROLES_CLAIM=roles \
  $B serve --listen 127.0.0.1:$P --data-dir "$W/data" --database-url "$DBURL" --auth builtin --dev-mocks --task-queue $Q >"$W/serve.log" 2>&1 & SRV=$!
$B work --queue $Q >"$W/work.log" 2>&1 & WK=$!
trap 'kill $IDP $SRV $WK 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
kill -0 $SRV 2>/dev/null || { echo "e2e-portal: cori serve exited:" >&2; cat "$W/serve.log" >&2; exit 1; }
for i in $(seq 1 40); do curl -sf $ISS/.well-known/openid-configuration >/dev/null && break; sleep 0.25; done
tok() { curl -sf -X POST $ISS/protocol/openid-connect/token -d "grant_type=password&username=$1&password=$1&client_id=${2:-cori-cli}" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])"; }
py() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
hdr() { curl -s -o /dev/null -D - "$@" | tr -d '\r'; }
enc() { python3 -c "import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=''))" "$1"; }
pkce() { python3 -c "import base64,hashlib,secrets; v=base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b'=').decode(); print(v, base64.urlsafe_b64encode(hashlib.sha256(v.encode()).digest()).rstrip(b'=').decode())"; }
TJ=$(tok jean)

# three apps, plain fronts with one workflow that waits for an app_user's decision: desk (live, with a description and an
# icon), ledger (live, then protected: its next release waits for an approver), draft (deployed, never live)
app() {  # <id> [description]
  env -u CORI_SERVER -u CORI_TOKEN HOME="$W" "$B" init "$W/$1" --no-register --no-git --front plain >/dev/null
  [ -z "${2:-}" ] || python3 -c "import sys; p=sys.argv[1]; s=open(p).read(); open(p,'w').write(s.replace('[app]\n', '[app]\ndescription = \"' + sys.argv[2] + '\"\n', 1))" "$W/$1/cori.toml" "$2"
  "$B" workflow new ask --dir "$W/$1" >/dev/null
  sed_i 's/^steps: \[\]/steps: [decide]/; s/^  allow: { roles: \[\] }/  allow: { roles: [app_user, admin] }/; s/^  mode: sync/  mode: async/' "$W/$1/workflows/ask/workflow.yaml"
  cat > "$W/$1/workflows/ask/steps/decide.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.human({
  name: "decide", description: "Decide", kind: "approval", to: { role: "app_user" },
  input: (_ctx: Ctx<"decide">) => ({}), output: z.object({ approved: z.boolean() }),
  timeout: "1d", on_timeout: { default: { approved: false } },
});
TS
  $B apps create "$1" --server $S --token "$TJ" | has "created"
}
deploy() { $B deploy "$W/$1" --server $S --token "${2:-$TJ}" --json | py "d['hash']"; }
app desk "Answer the support queue"; app ledger; app draft
printf '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="3" fill="#0a7"/></svg>' > "$W/desk/dist/favicon.svg"
printf 'document.title = "desk";\n' > "$W/desk/dist/app-3f9c2b1a.js"   # a file a bundler names after its content
$B promote desk "$(deploy desk)" --server $S --token "$TJ" | has "promoted"
$B promote ledger "$(deploy ledger)" --server $S --token "$TJ" | has "promoted"
deploy draft >/dev/null
# who holds what: alice an app_user of desk and of draft, sam of ledger, jean of desk (to try its release); alice builds
# ledger, sam draft. The tokens minted next carry the grants.
grant() { local body; body=$(printf '{"person":"%s","roles":["%s"],"granted":true}' "$2" "$3"); test "$(code -X POST $S/v1/apps/$1/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d "$body")" = 202; }
grant desk alice app_user; grant draft alice app_user; grant ledger sam app_user; grant desk jean app_user
curl -sf -X PUT $S/v1/apps/draft/builders -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"builders":["sam"]}' >/dev/null
curl -sf -X PUT $S/v1/apps/ledger/builders -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"builders":["alice"]}' >/dev/null
TA=$(tok alice); TS=$(tok sam); TJ=$(tok jean)

# what waits: a live run of desk and a run of its release v2 (a preview, on its sandbox) for an app_user's decision; a
# go-live of ledger, which alice asks for, for an approver's (sam)
curl -sf -X POST $S/v1/apps/desk/wf/ask/runs -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{}' | has '"run_id"'
printf '\n# another release\n' >> "$W/desk/cori.toml"; deploy desk >/dev/null
curl -sf -X POST $S/v1/apps/desk/versions/v2/wf/ask/runs -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{}' | has '"run_id"'
for i in $(seq 1 60); do [ "$(curl -sf $S/v1/apps/desk/approvals -H "Authorization: Bearer $TJ" | py "len(d)")" = 2 ] && break; sleep 0.5; done
test "$(curl -sf $S/v1/apps/desk/approvals -H "Authorization: Bearer $TJ" | py "len(d)")" = 2 || { echo "e2e-portal: desk's runs did not reach their decision" >&2; tail -20 "$W/work.log" >&2; exit 1; }
curl -sf -X POST $S/v1/apps/ledger/protect -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"protected":true}' >/dev/null
printf '\n# another release\n' >> "$W/ledger/cori.toml"
curl -sf -X POST "$S/v1/apps/ledger/versions/$(deploy ledger "$TA")/promote" -H "Authorization: Bearer $TA" | has '"status":"pending_approval"'

# --- the portal's sign-in, as its page makes it: its own host is the issuer, the console gives the code
read -r VER CH < <(pkce)
curl -sf -H "Host: $PH" $S/__cori/oidc/.well-known/openid-configuration | has "\"token_endpoint\":\"$PO/__cori/oidc/token\""
curl -s -o /dev/null -w '%{redirect_url}' -H "Host: $PH" "$S/__cori/oidc/authorize?response_type=code&client_id=portal&redirect_uri=$(enc "$PO/")&code_challenge=$CH&code_challenge_method=S256&state=s1" | has "^$S/#/portal?redirect_uri="
test "$(code -H "Host: $PH" "$S/__cori/oidc/authorize?response_type=code&client_id=preview&redirect_uri=$(enc "$PO/")&code_challenge=$CH&code_challenge_method=S256")" = 400
test "$(code -H "Host: $PH" "$S/__cori/oidc/authorize?response_type=code&client_id=portal&redirect_uri=$(enc "http://desk.localhost:$P/")&code_challenge=$CH&code_challenge_method=S256")" = 400
ask() { printf '{"code_challenge":"%s","redirect_uri":"%s"}' "$1" "$2"; }
test "$(code -X POST $S/v1/portal-codes -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d "$(ask "$CH" "http://desk.localhost:$P/")")" = 400
TAD=$(tok alice desk)   # what desk's front holds: no portal code for it
test "$(code -X POST $S/v1/portal-codes -H "Authorization: Bearer $TAD" -H 'content-type: application/json' -d "$(ask "$CH" "$PO/")")" = 403
test "$(code -X POST -H "Host: $PH" $S/v1/portal-codes -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d "$(ask "$CH" "$PO/")")" = 404
trade() { curl -s -X POST -H "Host: $PH" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$1&code_verifier=$2&redirect_uri=$(enc "$PO/")&client_id=portal"; }
portal() {  # <console token>: the portal's session (access, refresh) for whoever it names
  local v c pc; read -r v c < <(pkce)
  pc=$(curl -sf -X POST $S/v1/portal-codes -H "Authorization: Bearer $1" -H 'content-type: application/json' -d "$(ask "$c" "$PO/")" | py "d['code']")
  trade "$pc" "$v" | py "d['access_token'] + ' ' + d['refresh_token']"
}
read -r PA PR < <(portal "$TA")
PC=$(curl -sf -X POST $S/v1/portal-codes -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d "$(ask "$CH" "$PO/")" | py "d['code']")
trade "$PC" "wrong-verifier-wrong-verifier-wrong-verifier-0" | has '"error":"invalid_grant"'
trade "$PC" "$VER" | has '"error":"invalid_grant"'   # a code is tried once, whatever came of it
curl -sf -X POST -H "Host: $PH" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$PR" | has '"access_token"'
curl -s -X POST -H "Host: desk--v1.localhost:$P" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$PR" | has '"error":"invalid_grant"'
curl -s -o /dev/null -w '%{redirect_url}' -H "Host: $PH" "$S/__cori/oidc/logout?post_logout_redirect_uri=$(enc "$PO/")" | has "^$PO/$"

# --- each person's apps: on the portal the live ones where they hold a role and those they build, with what waits for them
# there of the live runs; the console lists every app they hold a role in, and every decision
listing() { curl -sf "$@" | py "sorted((a['id'], a['waiting_for_me'], a['build']) for a in d)"; }
test "$(listing -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $PA")" = "[('desk', 1, False), ('ledger', 0, True)]"
test "$(listing $S/v1/me/apps -H "Authorization: Bearer $TA")" = "[('desk', 2, False), ('draft', 0, False), ('ledger', 0, True)]"
read -r PS _ < <(portal "$TS")
test "$(listing -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $PS")" = "[('draft', 0, True), ('ledger', 0, False)]"
test "$(listing $S/v1/me/apps -H "Authorization: Bearer $TS")" = "[('draft', 0, True), ('ledger', 1, False)]"
read -r PJ _ < <(portal "$TJ")
test "$(listing -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $PJ")" = "[('desk', 1, True), ('draft', 0, True), ('ledger', 0, True)]"
curl -sf -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $PJ" | python3 -c "
import sys, json
a = {x['id']: x for x in json.load(sys.stdin)}
assert (a['desk']['description'], a['desk']['icon'], a['desk']['front_url']) == ('Answer the support queue', '/__cori/icons/desk', 'http://desk.localhost:$P/'), a['desk']
assert a['ledger']['description'] is None and a['ledger']['icon'] == '/__cori/icons/ledger', a['ledger']
assert a['draft']['icon'] is None and a['draft']['front_url'] is None, a['draft']
"
curl -sf -H "Host: $PH" $S/v1/me -H "Authorization: Bearer $PA" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert (d['sub'], d['portal_url'].rstrip('/'), d['console_url']) == ('alice', '$PO', '$S/'), d
"

# --- the portal's host: its page for every read, its sign-in, the person and their apps, the icons; nothing else
hdr -H "Host: $PH" $S/ | has -i "^content-security-policy: default-src 'self'; script-src 'sha256-"
hdr -H "Host: $PH" $S/ | has -i "frame-ancestors 'none'"
hdr -H "Host: $PH" $S/ | has -i "^x-frame-options: DENY"
hdr -H "Host: $PH" $S/ | has -i "^x-robots-tag: noindex"
curl -sf -H "Host: $PH" "$S/any/deep/link?x=1" | has '<div id="root">'
for p in /v1/apps /v1/apps/desk /v1/apps/desk/runs /v1/people /v1/secrets; do test "$(code -H "Host: $PH" $S$p -H "Authorization: Bearer $PA")" = 404 || { echo "e2e-portal: $p served on the portal's host" >&2; exit 1; }; done
curl -sf -H "Host: $PH" $S/apps/desk/__cori/config.json | has '<div id="root">'   # a read the portal's page answers, never a front's file
test "$(code -X POST -H "Host: $PH" $S/v1/apps/desk/wf/ask/runs -H "Authorization: Bearer $PA" -H 'content-type: application/json' -d '{}')" = 404
test "$(code -X POST -H "Host: $PH" $S/mcp)" = 404
test "$(code -X POST -H "Host: $PH" $S/)" = 404
hdr -H "Host: $PH" $S/__cori/icons/desk | has -i "^content-type: image/svg+xml"
hdr -H "Host: $PH" $S/__cori/icons/desk | has -i "^content-security-policy: default-src 'none'; style-src 'unsafe-inline'; sandbox"
test "$(code -H "Host: $PH" $S/__cori/icons/ledger)" = 404   # no icon: the page draws its initial
test "$(code -H "Host: $PH" $S/__cori/icons/draft)" = 404    # nothing live
test "$(code $S/__cori/icons/desk)" = 404                     # the portal's host only

# --- its session reaches the portal's host only; that host hears its own sessions only
test "$(code -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $PA")" = 200
test "$(code $S/v1/me -H "Authorization: Bearer $PA")" = 403                                          # the console's API
test "$(code -H "Host: desk.localhost:$P" $S/v1/me -H "Authorization: Bearer $PA")" = 403              # an app's front
test "$(code -H "Host: desk--v2.localhost:$P" $S/v1/apps/desk/runs -H "Authorization: Bearer $PA")" = 403   # a release's host
test "$(code -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $TA")" = 403    # the console's token
test "$(code -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $TAD")" = 403   # a front's
RH=desk--v2.localhost:$P; RCB="http://$RH/apps/desk/callback"; read -r RV RC < <(pkce)
RCODE=$(curl -sf -X POST $S/v1/apps/desk/versions/v2/preview-codes -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d "$(ask "$RC" "$RCB")" | py "d['code']")
RAT=$(curl -sf -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$RCODE&code_verifier=$RV&redirect_uri=$(enc "$RCB")&client_id=preview" | py "d['access_token']")
test "$(code -H "Host: $PH" $S/v1/me/apps -H "Authorization: Bearer $RAT")" = 403   # a release's session

# --- a front's files, for the people who may open it (CORI_FRONTS_PRIVATE)
DO=http://desk.localhost:$P; LO=http://ledger.localhost:$P; RO=http://desk--v2.localhost:$P
nav() { local jar=$1; shift; curl -s -b "$jar" -c "$jar" -o /dev/null -w '%{http_code} %{redirect_url}\n' -H 'Sec-Fetch-Mode: navigate' "$@"; }
jarval() { awk -v h="$2" -v n="$3" '($1 == h || $1 == "#HttpOnly_" h) && $6 == n { v = $7 } END { print v }' "$1"; }   # <jar> <host> <cookie>
# what the console does with a hand-off address (`#/pass/<app>[/v<n>]?…`) for whoever <token> names: the status of its
# code request, and the host's address to go back to with the code
console_pass() {
  python3 - "$1" "$2" "$S" <<'PY'
import json, sys, urllib.parse as u, urllib.request as r
hand, tok, server = sys.argv[1:4]
path, query = hand.split("#/pass/", 1)[1].split("?", 1)
app, *rel = [u.unquote(p) for p in path.split("/")]
q = dict(u.parse_qsl(query))
body = {"code_challenge": q["code_challenge"], "redirect_uri": q["redirect_uri"], **({"release": rel[0]} if rel else {})}
req = r.Request(f"{server}/v1/apps/{u.quote(app)}/pass-codes", data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
try:
    d = json.load(r.urlopen(req))
    print(200, d["redirect_uri"] + "?" + u.urlencode({"code": d["code"], "state": q.get("state", "")}))
except r.HTTPError as e:
    print(e.code, "-")
PY
}
# no pass: a page load goes to the console's hand-off for that host (with a cookie of that sign-in's own); a file, the
# config, a fetch get a 401; the app's own sign-in callback is not held up
J1=$W/jar-1; J2=$W/jar-2; J3=$W/jar-3
read -r c to < <(nav "$J1" "$DO/tickets?x=1")
test "$c" = 302
echo "$to" | has "^$S/#/pass/desk?redirect_uri=$(enc "$DO/__cori/pass")&code_challenge="
test "$(grep -c 'cori_flow_' "$J1")" = 1
test "$(code "$DO/__cori/config.json")" = 401
test "$(code -H 'Sec-Fetch-Mode: no-cors' "$DO/app-3f9c2b1a.js")" = 401
test "$(code -H 'Sec-Fetch-Mode: cors' -H 'Accept: text/html' "$DO/")" = 401
test "$(code "$DO/apps/desk/callback?code=c&state=s")" = 303
read -r c rto < <(nav "$J2" "$RO/")
test "$c" = 302
echo "$rto" | has "^$S/#/pass/desk/v2?redirect_uri=$(enc "$RO/__cori/pass")&"
# alice holds a role in desk: the console gets her a code, the host trades it for its pass and goes back to the page
read -r c back < <(console_pass "$to" "$TA")
test "$c" = 200
read -r c to2 < <(nav "$J1" "$back")
test "$c" = 303
test "$to2" = "$DO/tickets?x=1"
test -n "$(jarval "$J1" desk.localhost cori_pass)"
test "$(grep -c 'cori_flow_' "$J1")" = 0
# with it: the page, its files and its config, each kept by this browser alone
read -r c _ < <(nav "$J1" "$DO/tickets?x=1")
test "$c" = 200
hdr -b "$J1" "$DO/" | has -i "^cache-control: private, no-cache"
hdr -b "$J1" "$DO/app-3f9c2b1a.js" | has -i "^cache-control: private, max-age=31536000, immutable"
test "$(code -b "$J1" "$DO/__cori/config.json")" = 200
# a pass opens on its own host only: not on another app's, nor on a release's of the same app
PV=$(jarval "$J1" desk.localhost cori_pass)
test "$(code -H 'Sec-Fetch-Mode: no-cors' -H "Cookie: cori_pass=$PV" "$DO/app-3f9c2b1a.js")" = 200
test "$(code -H 'Sec-Fetch-Mode: no-cors' -H "Cookie: cori_pass=$PV" "$LO/")" = 401
test "$(code -H 'Sec-Fetch-Mode: no-cors' -H "Cookie: cori_pass=$PV" "$RO/")" = 401
test "$(code -H 'Sec-Fetch-Mode: no-cors' -H "Cookie: cori_pass=x$PV" "$DO/")" = 401
# a code is for the browser that started the sign-in: one without its cookie, or with another verifier, is refused
read -r c to < <(nav "$J3" "$DO/")
read -r c back < <(console_pass "$to" "$TA")
test "$c" = 200
test "$(code "$back")" = 400
read -r _ other < <(pkce)
read -r c forged < <(console_pass "$(printf '%s' "$to" | sed "s/code_challenge=[^&]*/code_challenge=$other/")" "$TA")
test "$c" = 200
test "$(nav "$J3" "$forged" | cut -d' ' -f1)" = 400
# who may open it: sam holds no role in desk and gina is a guest it is not shared with; a release is for those who may
# open one (alice may not, jean builds desk)
test "$(console_pass "$to" "$TS" | cut -d' ' -f1)" = 403
test "$(console_pass "$to" "$(tok gina)" | cut -d' ' -f1)" = 403
test "$(console_pass "$rto" "$TA" | cut -d' ' -f1)" = 403
read -r c back < <(console_pass "$rto" "$TJ")
test "$c" = 200
read -r c to2 < <(nav "$J2" "$back")
test "$c" = 303
test "$to2" = "$RO/"
test "$(nav "$J2" "$RO/" | cut -d' ' -f1)" = 200
# the pass codes are the console's: not on a front's host, nor for a front's own token
test "$(code -X POST -H "Host: desk.localhost:$P" $S/v1/apps/desk/pass-codes -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{}')" = 404
PASK=$(printf '{"code_challenge":"%s","redirect_uri":"%s"}' "$CH" "$DO/__cori/pass")
test "$(code -X POST $S/v1/apps/desk/pass-codes -H "Authorization: Bearer $TAD" -H 'content-type: application/json' -d "$PASK")" = 403
test "$(code -X POST $S/v1/apps/desk/pass-codes -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d "$(printf '{"code_challenge":"%s","redirect_uri":"%s"}' "$CH" "$LO/__cori/pass")")" = 400

# --- a sign-in client not given the apps' roles (cori-agents here): jean's roles in desk are the receiver's (`access.list`)
claims() { python3 -c "import sys,json,base64; p=sys.argv[1].split('.')[1]; d=json.loads(base64.urlsafe_b64decode(p + '=' * (-len(p) % 4))); print($2)" "$1"; }
TJL=$(tok jean cori-agents)
test "$(claims "$TJL" "sorted(d['resource_access'])")" = "['cori-agents']"
test "$(curl -sf $S/v1/apps/desk/me -H "Authorization: Bearer $TJL" | py "d['roles']")" = "['app_user']"
# a release's session holds them, and follows the receiver's grants at each refresh: taken back, then given again
read -r LV LC < <(pkce)
LCODE=$(curl -sf -X POST $S/v1/apps/desk/versions/v2/preview-codes -H "Authorization: Bearer $TJL" -H 'content-type: application/json' -d "$(ask "$LC" "$RCB")" | py "d['code']")
curl -sf -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$LCODE&code_verifier=$LV&redirect_uri=$(enc "$RCB")&client_id=preview" > "$W/lses.json"
test "$(claims "$(py "d['access_token']" < "$W/lses.json")" "d['app_roles']")" = "['app_user']"
renew() { curl -sf -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$(py "d['refresh_token']" < "$W/lses.json")" > "$W/lses.next" && mv "$W/lses.next" "$W/lses.json"; claims "$(py "d['access_token']" < "$W/lses.json")" "d['app_roles']"; }
test "$(code -X POST $S/v1/apps/desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"jean","roles":["app_user"],"granted":false}')" = 202
test "$(renew)" = "[]"
test "$(curl -sf $S/v1/apps/desk/me -H "Authorization: Bearer $TJL" | py "d['roles']")" = "[]"
grant desk jean app_user
test "$(renew)" = "['app_user']"
# a person's token that carries the app's roles is read as it is: no question asked (sam's console token)
test "$(curl -sf $S/v1/apps/ledger/me -H "Authorization: Bearer $TS" | py "d['roles']")" = "['app_user']"

# --- another tool frames a release (CORI_PREVIEW_SIGN_IN_URL): framed, the release's sign-in and its pass go to the tool's
# page, which asks for the code as the person, with a token of its own client (cori-agents here: neither the platform's
# nor an app's); in a tab of its own, and on the live front, the console as before
SO=http://embed.localhost:$P/cori/sign-in
framed() { local jar=$1; shift; curl -s -b "$jar" -c "$jar" -o /dev/null -w '%{http_code} %{redirect_url}\n' -H 'Sec-Fetch-Mode: navigate' -H 'Sec-Fetch-Dest: iframe' "$@"; }
test "$(curl -sf $S/v1/auth/config | py "d['fronts_url'], d['portal_url']")" = "http://*.localhost:$P $PO/"
read -r SV SC < <(pkce)
AUTHZ="$S/__cori/oidc/authorize?response_type=code&client_id=preview&redirect_uri=$(enc "$RCB")&code_challenge=$SC&code_challenge_method=S256&state=s9"
J5=$W/jar-5
read -r c to < <(framed "$J5" -H "Host: $RH" "$AUTHZ")
test "$c" = 302
test "$to" = "$SO?kind=preview&app=desk&release=v2&redirect_uri=$(enc "$RCB")&code_challenge=$SC&state=s9"
curl -s -o /dev/null -w '%{redirect_url}' -H "Host: $RH" -H 'Sec-Fetch-Dest: document' "$AUTHZ" | has "^$S/#/preview/desk/v2?redirect_uri="
SCODE=$(curl -sf -X POST $S/v1/apps/desk/versions/v2/preview-codes -H "Authorization: Bearer $TJL" -H 'content-type: application/json' -d "$(ask "$SC" "$RCB")" | py "d['code']")
test "$(curl -sf -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$SCODE&code_verifier=$SV&redirect_uri=$(enc "$RCB")&client_id=preview" | py "d['token_type']")" = Bearer
# the release's builders only (alice may not open one), and its own callback only
test "$(code -X POST $S/v1/apps/desk/versions/v2/preview-codes -H "Authorization: Bearer $(tok alice cori-agents)" -H 'content-type: application/json' -d "$(ask "$SC" "$RCB")")" = 403
for cb in "$DO/apps/desk/callback" "http://desk--v1.localhost:$P/apps/desk/callback" "$SO" "$RCB?x=1"; do
  test "$(code -X POST $S/v1/apps/desk/versions/v2/preview-codes -H "Authorization: Bearer $TJL" -H 'content-type: application/json' -d "$(ask "$SC" "$cb")")" = 400
done
# a release's pass: the tool's page gets jean a code, the frame goes back to the host with it, and the host lets it in
read -r c sto < <(framed "$J5" "$RO/")
test "$c" = 302
echo "$sto" | has "^$SO?kind=pass&app=desk&release=v2&redirect_uri=$(enc "$RO/__cori/pass")&code_challenge="
read -r c lto < <(framed "$W/jar-6" "$DO/")
echo "$lto" | has "^$S/#/pass/desk?redirect_uri="
framed_pass() {  # <its sign-in page> <token>: what the tool's page does, as console_pass does for the console's
  python3 - "$1" "$2" "$S" <<'PY'
import json, sys, urllib.parse as u, urllib.request as r
at, tok, server = sys.argv[1:4]
q = dict(u.parse_qsl(at.split("?", 1)[1]))
assert q["kind"] == "pass", q
body = {"code_challenge": q["code_challenge"], "redirect_uri": q["redirect_uri"], "release": q["release"]}
req = r.Request(f"{server}/v1/apps/{u.quote(q['app'])}/pass-codes", data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
try:
    d = json.load(r.urlopen(req))
    print(200, d["redirect_uri"] + "?" + u.urlencode({"code": d["code"], "state": q["state"]}))
except r.HTTPError as e:
    print(e.code, "-")
PY
}
test "$(framed_pass "$sto" "$(tok alice cori-agents)" | cut -d' ' -f1)" = 403
read -r c back < <(framed_pass "$sto" "$TJL")
test "$c" = 200
read -r c to2 < <(framed "$J5" "$back")
test "$c" = 303
test "$to2" = "$RO/"
test "$(framed "$J5" "$RO/" | cut -d' ' -f1)" = 200
# a refused pass is shown in the tool's frame too
hdr "$RO/__cori/pass?state=nope" | has -i "^content-security-policy: .*frame-ancestors 'self' http://embed.localhost:$P"

# --- releases tried with roles of one's choosing (`preview-roles`): those who build the app choose, from the tool with its
# own client's token; their releases' sessions hold the roles chosen from their next sign-in or refresh, never the app
proles() {  # <method> <app> <token> [body]: what the preview roles' routes answer, as (roles, choices, own)
  local m=$1 a=$2 t=$3; shift 3
  curl -sf -X "$m" "$S/v1/apps/$a/preview-roles" -H "Authorization: Bearer $t" -H 'content-type: application/json' ${1+-d "$1"} | py "(d['roles'], d['choices'], d['own'])"
}
release_session() {  # <app> <release> <token> <file>: a session of the release's host for whoever <token> names, kept in <file>; its roles
  local v c host=$1--$2.localhost:$P cb rc
  cb="http://$host/apps/$1/callback"; read -r v c < <(pkce)
  rc=$(curl -sf -X POST $S/v1/apps/$1/versions/$2/preview-codes -H "Authorization: Bearer $3" -H 'content-type: application/json' -d "$(ask "$c" "$cb")" | py "d['code']")
  curl -sf -X POST -H "Host: $host" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$rc&code_verifier=$v&redirect_uri=$(enc "$cb")&client_id=preview" > "$4"
  claims "$(py "d['access_token']" < "$4")" "d['app_roles']"
}
refresh() {  # <app> <release> <file>: the session refreshed, as a reload of the release's page does; its roles
  curl -sf -X POST -H "Host: $1--$2.localhost:$P" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$(py "d['refresh_token']" < "$3")" > "$3.next" && mv "$3.next" "$3"
  claims "$(py "d['access_token']" < "$3")" "d['app_roles']"
}
# jean owns desk and holds app_user there; its releases declare app_user and admin
test "$(proles GET desk "$TJL")" = "(None, ['admin', 'app_user'], ['app_user'])"
test "$(curl -sf "$S/v1/apps/desk/preview-roles?release=v2" -H "Authorization: Bearer $TJL" | py "d['choices']")" = "['admin', 'app_user']"
test "$(code "$S/v1/apps/desk/preview-roles?release=v9" -H "Authorization: Bearer $TJL")" = 404
# he tries desk's releases as an admin: a sign-in from the tool's page or the console's holds it, the app does not
test "$(proles PUT desk "$TJL" '{"roles":["admin"]}')" = "(['admin'], ['admin', 'app_user'], ['app_user'])"
test "$(release_session desk v2 "$TJL" "$W/pr.json")" = "['admin']"
test "$(release_session desk v2 "$TJ" "$W/pr-console.json")" = "['admin']"
test "$(curl -sf $S/v1/apps/desk/me -H "Authorization: Bearer $TJL" | py "d['roles']")" = "['app_user']"
PRT=$(py "d['access_token']" < "$W/pr.json")
test "$(curl -sf -H "Host: $RH" $S/v1/apps/desk/me -H "Authorization: Bearer $PRT" | py "d['roles']")" = "['admin']"
curl -sf -X POST -H "Host: $RH" $S/v1/apps/desk/wf/ask/runs -H "Authorization: Bearer $PRT" -H 'content-type: application/json' -d '{}' | has '"run_id"'
# then with no role at all: the next refresh holds it, and the release's workflow refuses him
test "$(proles PUT desk "$TJL" '{"roles":[]}')" = "([], ['admin', 'app_user'], ['app_user'])"
test "$(refresh desk v2 "$W/pr.json")" = "[]"
PRT=$(py "d['access_token']" < "$W/pr.json")
test "$(code -X POST -H "Host: $RH" $S/v1/apps/desk/wf/ask/runs -H "Authorization: Bearer $PRT" -H 'content-type: application/json' -d '{}')" = 403
# then his own roles again
test "$(proles DELETE desk "$TJL")" = "(None, ['admin', 'app_user'], ['app_user'])"
test "$(refresh desk v2 "$W/pr.json")" = "['app_user']"
curl -sf $S/v1/audit -H "Authorization: Bearer $TJ" | has '"release.preview_roles"'
# the app's roles only, as a list
for body in '{"roles":["nope"]}' '{"roles":"admin"}' '{"roles":[1]}' '{}'; do
  test "$(code -X PUT $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $TJL" -H 'content-type: application/json' -d "$body")" = 400 || { echo "e2e-portal: preview roles $body accepted" >&2; exit 1; }
done
# for those who build the app only: not sam (an approver, who may open its releases), nor alice; nor a release's session,
# a front's token, or on a front's host
test "$(code $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $TS")" = 403
test "$(code -X PUT $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"roles":[]}')" = 403
test "$(release_session desk v2 "$TS" "$W/ps.json")" = "[]"
test "$(code -X PUT $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $PRT" -H 'content-type: application/json' -d '{"roles":[]}')" = 403
test "$(code -X PUT $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $TAD" -H 'content-type: application/json' -d '{"roles":[]}')" = 403
test "$(code -X PUT -H "Host: $RH" $S/v1/apps/desk/preview-roles -H "Authorization: Bearer $TJL" -H 'content-type: application/json' -d '{"roles":[]}')" = 404
# alice builds ledger and holds no role there: she tries its release as an app_user; taken off its builders, her session
# holds her own roles again at its next refresh, and she may no longer choose
TAL=$(tok alice cori-agents)
test "$(proles PUT ledger "$TAL" '{"roles":["app_user"]}')" = "(['app_user'], ['admin', 'app_user'], [])"
test "$(release_session ledger v2 "$TAL" "$W/pa.json")" = "['app_user']"
curl -sf -X PUT $S/v1/apps/ledger/builders -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"builders":[]}' >/dev/null
test "$(refresh ledger v2 "$W/pa.json")" = "[]"
test "$(code $S/v1/apps/ledger/preview-roles -H "Authorization: Bearer $TAL")" = 403
curl -sf -X PUT $S/v1/apps/ledger/builders -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"builders":["alice"]}' >/dev/null
test "$(refresh ledger v2 "$W/pa.json")" = "['app_user']"

# --- in Chrome: alice opens the portal, signs in through the console, and sees her apps under the page's policy
"$CHROME" --headless=new --disable-gpu --no-first-run --no-default-browser-check --disable-background-networking --disable-extensions \
  --disable-component-update --disable-sync --user-data-dir="$W/chrome" --remote-debugging-port=0 about:blank >"$W/chrome.log" 2>&1 & CHR=$!
trap 'kill $IDP $SRV $WK $CHR 2>/dev/null || true' EXIT
for i in $(seq 1 40); do [ -s "$W/chrome/DevToolsActivePort" ] && break; sleep 0.25; done
node scripts/browse.mjs "$(head -1 "$W/chrome/DevToolsActivePort")" "$PO/" "Your apps" > "$W/portal.json"
python3 - "$W/portal.json" "$PO/" <<'PY'
import json, sys
p, home = json.load(open(sys.argv[1])), sys.argv[2]
text, html = p["text"], p["html"]
assert p["url"] == home and "Your apps" in text, f"the portal did not open signed in: {p['url']} {text[:500]!r} {p['console']}"
assert "Answer the support queue" in text and 'src="/__cori/icons/desk"' in html, "desk's card"
assert "waiting for you\n" in text.lower() and "1 waiting for you" in text.lower(), f"what waits for alice: {text!r}"
assert "ledger" in text, "the app alice builds"
assert "draft" not in text, "an app with nothing live, which alice does not build"
broke = [l for l in p["console"] if "Content Security Policy" in l or "Refused to" in l or "ContentSecurityPolicyIssue" in l]
assert not broke, f"the portal or the console broke its policy: {broke}"
PY
# alice opens desk's front: its host sends her through the console for a pass, then shows the page
node scripts/browse.mjs "$(head -1 "$W/chrome/DevToolsActivePort")" "$DO/tickets" "Front hosted by Cori" > "$W/desk.json"
python3 - "$W/desk.json" "$DO/tickets" <<'PY'
import json, sys
p, page = json.load(open(sys.argv[1])), sys.argv[2]
assert p["url"] == page and "Front hosted by Cori" in p["text"], f"desk did not open: {p['url']} {p['text'][:300]!r} {p['console']}"
PY
echo "e2e-portal: the apps portal signs its person in through the console (in Chrome too), lists the live apps where they hold a role and those they build with what waits for them there and their icons, serves nothing else on its host, and its session reaches nothing but it; a front's files reach the people who may open it only, through a pass of its host (in Chrome too); a sign-in client given none of the apps' roles gets them from the receiver; those who build an app try its releases with roles of their choosing — passed"

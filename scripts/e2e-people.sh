#!/usr/bin/env bash
# People and access through the identity hook, the dev IdP standing in for the organization's
# receiver (DEV_IDP_PEOPLE):
# - a receiver that handles them (`manage`): the people its directory lists, an app's access, a grant the next token
#   carries, a role the app does not declare refused, invitations (with an app's roles) and their cancelling, a member's
#   roles, a removal, the receiver's refusals said as they were (an address already a member);
# - one with its own approvals (`requests`): a grant answered 202, then settled by the receiver's notification;
# - one that handles nothing about people: the changes are made in the identity provider, and the people are those seen;
# - notifications: unsigned, stale and repeated ones;
# - who may: an admin for people, an app's owner or an admin for its access, never an agent session; a member who holds
#   no power and builds no app reaches their apps and organizations only, the people again once they build an app;
# - guests: an app shared outside the organization by its owner, approved by an admin (or refused, or withdrawn); the
#   guest reaches that app and nothing else; an end date the leader keeps; a guest with no app left is removed;
# - leaving: what a removal leaves behind (apps handed to another owner, builds, agent sessions), the person's tokens
#   refused at once; a notification of someone removed elsewhere; someone the directory no longer lists, compared;
# - a directory of the organization's own (CORI_IDENTITY_PEOPLE_URL) listing the people in place of the receiver: every
#   page read, what it leaves out filled from tokens, the preflight and its conformance kit, someone it no longer lists
#   compared, and one that refuses Cori: the people seen, said why, nobody leaving;
# - an access webhook of the organization's own (CORI_IDENTITY_ACCESS_URL) beside that directory: an app's owner gives its
#   roles to the people it lists, the changes go to the webhook (signed with its own secret), applied at once or at its
#   IAM's next sync (202, then settled with that secret, which settles nothing else); the console's message; its kit.
# Needs CORI_E2E_DATABASE_URL (or CORI_STORE_DATABASE_URL), a Postgres admin URL.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q; cargo build -p cori-server --example dev_idp -q
B=./target/debug/cori; source scripts/lib.sh; DATA=$(mktemp -d)
P=${CORI_E2E_PEOPLE_PORT:-8178}; IP=$((P + 1)); S=http://127.0.0.1:$P; ISS=http://127.0.0.1:$IP/realms/acme
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-people: set CORI_E2E_DATABASE_URL to a Postgres admin URL" >&2; exit 1; }
for port in $P $IP; do if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-people: port $port is taken (CORI_E2E_PEOPLE_PORT)" >&2; exit 1; fi; done
${PSQL:-psql} "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_people WITH (FORCE)" -c "CREATE DATABASE cori_e2e_people"
DBURL="${ADMIN%/*}/cori_e2e_people"
LOG=$DATA/hook.jsonl
# the receiver, in one of its modes: `idp <mode>` (re)starts it
IDP=""
idp() {
  [ -n "$IDP" ] && { kill "$IDP" 2>/dev/null || true; wait "$IDP" 2>/dev/null || true; }
  DEV_IDP_PORT=$IP DEV_IDP_KEY_FILE=$DATA/idp-key.pem DEV_IDP_HOOK_LOG=$LOG DEV_IDP_PEOPLE="$1" DEV_IDP_NOTIFY_URL=$S/v1/identity/notify DEV_IDP_SETTLE_SECS=2 CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret \
    DEV_IDP_DIRECTORY_SECRET=peoplesecret DEV_IDP_DIRECTORY_LEFT=$DATA/left.txt DEV_IDP_ACCESS_SECRET=accesssecret DEV_IDP_ACCESS_SYNC_SECS=${ACCESS_SYNC:-0} \
    ./target/debug/examples/dev_idp >>"$DATA/idp.log" 2>&1 & IDP=$!
  # its first start makes its RSA key: seconds in a debug build, and more than ten on a CI runner
  for i in $(seq 1 240); do curl -sf $ISS/.well-known/openid-configuration >/dev/null && return 0; sleep 0.25; done
  echo "e2e-people: the dev IdP did not start" >&2; cat "$DATA/idp.log" >&2; return 1
}
idp manage
# the server's settings; `serve` (re)starts it with them and any more given (`NAME=value`)
SETTINGS=(CORI_AUTH_CLIENT_HOOK=http://127.0.0.1:$IP/cori/clients CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret CORI_IDENTITY_CAPS_SECS=1 CORI_AUTH_ACCOUNT_URL=$ISS/account CORI_PEOPLE_COMPARE_LOOPS=1
  CORI_AUTH_ISSUER=$ISS CORI_AUTH_ROLES_CLAIM=roles)
SRV=""
serve() {
  [ -n "$SRV" ] && { kill "$SRV" 2>/dev/null || true; wait "$SRV" 2>/dev/null || true; }
  env "${SETTINGS[@]}" ${1+"$@"} $B serve --listen 127.0.0.1:$P --data-dir "$DATA" --database-url "$DBURL" --auth builtin --dev-mocks >>"$DATA/serve.log" 2>&1 & SRV=$!
  for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
  kill -0 $SRV 2>/dev/null || { echo "e2e-people: cori serve exited:" >&2; cat "$DATA/serve.log" >&2; exit 1; }
}
trap 'kill $IDP $SRV 2>/dev/null || true' EXIT
serve
tok() { curl -sf -X POST $ISS/protocol/openid-connect/token -d "grant_type=password&username=$1&password=$1&client_id=${2:-cori-cli}" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])"; }
claim() { python3 -c "import sys,json,base64; p=sys.argv[1].split('.')[1]; d=json.loads(base64.urlsafe_b64decode(p + '=' * (-len(p) % 4))); print(json.dumps(eval('d' + sys.argv[2])))" "$1" "$2"; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval('d' + sys.argv[1]))" "$1"; }
py() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
code() { curl -s -o "$DATA/body.json" -w '%{http_code}' "$@"; }
as() { local t=$1; shift; curl -s -H "Authorization: Bearer $t" -H 'content-type: application/json' "$@"; }
TJ=$(tok jean); TA=$(tok alice); TS=$(tok sam)

# --- the organization and its people, as the identity provider says them
as "$TJ" $S/v1/organization | tee "$DATA/org.json" | has '"name":"Acme"'
has '"member.invited"' "$DATA/org.json"; json "['can']['invite']" < "$DATA/org.json" | has -x True
as "$TS" $S/v1/people | tee "$DATA/people.json" | has '"source":"directory"'
test "$(py "[p['id'] for p in d['people']]" < "$DATA/people.json")" = "['alice', 'gina', 'jean', 'sam']"
# each member role, what it gives in Cori (from the server's role lists)
test "$(py "[(r['name'], r['powers']) for r in d['member_roles'] if r['name'] in ('it_admin', 'builder', 'member')]" < "$DATA/people.json")" = "[('it_admin', ['admin', 'approver']), ('builder', ['builder']), ('member', [])]"
json "['can']['invite']" < "$DATA/people.json" | has -x False   # sam approves, and is no admin
# alice holds no power and builds no app: she reaches her apps and her organizations, nothing else of the organization
test "$(as "$TA" $S/v1/me | py "(d['apps_only'], d['can']['people'])")" = "(True, False)"
test "$(as "$TS" $S/v1/me | py "(d['apps_only'], d['can']['people'])")" = "(False, True)"
for path in /v1/people /v1/organization /v1/connectors /v1/start /v1/access-requests /v1/apps; do test "$(code $S$path -H "Authorization: Bearer $TA")" = 403; done
has "your roles open the organization's apps" "$DATA/body.json"
test "$(code -X POST $S/mcp -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{}')" = 403
test "$(code $S/v1/me/organizations -H "Authorization: Bearer $TA")" = 200

# --- an app's access: a grant the next token carries, a role the app does not declare refused, who may
$B apps create support-desk --roles support_agent --server $S --token "$TJ" >/dev/null
# alice may not create an app, and learns nothing of the ids taken (support-desk exists): 403, not 409
test "$(code -X POST $S/v1/apps -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"id":"support-desk"}')" = 403
has "needs the builder role" "$DATA/body.json"
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"alice","roles":["support_agent"],"granted":true}')" = 202
json "['status']" < "$DATA/body.json" | has -x done
test "$(claim "$(tok alice)" "['resource_access']['support-desk']['roles']")" = '["support_agent"]'
as "$TJ" $S/v1/apps/support-desk/access | tee "$DATA/access.json" | has '"source":"directory"'
test "$(py "[r['holders'][0]['id'] for r in d['roles'] if r['role'] == 'support_agent']" < "$DATA/access.json")" = "['alice']"
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"alice","roles":["boss"],"granted":true}')" = 400
has "not one of" "$DATA/body.json"
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"person":"sam","roles":["support_agent"],"granted":true}')" = 403
# the receiver's own refusal, said as it was: nobody by that id
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"nobody","roles":["support_agent"],"granted":true}')" = 202
json "['status'], d['answer']" < "$DATA/body.json" | has "refused.*no such person"
# the app handed to another owner: the receiver hears of it (app.updated), as it checks who may grant the app's roles
as "$TJ" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":[],"owner":"sam"}' | has '"owner":"sam"'
for i in $(seq 1 20); do has '"event":"app.updated".*"owner":"sam"' "$LOG" && break; sleep 0.5; done
has '"app":"support-desk".*"event":"app.updated".*"owner":"sam"' "$LOG"
# its owner, no admin, hands it on as well; anyone else may not
test "$(code -X PUT $S/v1/apps/support-desk/builders -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"builders":[],"owner":"alice"}')" = 403
as "$TS" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":[],"owner":"jean"}' | has '"owner":"jean"'
# taking a role back
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"alice","roles":["support_agent"],"granted":false}')" = 202
test "$(claim "$(tok alice)" "['resource_access'].get('support-desk', {}).get('roles')")" != '["support_agent"]'

# --- invitations: with an app's roles, listed, cancelled; an address already a member refused
test "$(code -X POST $S/v1/invitations -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"email":"Zoe@Example.test","grants":[{"app":"support-desk","roles":["support_agent"]}]}')" = 202
json "['status']" < "$DATA/body.json" | has -x done
INV=$(json "['result']['invitation']" < "$DATA/body.json")
as "$TJ" $S/v1/invitations | tee "$DATA/inv.json" | has "\"id\":\"$INV\""
has '"email":"zoe@example.test"' "$DATA/inv.json"; has '"client_id":"support-desk"' "$DATA/inv.json"
test "$(code -X DELETE $S/v1/invitations/$INV -H "Authorization: Bearer $TJ")" = 202
as "$TJ" $S/v1/invitations | lacks "\"id\":\"$INV\""
test "$(code -X POST $S/v1/invitations -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"email":"sam@acme.example"}')" = 202
json "['status'], d['answer']" < "$DATA/body.json" | has "refused.*already a member"
test "$(code -X POST $S/v1/invitations -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"email":"x@example.test"}')" = 403   # not an admin
test "$(code -X POST $S/v1/invitations -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"email":"not an address"}')" = 400

# --- a member's roles, then a removal
test "$(code -X PUT $S/v1/people/sam/roles -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"roles":["support_lead","approver","builder"]}')" = 202
test "$(claim "$(tok sam)" "['roles']")" = '["support_lead", "approver", "builder"]'
test "$(code -X DELETE $S/v1/people/jean -H "Authorization: Bearer $TJ")" = 400   # leaving is another route
# what alice leaves: an app she owns (handed to sam), one she builds, an agent session
$B apps create desk2 --roles agent --server $S --token "$TJ" >/dev/null
as "$TJ" -X PUT $S/v1/apps/desk2/builders -d '{"builders":[],"owner":"alice"}' >/dev/null
as "$TJ" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":["alice"]}' >/dev/null
test "$(code $S/v1/people -H "Authorization: Bearer $TA")" = 200   # she builds apps now: the people are hers to read
AGA=$(as "$TA" -X POST $S/mcp -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"login","arguments":{"app":"desk2"}}}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['structuredContent']['token'])")
test "$(code $S/v1/apps/desk2 -H "Authorization: Bearer $AGA")" = 200
test "$(as "$TJ" $S/v1/people/alice/leaving | py "(d['owns'], d['builds'], d['agent_sessions'])")" = "(['desk2'], ['support-desk'], 1)"
test "$(code -X DELETE $S/v1/people/alice -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"owners":{"desk2":"sam"}}')" = 202
sleep 1; as "$TJ" $S/v1/people | lacks '"id":"alice"'
# at once: her tokens from before refused, her agent session too; desk2 is sam's, and she builds nothing any more
test "$(code $S/v1/me -H "Authorization: Bearer $TA")" = 401; has "you left this organization" "$DATA/body.json"
test "$(code $S/v1/apps/desk2 -H "Authorization: Bearer $AGA")" = 401
test "$(as "$TJ" $S/v1/apps/desk2/access | py "d['owner'], d['owner_left']")" = "('sam', False)"
test "$(as "$TJ" $S/v1/apps/support-desk/access | py "d['builders']")" = "[]"
for i in $(seq 1 30); do grep '"app":"desk2"' "$LOG" | grep -q '"event":"app.updated".*"owner":"sam"' && break; sleep 1; done   # the leader delivers it
grep '"app":"desk2"' "$LOG" | has '"event":"app.updated".*"owner":"sam"'
# every request is kept, with its outcome
as "$TJ" $S/v1/access-requests | tee "$DATA/requests.json" | has '"kind":"remove"'
test "$(py "len([r for r in d if r['status'] == 'refused'])" < "$DATA/requests.json")" = 2
as "$TS" $S/v1/access-requests | lacks '"kind":"remove"'   # not an admin: their own requests only
as "$TS" $S/v1/people | lacks '"kind":"remove"'
has '"event":"member.removed".*"person":"alice".*"status":200' "$LOG"

# --- an app shared outside the organization (gina: a guest of the directory): its owner asks, an admin approves
J='content-type: application/json'
as "$TJ" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":[],"owner":"sam"}' >/dev/null
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TS" -H "$J" -d '{"email":"Gina@acme.example","roles":["support_agent"],"granted":true,"note":"for the audit"}')" = 202
json "['status']" < "$DATA/body.json" | has -x awaiting_approval
SHARE=$(json "['id']" < "$DATA/body.json")
as "$TJ" $S/v1/me | has '"access_waiting":1'
test "$(code -X POST $S/v1/access-requests/$SHARE/approve -H "Authorization: Bearer $TS" -H "$J" -d '{}')" = 403   # not an admin
test "$(code -X POST $S/v1/access-requests/$SHARE/approve -H "Authorization: Bearer $TJ" -H "$J" -d '{"note":"fine"}')" = 202
json "['status'], d['approved_by']" < "$DATA/body.json" | has "done.*jean"
has '"approved_by":"jean"' "$DATA/body.json"
test "$(code -X POST $S/v1/access-requests/$SHARE/approve -H "Authorization: Bearer $TJ" -H "$J" -d '{}')" = 409   # decided already
TG=$(tok gina)
test "$(claim "$TG" "['resource_access']['support-desk']['roles']")" = '["support_agent"]'
# the guest reaches that app and nothing else: not the people, the platform, the connectors, the MCP, another app
test "$(code $S/v1/me -H "Authorization: Bearer $TG")" = 200
json "['guest'], d['can']['people'], d['persona']" < "$DATA/body.json" | has "True.*False.*user"
test "$(code $S/v1/apps/support-desk -H "Authorization: Bearer $TG")" = 200
test "$(code $S/v1/me/apps -H "Authorization: Bearer $TG")" = 200; has '"id":"support-desk"' "$DATA/body.json"
for path in /v1/people /v1/organization /v1/connectors /v1/apps /v1/apps/support-desk/access /v1/access-requests /v1/llm /v1/audit; do test "$(code $S$path -H "Authorization: Bearer $TG")" = 403; done
test "$(code -X POST $S/mcp -H "Authorization: Bearer $TG" -H "$J" -d '{}')" = 403
$B apps create crm --roles agent --server $S --token "$TJ" >/dev/null
test "$(code $S/v1/apps/crm -H "Authorization: Bearer $TG")" = 403
# the people list marks her, with who shared the app and who approved
test "$(as "$TJ" $S/v1/people | py "[(p['guest'], p['shares'][0]['app'], p['shares'][0]['by'], p['shares'][0]['approved_by']) for p in d['people'] if p['id'] == 'gina'][0]")" = "(True, 'support-desk', 'sam', 'jean')"
# refused with a reason; withdrawn by who asked
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TS" -H "$J" -d '{"email":"x@partner.example","roles":["support_agent"],"granted":true}')" = 202
R2=$(json "['id']" < "$DATA/body.json")
test "$(code -X POST $S/v1/access-requests/$R2/refuse -H "Authorization: Bearer $TJ" -H "$J" -d '{"note":"not this one"}')" = 200
json "['status'], d['answer']" < "$DATA/body.json" | has "refused.*not this one"
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TS" -H "$J" -d '{"email":"y@partner.example","roles":["support_agent"],"granted":true}')" = 202
R3=$(json "['id']" < "$DATA/body.json")
test "$(code -X DELETE $S/v1/access-requests/$R3 -H "Authorization: Bearer $TG")" = 403   # neither who asked nor an admin (a guest)
test "$(code -X DELETE $S/v1/access-requests/$R3 -H "Authorization: Bearer $TS")" = 200
json "['status']" < "$DATA/body.json" | has -x cancelled
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TS" -H "$J" -d '{"email":"z@partner.example","roles":["support_agent"],"granted":true,"until":"2020-01-01"}')" = 400   # an end date past
as "$TS" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":[],"owner":"jean"}' >/dev/null
# an admin's own share needs no other approval; at its end date the leader takes it back
UNTIL=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=3)).isoformat())")
test "$(code -X POST $S/v1/apps/crm/access -H "Authorization: Bearer $TJ" -H "$J" -d "$(printf '{"email":"gina@acme.example","roles":["agent"],"granted":true,"until":"%s"}' "$UNTIL")")" = 202
json "['status']" < "$DATA/body.json" | has -x done
test "$(claim "$(tok gina)" "['resource_access']['crm']['roles']")" = '["agent"]'
for i in $(seq 1 45); do [ "$(claim "$(tok gina)" "['resource_access'].get('crm', {}).get('roles')")" = null ] && break; sleep 2; done
test "$(claim "$(tok gina)" "['resource_access'].get('crm', {}).get('roles')")" = null
as "$TJ" "$S/v1/access-requests?app=crm" | py "sorted(r['status'] for r in d)" | has "'done', 'ended'"
# her last app taken back: she is no longer of the organization
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H "$J" -d '{"person":"gina","roles":["support_agent"],"granted":false}')" = 202
as "$TJ" $S/v1/people | lacks '"id":"gina"'
# the receiver told Cori nothing more: the leader, comparing, finds she left, and her token from before is refused
for i in $(seq 1 45); do [ "$(code $S/v1/me -H "Authorization: Bearer $TG")" = 401 ] && break; sleep 1; done
test "$(code $S/v1/me -H "Authorization: Bearer $TG")" = 401
as "$TJ" $S/v1/access-requests >/dev/null

# --- the organization's settings: a server of a single organization is not deleted from its console; the account page
test "$(code $S/v1/organization -H "Authorization: Bearer $TJ")" = 200
json "['can']['delete'], d['can']['export']" < "$DATA/body.json" | has "False.*True"
test "$(code -X DELETE "$S/v1/organization?confirm=acme" -H "Authorization: Bearer $TJ")" = 404
curl -sf $S/v1/auth/config | has '"account_url":"http://127.0.0.1:'"$IP"'/realms/acme/account"'

# --- an agent session reaches none of this
AGENT=$(as "$TJ" -X POST $S/mcp -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"login","arguments":{"app":"support-desk"}}}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['structuredContent']['token'])")
for path in /v1/people /v1/organization /v1/access-requests /v1/apps/support-desk/access; do test "$(code $S$path -H "Authorization: Bearer $AGENT")" = 403; done

# --- a receiver with approvals of its own: 202, then settled by its notification
idp requests; sleep 1.5
TJ=$(tok jean)
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"person":"sam","roles":["support_agent"],"granted":true}')" = 202
json "['status']" < "$DATA/body.json" | has -x pending
REQ=$(json "['id']" < "$DATA/body.json")
for i in $(seq 1 30); do [ "$(as "$TJ" "$S/v1/access-requests?app=support-desk" | py "[r['status'] for r in d if r['id'] == '$REQ'][0]")" = done ] && break; sleep 0.5; done
test "$(as "$TJ" "$S/v1/access-requests?app=support-desk" | py "[r['status'] for r in d if r['id'] == '$REQ'][0]")" = done
test "$(claim "$(tok sam)" "['resource_access']['support-desk']['roles']")" = '["support_agent"]'
TS=$(tok sam); as "$TS" $S/v1/me >/dev/null   # sam calls with the role: the server sees it

# --- notifications: signed, fresh, each once
note() { local b="$1" key="${2:-hooksecret}" sig; sig=$(printf '%s' "$b" | openssl dgst -sha256 -hmac "$key" | awk '{print $NF}'); code -X POST "$S/v1/identity/notify" -H 'content-type: application/json' -H "x-cori-signature: sha256=$sig" -d "$b"; }
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
removed() { printf '{"id":"n1","event":"person.removed","at":"%s","person":"sam"}' "$1"; }
J=$(removed "$NOW"); test "$(note "$J" wrong-secret)" = 401
J=$(removed 2020-01-01T00:00:00Z); test "$(note "$J")" = 400
as "$TJ" -X PUT $S/v1/apps/crm/builders -d '{"builders":[],"owner":"sam"}' >/dev/null
J=$(removed "$NOW"); test "$(note "$J")" = 200
test "$(note "$J")" = 200; has '"duplicate"' "$DATA/body.json"
# Cori's part, except ownership: sam's token from before is refused; crm stays his, shown as an owner who left
test "$(code $S/v1/me -H "Authorization: Bearer $TS")" = 401
test "$(as "$TJ" $S/v1/apps/crm/access | py "d['owner'], d['owner_left']")" = "('sam', True)"
J=$(printf '{"id":"n2","event":"request.settled","at":"%s","request":"%s","outcome":"refused"}' "$NOW" "$REQ"); test "$(note "$J")" = 404   # already settled

# --- a receiver that handles nothing about people: changes are the identity provider's, people are those seen
idp ""; sleep 1.5
TJ=$(tok jean)
test "$(code -X POST $S/v1/invitations -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"email":"y@example.test"}')" = 409
has "is done in your identity provider" "$DATA/body.json"
as "$TJ" $S/v1/people | tee "$DATA/seen.json" | has '"source":"seen"'
has '"id":"jean"' "$DATA/seen.json"
# with the roles their last token carried: member roles, and their apps' roles
test "$(py "[(p['roles'], p['apps']['uses']) for p in d['people'] if p['id'] == 'sam'][0]" < "$DATA/seen.json")" = "(['support_lead', 'approver'], [{'app': 'support-desk', 'roles': ['support_agent']}])"
json "['can']['invite']" < "$DATA/seen.json" | has -x False

# --- the conformance kit against a receiver that handles them
idp manage; sleep 1.5
CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret $B identity test-hook http://127.0.0.1:$IP/cori/clients | tee "$DATA/kit.out" | has "people.list"

# --- a directory of the organization's own (CORI_IDENTITY_PEOPLE_URL): it lists the people, two a page, by id, address and
# name; the receiver handles nothing about people
idp ""
DIR=http://127.0.0.1:$IP/people
serve CORI_IDENTITY_PEOPLE_URL=$DIR CORI_IDENTITY_PEOPLE_SECRET=peoplesecret
TJ=$(tok jean); TA=$(tok alice)
as "$TA" $S/v1/me/organizations >/dev/null   # alice, who left, signs in again: the server sees her
as "$TJ" $S/v1/people | tee "$DATA/dir.json" | has '"source":"directory"'
test "$(py "[p['id'] for p in d['people']]" < "$DATA/dir.json")" = "['alice', 'gina', 'jean', 'sam']"   # every page
# its name for them, and the roles their tokens carried, which it does not say
test "$(py "[(p['name'], p['email'], p['roles']) for p in d['people'] if p['id'] == 'jean'][0]" < "$DATA/dir.json")" = "('Jean (directory)', 'jean@acme.example', ['it_admin'])"
json "['can']['invite']" < "$DATA/dir.json" | has -x False
env "${SETTINGS[@]}" CORI_IDENTITY_PEOPLE_URL=$DIR CORI_IDENTITY_PEOPLE_SECRET=peoplesecret $B serve --preflight --data-dir "$DATA" --database-url "$DBURL" --auth builtin > "$DATA/dir-preflight.txt" 2>&1 || true
has "^ok   people directory .*2 people on its first page, more pages follow" "$DATA/dir-preflight.txt"
CORI_IDENTITY_PEOPLE_SECRET=peoplesecret $B identity test-people $DIR | tee "$DATA/dir-kit.out" | has "pass  every person has an id"
has "pass  a bad signature is refused" "$DATA/dir-kit.out"; has "4 people in 2 page(s)" "$DATA/dir-kit.out"
# someone it no longer lists left: the leader, comparing, finds it, and her token from before is refused
echo alice > "$DATA/left.txt"
for i in $(seq 1 60); do [ "$(code $S/v1/me -H "Authorization: Bearer $TA")" = 401 ] && break; sleep 1; done
test "$(code $S/v1/me -H "Authorization: Bearer $TA")" = 401
as "$TJ" $S/v1/people | lacks '"id":"alice"'
# a directory that refuses Cori (another secret): the console shows the people seen and says why; nobody leaves
serve CORI_IDENTITY_PEOPLE_URL=$DIR CORI_IDENTITY_PEOPLE_SECRET=not-its-secret
as "$TJ" $S/v1/people | tee "$DATA/dir-refused.json" | has '"source":"seen"'
has 'the directory of people.*did not answer `people.list`: http 401' "$DATA/dir-refused.json"
sleep 25   # a comparison or more
test "$(code $S/v1/me -H "Authorization: Bearer $TJ")" = 200

# --- an access webhook of the organization's own (CORI_IDENTITY_ACCESS_URL) beside the directory; the receiver handles
# nothing about people. An app's owner gives its roles to the people the directory lists; the console says the message
ACC=http://127.0.0.1:$IP/access; MSG="Role changes reach people within 10 minutes."; JSON='content-type: application/json'
ADMIN_URL='CORI_IDENTITY_ADMIN_URL=http://idp.example/admin/{client_id}'
# without it, the app's page links to the identity provider's console, where its roles are given
serve CORI_IDENTITY_PEOPLE_URL=$DIR CORI_IDENTITY_PEOPLE_SECRET=peoplesecret "$ADMIN_URL"
TJ=$(tok jean)
test "$(as "$TJ" $S/v1/apps/support-desk/identity | py "d['admin_url']")" = "http://idp.example/admin/support-desk"
test "$(as "$TJ" $S/v1/apps/support-desk/access | py "(d['can']['grant'], d['grants_here'], d['admin_url'])")" = "(False, False, 'http://idp.example/admin/support-desk')"
ACCESS=(CORI_IDENTITY_PEOPLE_URL=$DIR CORI_IDENTITY_PEOPLE_SECRET=peoplesecret CORI_IDENTITY_ACCESS_URL=$ACC CORI_IDENTITY_ACCESS_SECRET=accesssecret "CORI_IDENTITY_ACCESS_MESSAGE=$MSG" "$ADMIN_URL")
serve "${ACCESS[@]}"
TJ=$(tok jean)
as "$TJ" $S/v1/apps/support-desk/access | tee "$DATA/acc.json" | has "\"message\":\"$MSG\""
test "$(py "(d['can']['grant'], d['can']['revoke'], d['grants_here'], d['admin_url'])" < "$DATA/acc.json")" = "(True, True, True, None)"
test "$(as "$TJ" $S/v1/apps/support-desk/identity | py "d['admin_url']")" = None   # no link: its roles are given here
test "$(as "$TJ" $S/v1/people | py "(d['source'], d['can']['grant'], d['can']['invite'])")" = "('directory', True, False)"
# a grant: delivered to the webhook, signed with its own secret (it refuses any other), never to the receiver
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H "$JSON" -d '{"person":"sam","roles":["support_agent"],"granted":true}')" = 202
json "['status']" < "$DATA/body.json" | has -x done
RID=$(json "['id']" < "$DATA/body.json")
grep "\"request\":\"$RID\"" "$LOG" | has '"via":"access"'
test "$(grep -c "\"request\":\"$RID\"" "$LOG")" = "$(grep "\"request\":\"$RID\"" "$LOG" | grep -c '"via":"access"')"
test "$(claim "$(tok sam)" "['resource_access']['support-desk']['roles']")" = '["support_agent"]'
# a builder sees who holds what, and that its owner and admins give it
as "$TJ" -X PUT $S/v1/apps/support-desk/builders -d '{"builders":["sam"]}' >/dev/null
test "$(as "$(tok sam)" $S/v1/apps/support-desk/access | py "(d['can']['grant'], d['grants_here'], d['message'])")" = "(False, True, '$MSG')"
# the webhook's own refusal, said as it was; taking the role back
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H "$JSON" -d '{"person":"nobody","roles":["support_agent"],"granted":true}')" = 202
json "['status'], d['answer']" < "$DATA/body.json" | has "refused.*no such person"
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H "$JSON" -d '{"person":"sam","roles":["support_agent"],"granted":false}')" = 202
json "['status']" < "$DATA/body.json" | has -x done
test "$(claim "$(tok sam)" "['resource_access'].get('support-desk', {}).get('roles')")" = null
# sam calls without it: the directory names him, his token says what he holds, and the console shows it
holds() { as "$TJ" $S/v1/apps/support-desk/access | py "'sam' in [h['id'] for r in d['roles'] if r['role'] == 'support_agent' for h in r['holders']]"; }
as "$(tok sam)" $S/v1/me >/dev/null; test "$(holds)" = False
# the preflight, and the conformance kit
env "${SETTINGS[@]}" "${ACCESS[@]}" $B serve --preflight --data-dir "$DATA" --database-url "$DBURL" --auth builtin > "$DATA/acc-preflight.txt" 2>&1 || true
has "^ok   access webhook .*answered the signed ping.*shows your message" "$DATA/acc-preflight.txt"
CORI_IDENTITY_ACCESS_SECRET=accesssecret $B identity test-access $ACC | tee "$DATA/acc-kit.out" | has "pass  access.granted for nobody is refused, saying why"
has "pass  a bad signature is refused" "$DATA/acc-kit.out"; has "0 failed" "$DATA/acc-kit.out"
# an IAM that syncs every so often: 202, then its own request.settled, signed with the webhook's secret
ACCESS_SYNC=2; idp ""; ACCESS_SYNC=0; sleep 1.5
TJ=$(tok jean)
test "$(code -X POST $S/v1/apps/support-desk/access -H "Authorization: Bearer $TJ" -H "$JSON" -d '{"person":"sam","roles":["support_agent"],"granted":true}')" = 202
json "['status']" < "$DATA/body.json" | has -x pending
REQ=$(json "['id']" < "$DATA/body.json")
for i in $(seq 1 30); do [ "$(as "$TJ" "$S/v1/access-requests?app=support-desk" | py "[r['status'] for r in d if r['id'] == '$REQ'][0]")" = done ] && break; sleep 0.5; done
test "$(as "$TJ" "$S/v1/access-requests?app=support-desk" | py "[r['status'] for r in d if r['id'] == '$REQ'][0]")" = done
test "$(claim "$(tok sam)" "['resource_access']['support-desk']['roles']")" = '["support_agent"]'
as "$(tok sam)" $S/v1/me >/dev/null; test "$(holds)" = True
# that secret settles the access changes it was sent, nothing else
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
J=$(printf '{"id":"n-access-1","event":"person.removed","at":"%s","person":"sam"}' "$NOW"); test "$(note "$J" accesssecret)" = 403
J=$(printf '{"id":"n-access-2","event":"person.removed","at":"%s","person":"sam"}' "$NOW"); test "$(note "$J" not-a-secret)" = 401
test "$(code $S/v1/me -H "Authorization: Bearer $(tok sam)")" = 200
echo "e2e-people: the people and access of an organization through the identity hook — passed"

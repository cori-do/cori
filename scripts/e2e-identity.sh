#!/usr/bin/env bash
# The identity hook end to end, the dev IdP standing in for the organization's receiver:
# - an app's client the IAM creates later (202): the app exists with its client pending, its preview says so, and the
#   client binds when the outbox delivers app.created again;
# - roles one by one as versions declare them (role.created, role.updated), each confirmed; a deploy using a role cori.toml
#   does not declare is refused (CORI_ROLES_STRICT); a release whose roles are not confirmed does not go live
#   (CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES);
# - the receiver down: events wait, and go once it is back;
# - a role no kept version declares any more: role.deleted (no grace here);
# - the app's deletion through the outbox; the catalog and the events; the conformance kit against the dev IdP;
# - a provider other than Keycloak all along: every app's roles in one claim after the member roles (`roles`, as
#   `<client_id>:<role>`, as Entra ID lists the app roles of one registration), the scopes and resource its sign-ins ask
#   for, and the client agents sign in with.
# Needs CORI_E2E_DATABASE_URL (or CORI_STORE_DATABASE_URL), a Postgres admin URL, and Temporal on 127.0.0.1:7233.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q; cargo build -p cori-server --example dev_idp -q
B=./target/debug/cori; source scripts/lib.sh; E=$(e2e_app); DATA=$(mktemp -d)
P=${CORI_E2E_IDENTITY_PORT:-8176}; IP=$((P + 1)); S=http://127.0.0.1:$P; ISS=http://127.0.0.1:$IP/realms/acme
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-identity: set CORI_E2E_DATABASE_URL to a Postgres admin URL" >&2; exit 1; }
for port in $P $IP; do if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-identity: port $port is taken (CORI_E2E_IDENTITY_PORT)" >&2; exit 1; fi; done
${PSQL:-psql} "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_identity WITH (FORCE)" -c "CREATE DATABASE cori_e2e_identity"
DBURL="${ADMIN%/*}/cori_e2e_identity"
LOG=$DATA/hook.jsonl; DOWN=$DATA/hook-down
# the receiver: signed deliveries, every one logged, the first two app.created of an app answered 202
DEV_IDP_PORT=$IP DEV_IDP_HOOK_LOG=$LOG DEV_IDP_HOOK_PENDING=2 DEV_IDP_HOOK_DOWN=$DOWN CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret DEV_IDP_APP_ROLES_PREFIX='{client_id}:' \
  ./target/debug/examples/dev_idp >"$DATA/idp.log" 2>&1 & IDP=$!
CORI_AUTH_CLIENT_HOOK=http://127.0.0.1:$IP/cori/clients CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret CORI_AUTH_CLIENT_HOOK_ROLES=true CORI_ROLES_STRICT=true \
  CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES=true CORI_ROLE_DELETE_GRACE_SECS=0 CORI_AUTH_ISSUER=$ISS CORI_AUTH_ROLES_CLAIM=roles \
  CORI_AUTH_APP_ROLES_CLAIM=roles CORI_AUTH_APP_ROLES_PREFIX='{client_id}:' CORI_AUTH_SCOPES='api://cori-e2e/access openid' CORI_AUTH_RESOURCE=api://cori-e2e \
  CORI_AUTH_MCP_CLIENT_ID=cori-agents CORI_AUTH_MCP_CALLBACK_PORT=33418 \
  $B serve --listen 127.0.0.1:$P --data-dir "$DATA" --database-url "$DBURL" --auth builtin --dev-mocks >"$DATA/serve.log" 2>&1 & SRV=$!
trap 'kill $IDP $SRV 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
kill -0 $SRV 2>/dev/null || { echo "e2e-identity: cori serve exited:" >&2; cat "$DATA/serve.log" >&2; exit 1; }
for i in $(seq 1 40); do curl -sf $ISS/.well-known/openid-configuration >/dev/null && break; sleep 0.5; done
TJ=$(curl -sf -X POST $ISS/protocol/openid-connect/token -d 'grant_type=password&username=jean&password=jean&client_id=cori-cli' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
api() { curl -sf "$S$1" -H "Authorization: Bearer $TJ"; }
catalog_of() { api /v1/identity/catalog | python3 -c "
import sys, json
a = [x for x in json.load(sys.stdin)['apps'] if x['app'] == '$1']
print(('client=' + a[0]['client_status'] + ' ' + ' '.join(sorted(r['role'] + '=' + r['state'] for r in a[0]['roles']))).strip() if a else 'gone')"; }
wait_for() {  # wait_for <seconds> <app> <expected catalog_of output>
  for i in $(seq 1 "$1"); do [ "$(catalog_of "$2")" = "$3" ] && return 0; sleep 1; done
  echo "e2e-identity: after $1 s, $2 is \`$(catalog_of "$2")\`, not \`$3\`" >&2; tail -20 "$LOG" >&2; return 1
}
version() { python3 -c "import sys,json; print(json.load(sys.stdin)['short'])"; }
copy() { local d; d=$(mktemp -d)/support-desk; cp -R "$E" "$d"; echo "$d"; }

# the app is created though its client is not there yet: the IAM answered 202
$B apps create support-desk --server $S --token "$TJ" >/dev/null
test "$(catalog_of support-desk)" = "client=pending"
has '"event":"app.created".*"status":202' "$LOG"

# strict roles: a role used without being declared refuses the deploy
X=$(copy); sed -i.bak 's/"it_admin"\]/"it_admin", "night_shift"]/' "$X/cori.toml"
OUT=$($B deploy "$X" --server $S --token "$TJ" 2>&1 || true)
echo "$OUT" | has night_shift; echo "$OUT" | has CORI_ROLES_STRICT

# deploying works while the client is pending; the preview says the sign-in is being set up
V1=$($B deploy "$E" --server $S --token "$TJ" --json | version)
test "$(curl -s -o "$DATA/pending.html" -w '%{http_code}' "$S/apps/support-desk/@$V1/")" = 200
has "almost ready" "$DATA/pending.html"

# the outbox delivers app.created again until the client comes back; the roles queued behind it follow, confirmed
wait_for 150 support-desk "client=ready support_agent=confirmed support_lead=confirmed"
test "$(grep -c '"event":"app.created"' "$LOG")" -ge 3
api /v1/apps/support-desk | has '"client_id":"support-desk"'
has '"description":"Files and triages tickets.*"event":"role.created"' "$LOG"   # the log's keys are in alphabetical order
$B promote support-desk "$V1" --server $S --token "$TJ" | has promoted

# every app's roles in one claim: alice's token from the app's client lists `support-desk:support_agent` after her member
# roles, and no `resource_access`; she holds support_agent in the app, and in no other app from another client's token
claims() { python3 -c "import sys,json,base64; p=sys.argv[1].split('.')[1]; print(json.dumps(json.loads(base64.urlsafe_b64decode(p+'='*(-len(p)%4)))))" "$1"; }
tok() { curl -sf -X POST $ISS/protocol/openid-connect/token -d "grant_type=password&username=$1&password=$1&client_id=$2" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])"; }
TA=$(tok alice support-desk)
claims "$TA" | has '"roles": \["support_agent", "support-desk:support_agent"\]'
claims "$TA" | lacks resource_access
curl -sf $S/v1/apps/support-desk/me -H "Authorization: Bearer $TA" | has '"roles":\["support_agent"\]'
curl -sf $S/v1/apps/support-desk/me -H "Authorization: Bearer $(tok alice cori-cli)" | has '"roles":\[\]'
# the scopes and resource sign-ins ask for, and the client agents sign in with: in the auth config, the MCP endpoint's
# metadata and challenge, and the front's config
CFG=$(curl -sf $S/v1/auth/config)
for want in '"resource":"api://cori-e2e"' '"scopes":\["api://cori-e2e/access","openid"\]' '"mcp_client_id":"cori-agents"' '"mcp_callback_port":33418'; do echo "$CFG" | has "$want"; done
curl -sf $S/.well-known/oauth-protected-resource | has '"scopes_supported":\["openid","profile","email","offline_access","api://cori-e2e/access"\]'
curl -s -o /dev/null -D - -X POST $S/mcp -H 'content-type: application/json' -d '{}' | has 'scope="openid profile email offline_access api://cori-e2e/access"'
FRONT=$(curl -sf $S/apps/support-desk/__cori/config.json)
echo "$FRONT" | has '"resource":"api://cori-e2e"'; echo "$FRONT" | has '"scopes":\["api://cori-e2e/access","openid"\]'

# a new role, then its description changed: role.created, role.updated
V2D=$(copy); printf '\n[auth.roles.reviewer]\ndescription = "Reads the tickets"\n' >> "$V2D/cori.toml"
V2=$($B deploy "$V2D" --server $S --token "$TJ" --json | version)
wait_for 60 support-desk "client=ready reviewer=confirmed support_agent=confirmed support_lead=confirmed"
V3D=$(copy); printf '\n[auth.roles.reviewer]\ndescription = "Reads the tickets and the refunds"\n' >> "$V3D/cori.toml"
V3=$($B deploy "$V3D" --server $S --token "$TJ" --json | version)
for i in $(seq 1 30); do has '"event":"role.updated".*"role":"reviewer".*"status":200' "$LOG" && break; sleep 1; done
has '"event":"role.updated".*"role":"reviewer".*"status":200' "$LOG"

# the receiver down: the new role waits, and so does the release declaring it; back up, both go
touch "$DOWN"
V4D=$(copy); printf '\n[auth.roles.viewer]\ndescription = "Looks"\n' >> "$V4D/cori.toml"
V4=$($B deploy "$V4D" --server $S --token "$TJ" --json | version)
sleep 2
test "$(catalog_of support-desk)" = "client=ready reviewer=confirmed support_agent=confirmed support_lead=confirmed viewer=pending"
$B identity events --app support-desk --status queued --server $S --token "$TJ" | has 'role.created'
OUT=$($B promote support-desk "$V4" --server $S --token "$TJ" 2>&1 || true); echo "$OUT" | has "not confirmed"
rm "$DOWN"
wait_for 90 support-desk "client=ready reviewer=confirmed support_agent=confirmed support_lead=confirmed viewer=confirmed"
$B promote support-desk "$V4" --server $S --token "$TJ" | has promoted

# the versions that declared `reviewer` go: no kept version declares it, and the IAM hears of it
$B undeploy support-desk "$V2" --server $S --token "$TJ" >/dev/null
test "$(catalog_of support-desk)" = "client=ready reviewer=confirmed support_agent=confirmed support_lead=confirmed viewer=confirmed"
$B undeploy support-desk "$V3" --server $S --token "$TJ" >/dev/null
wait_for 60 support-desk "client=ready support_agent=confirmed support_lead=confirmed viewer=confirmed"
has '"event":"role.deleted".*"role":"reviewer"' "$LOG"

# the catalog an IAM reconciles from, and the events as the CLI shows them
$B identity catalog --server $S --token "$TJ" | has "viewer"
$B identity events --app support-desk --server $S --token "$TJ" | has "role.deleted"

# the conformance kit passes against the dev IdP (which answers app.created 202 twice first)
HOOKSECRET=hooksecret $B identity test-hook http://127.0.0.1:$IP/cori/clients --secret-env HOOKSECRET --wait 30 > "$DATA/kit.txt"
has "0 failed" "$DATA/kit.txt"
# and fails against a receiver that takes any signature
OUT=$($B identity test-hook http://127.0.0.1:$IP/cori/clients --secret-env NO_SUCH_SECRET --wait 1 2>&1 || true); echo "$OUT" | has "not checked: no secret"

# deleting the app: app.deleted through the outbox, with its client
$B apps delete support-desk --confirm support-desk --server $S --token "$TJ" >/dev/null
for i in $(seq 1 30); do has '"app":"support-desk","client_id":"support-desk".*"event":"app.deleted"' "$LOG" && break; sleep 1; done
has '"app":"support-desk","client_id":"support-desk".*"event":"app.deleted"' "$LOG"
test "$(catalog_of support-desk)" = "gone"
echo "e2e-identity: ok"

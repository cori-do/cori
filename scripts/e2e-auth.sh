#!/usr/bin/env bash
# Builtin OIDC auth end to end with the dev IdP: JWT verify, 401 on bad tokens, token exchange into a native activity.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q; cargo build -p cori-server --example dev_idp -q; cargo build -p cori-engine --example billing_worker -q
B=./target/debug/cori; source scripts/lib.sh; E=$(e2e_app); DATA=$(mktemp -d)
P=${CORI_E2E_PORT:-8092}; S=http://127.0.0.1:$P   # the server under test (CORI_E2E_PORT when 8092 is taken)
# the platform database, on the Postgres server CORI_E2E_DATABASE_URL names (else CORI_STORE_DATABASE_URL): an admin URL,
# on which the `cori_e2e_platform` database is created again
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-auth: set CORI_E2E_DATABASE_URL to a Postgres admin URL (the platform database is Postgres)" >&2; exit 1; }
${PSQL:-psql} "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_platform WITH (FORCE)" -c "CREATE DATABASE cori_e2e_platform"
DBURL="${ADMIN%/*}/cori_e2e_platform"
# access tokens live 45 s, like a (short) Keycloak realm: a run waiting on an approval outlives the caller's token
# every app gets its own OIDC client from the org's client hook: the dev IdP's (signed deliveries, client id = the app)
DEV_IDP_TTL=45 CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret ./target/debug/examples/dev_idp >/dev/null 2>&1 & IDP=$!
CORI_FRONTS_URL="http://*.localhost:$P" CORI_AUTH_CLIENT_HOOK=http://127.0.0.1:8099/cori/clients CORI_AUTH_CLIENT_HOOK_SECRET=hooksecret CORI_AUTH_ISSUER=http://127.0.0.1:8099/realms/acme CORI_AUTH_ROLES_CLAIM=roles CORI_SLACK_SIGNING_SECRET=slacksecret CORI_SLACK_USERS="U0SAM=sam:support_lead,approver" $B serve --listen 127.0.0.1:$P --data-dir "$DATA" ${DBURL:+--database-url "$DBURL"} --auth builtin --dev-mocks --no-mock billing >"$DATA/serve.log" 2>&1 & SRV=$!
$B work --queue cori.shared >/dev/null 2>&1 & WK=$!
BW=; trap 'kill $IDP ${BW:-} $SRV $WK 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
kill -0 $SRV 2>/dev/null || { echo "e2e-auth: cori serve exited:" >&2; cat "$DATA/serve.log" >&2; exit 1; }
for i in $(seq 1 40); do curl -sf http://127.0.0.1:8099/realms/acme/.well-known/openid-configuration >/dev/null && break; sleep 0.5; done
I=http://127.0.0.1:8099/realms/acme/protocol/openid-connect/token
TA=$(curl -sf -X POST $I -d 'grant_type=password&username=alice&password=alice&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
TJ=$(curl -sf -X POST $I -d 'grant_type=password&username=jean&password=jean&client_id=cori-cli' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
$B apps create support-desk --server $S --token "$TJ" | has "created"
# a worker pool: an admin creates it and is shown its key once; its heartbeat proves the key and names the queue it polls
PK=$(curl -sf -X POST $S/v1/pools -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"name":"onprem"}' | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")
curl -sf -X POST $S/v1/pools/onprem/heartbeat -H "x-cori-pool-key: $PK" -H 'content-type: application/json' -d '{"capabilities":{}}' | has '"task_queue":"cori.pool.onprem"'
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/pools/onprem/heartbeat -H 'x-cori-pool-key: wrong' -H 'content-type: application/json' -d '{}')" = 401
# the org declares the billing activities: the registry, not the app's cori.toml, gives a hosted connector its audience
$B connectors add activities --yaml $E/billing-activities/activities.yaml --name billing --server $S --token "$TJ" | has "IssueRefund"
# the billing team's worker gets the connector's key, which opens the run's token sealed in each envelope; nobody else does
CK=$($B connectors key billing --server $S --token "$TJ" 2>/dev/null)
test ${#CK} = 64
test "$(curl -s -o /dev/null -w '%{http_code}' $S/v1/connectors/billing/key -H "Authorization: Bearer $TA")" = 403
CORI_CONNECTOR_KEY=$CK ./target/debug/examples/billing_worker >"$DATA/billing.log" 2>&1 & BW=$!
# the hook named the app's client: a user's roles in the app are the token's resource_access.support-desk.roles
curl -sf $S/v1/apps/support-desk -H "Authorization: Bearer $TJ" | has '"client_id":"support-desk"'
# a token of the app's own client (what its front holds) reaches the app's runtime routes, never the platform, even jean's
TF=$(curl -sf -X POST $I -d 'grant_type=password&username=jean&password=jean&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/connectors -H "Authorization: Bearer $TF" -H 'content-type: application/json' -d '{"name":"x","kind":"mcp"}')" = 403
test "$(curl -s -o /dev/null -w '%{http_code}' $S/v1/apps/support-desk/runs -H "Authorization: Bearer $TF")" = 200
HV=$($B deploy $E --server $S --token "$TJ" --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
$B promote support-desk $HV --server $S --token "$TJ" | has promoted
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/apps/support-desk/wf/support_ticket_triage/runs -H 'Authorization: Bearer bad.bad.bad' -H 'content-type: application/json' -d '{"ticket_id":"T-1"}')" = 401
curl -sf -m 60 -X POST $S/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | has '"status":"succeeded"'
RID=$(curl -sf -X POST $S/v1/apps/support-desk/wf/refund_request/runs -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"ticket_id":"T-1042","invoice_id":"INV-42","amount_eur":90}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
for i in $(seq 1 40); do curl -sf $S/v1/apps/support-desk/runs/$RID -H "Authorization: Bearer $TA" | has '"status":"succeeded"' && break; sleep 0.5; done
has "principal_token=opened" "$DATA/billing.log"
# Temporal's history of the run holds the token sealed only, in the bind's result and the envelope: no payload carries a
# JWT (the dev IdP's start `eyJhbGci`)
temporal workflow show -w "$RID" -o json > "$DATA/history.json"
python3 - "$DATA/history.json" <<'PY'
import base64, re, sys
payloads = [base64.b64decode(d).decode("utf8", "replace") for d in re.findall(r'"data":\s*"([^"]+)"', open(sys.argv[1]).read())]
assert sum("cori-sealed.v1." in p for p in payloads) == 2, "the bind's result and the envelope carry the sealed token"
assert not [p for p in payloads if "eyJhbGci" in p], "a token in clear in the run's history"
PY
# a token without a subject or signed with the wrong kind of key is refused (hardened verification)
test "$(curl -s -o /dev/null -w '%{http_code}' $S/v1/me -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIsImtpZCI6ImRldi0xIn0.eyJzdWIiOiJ4In0.x")" = 401
# long runs keep their identity: the refund waits on a lead's approval past the caller's access token lifetime; the
# native step then gets a token exchanged from the run's refreshed identity (refresh token obtained at launch)
RID2=$(curl -sf -X POST $S/v1/apps/support-desk/wf/refund_request/runs -H "Authorization: Bearer $TA" -H 'content-type: application/json' -d '{"ticket_id":"T-1042","invoice_id":"INV-43","amount_eur":350}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
for i in $(seq 1 40); do curl -sf "$S/v1/apps/support-desk/approvals" -H "Authorization: Bearer $(curl -sf -X POST $I -d 'grant_type=password&username=sam&password=sam&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")" | has "$RID2" && break; sleep 0.5; done
sleep 50   # past the 45 s lifetime of alice's token
TA2=$(curl -sf -X POST $I -d 'grant_type=password&username=alice&password=alice&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
TS=$(curl -sf -X POST $I -d 'grant_type=password&username=sam&password=sam&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
AP=$(curl -sf $S/v1/apps/support-desk/approvals -H "Authorization: Bearer $TS" | python3 -c "import sys,json; print([a['id'] for a in json.load(sys.stdin) if a['run_id'] == '$RID2'][0])")
# four eyes: alice started the run and cannot decide its approval, even through the run's decision endpoint
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$S/v1/apps/support-desk/approvals/$AP" -H "Authorization: Bearer $TA2" -H 'content-type: application/json' -d '{"approved":true}')" = 403
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$S/v1/apps/support-desk/runs/$RID2/decisions/approve_refund" -H "Authorization: Bearer $TA2" -H 'content-type: application/json' -d '{"approved":true}')" = 403
curl -sf -X POST "$S/v1/apps/support-desk/approvals/$AP" -H "Authorization: Bearer $TS" -H 'content-type: application/json' -d '{"approved":true}' | has '"ok":true'
for i in $(seq 1 40); do curl -sf $S/v1/apps/support-desk/runs/$RID2 -H "Authorization: Bearer $TA2" | has '"status":"succeeded"' && break; sleep 0.5; done
curl -sf $S/v1/apps/support-desk/runs/$RID2 -H "Authorization: Bearer $TA2" | has '"status":"succeeded"'
# scheduled workflows run as a service account of the issuer: its secret in the broker, its roles from its own token
TJ=$(curl -sf -X POST $I -d 'grant_type=password&username=jean&password=jean&client_id=cori-cli' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
curl -sf $S/v1/apps/support-desk/schedules -H "Authorization: Bearer $TJ" | has '"mode":"disabled"'   # no secret yet
DIGEST_SECRET=digest-secret $B secrets set schedule.schedule-support-digest --from-env DIGEST_SECRET --server $S --token "$TJ" >/dev/null
$B promote support-desk $HV --server $S --token "$TJ" >/dev/null   # going live again re-syncs the schedules
curl -sf $S/v1/apps/support-desk/schedules -H "Authorization: Bearer $TJ" | has '"sub":"service-account-schedule-support-digest"'
curl -sf $S/v1/apps/support-desk/schedules -H "Authorization: Bearer $TJ" | has '"roles":\["support_lead"\]'
# a go-live is a person's decision in the console, Slack or the cli, never through an app's front: alice builds the
# protected app and asks for a release to go live; sam, an approver, neither sees nor decides it with a token of the app's
# own client (what its front holds), and decides it with the cli's
curl -sf -X POST $S/v1/apps/support-desk/protect -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"protected":true}' >/dev/null
curl -sf -X PUT $S/v1/apps/support-desk/builders -H "Authorization: Bearer $TJ" -H 'content-type: application/json' -d '{"builders":["alice"]}' >/dev/null
E2=$DATA/support-desk-2; cp -R "$E" "$E2"; printf '\n# another release\n' >> "$E2/cori.toml"
TAC=$(curl -sf -X POST $I -d 'grant_type=password&username=alice&password=alice&client_id=cori-cli' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
HV2=$($B deploy "$E2" --server $S --token "$TAC" --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
curl -sf -X POST "$S/v1/apps/support-desk/versions/$HV2/promote" -H "Authorization: Bearer $TAC" | has '"status":"pending_approval"'
TSF=$(curl -sf -X POST $I -d 'grant_type=password&username=sam&password=sam&client_id=support-desk' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
TSC=$(curl -sf -X POST $I -d 'grant_type=password&username=sam&password=sam&client_id=cori-cli' | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
curl -sf $S/v1/apps/support-desk/approvals -H "Authorization: Bearer $TSF" | lacks "promote:"
curl -sf $S/v1/apps/support-desk/approvals -H "Authorization: Bearer $TSC" | has "promote:support-desk:$HV2"
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$S/v1/apps/support-desk/approvals/promote:support-desk:$HV2" -H "Authorization: Bearer $TSF" -H 'content-type: application/json' -d '{"approved":true}')" = 403
curl -sf -X POST "$S/v1/apps/support-desk/approvals/promote:support-desk:$HV2" -H "Authorization: Bearer $TSC" -H 'content-type: application/json' -d '{"approved":false}' | has '"approved":false'
# a release's host (v2, not live) signs its person in through the console, never the identity provider: it is its own
# issuer, the console gets a code for it, and the page trades the code (with its PKCE verifier) for a session of its own
RH=support-desk--v2.localhost:$P; CB="http://$RH/apps/support-desk/callback"
enc() { python3 -c "import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=''))" "$1"; }
pkce() { python3 -c "import base64,hashlib,secrets; v=base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b'=').decode(); print(v, base64.urlsafe_b64encode(hashlib.sha256(v.encode()).digest()).rstrip(b'=').decode())"; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
curl -sf -H "Host: $RH" $S/__cori/config.json | python3 -c "import sys,json; o=json.load(sys.stdin)['oidc']; assert o == {'issuer': 'http://$RH/__cori/oidc', 'client_id': 'preview', 'resource': 'http://$RH', 'redirect_uri': '$CB'}, o"
curl -sf -H "Host: $RH" $S/__cori/oidc/.well-known/openid-configuration | has "\"token_endpoint\":\"http://$RH/__cori/oidc/token\""
test "$(code -H "Host: support-desk.localhost:$P" $S/__cori/oidc/.well-known/openid-configuration)" = 404   # the live front signs in with the IdP
read -r VER CH < <(pkce)
curl -s -o /dev/null -w '%{redirect_url}' -H "Host: $RH" "$S/__cori/oidc/authorize?response_type=code&client_id=preview&redirect_uri=$(enc "$CB")&code_challenge=$CH&code_challenge_method=S256&state=s1" | has "^$S/#/preview/support-desk/v2?redirect_uri="
test "$(code -H "Host: $RH" "$S/__cori/oidc/authorize?response_type=code&client_id=preview&redirect_uri=$(enc http://evil.example/cb)&code_challenge=$CH&code_challenge_method=S256")" = 400
# the console's part (sam signed in there: a platform client's token; an app's own client's is refused)
ASK=$(printf '{"code_challenge":"%s","redirect_uri":"%s"}' "$CH" "$CB")
test "$(code -X POST $S/v1/apps/support-desk/versions/v2/preview-codes -H "Authorization: Bearer $TSF" -H 'content-type: application/json' -d "$ASK")" = 403
PC=$(curl -sf -X POST $S/v1/apps/support-desk/versions/v2/preview-codes -H "Authorization: Bearer $TSC" -H 'content-type: application/json' -d "$ASK" | python3 -c "import sys,json; print(json.load(sys.stdin)['code'])")
trade() { curl -s -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=authorization_code&code=$1&code_verifier=$2&redirect_uri=$(enc "$CB")&client_id=preview"; }
SES=$(trade "$PC" "$VER")
AT=$(echo "$SES" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])"); RT=$(echo "$SES" | python3 -c "import sys,json; print(json.load(sys.stdin)['refresh_token'])")
trade "$PC" "$VER" | has '"error":"invalid_grant"'   # a code works once
PC2=$(curl -sf -X POST $S/v1/apps/support-desk/versions/v2/preview-codes -H "Authorization: Bearer $TSC" -H 'content-type: application/json' -d "$ASK" | python3 -c "import sys,json; print(json.load(sys.stdin)['code'])")
trade "$PC2" "wrong-verifier-wrong-verifier-wrong-verifier-0" | has '"error":"invalid_grant"'
# the session reaches the release's runtime on its own host, and nothing else: the live host, the console's, the platform
test "$(code -H "Host: $RH" $S/v1/apps/support-desk/runs -H "Authorization: Bearer $AT")" = 200
test "$(code -H "Host: $RH" $S/v1/apps/support-desk/approvals -H "Authorization: Bearer $AT")" = 200
test "$(code -H "Host: support-desk.localhost:$P" $S/v1/apps/support-desk/runs -H "Authorization: Bearer $AT")" = 403
test "$(code -H "Host: support-desk--v1.localhost:$P" $S/v1/apps/support-desk/runs -H "Authorization: Bearer $AT")" = 403
test "$(code $S/v1/apps/support-desk/runs -H "Authorization: Bearer $AT")" = 403
test "$(code $S/v1/me -H "Authorization: Bearer $AT")" = 403
# an identity provider's token never reaches a release's code there, the app's own client's included
test "$(code -H "Host: $RH" $S/v1/apps/support-desk/runs -H "Authorization: Bearer $TSF")" = 403
# a refresh renews the session on its host only
curl -sf -X POST -H "Host: $RH" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$RT" | has '"access_token"'
curl -s -X POST -H "Host: support-desk--v1.localhost:$P" $S/__cori/oidc/token -d "grant_type=refresh_token&refresh_token=$RT" | has '"error":"invalid_grant"'
curl -s -o /dev/null -w '%{redirect_url}' -H "Host: $RH" "$S/__cori/oidc/logout?post_logout_redirect_uri=$(enc "http://$RH/apps/support-desk/")" | has "^http://$RH/apps/support-desk/$"
echo "e2e-auth: builtin OIDC auth, a signed client hook giving each app its client, 401 on bad and unsigned tokens, token exchange into the native envelope (sealed for the connector's worker: none in Temporal's history), a run that outlives its caller's token, four eyes on approvals, schedules as a verified service account, go-lives never decided through an app's front, a release's host signing its person in through the console — passed"

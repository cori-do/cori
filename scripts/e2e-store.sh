#!/usr/bin/env bash
# Cori Store end to end: needs CORI_STORE_DATABASE_URL (a Postgres admin URL). A deploy gives the version a seeded sandbox per
# store (the live databases untouched); previews and role-scoped tools work on it; a parked change (requires_approval) is
# decided from the platform inbox; promote migrates the live database (empty: seeds never run there) and sandbox writes stay
# behind; reseed restores the sandbox; a protected app parks a builder's promotion until an approver's own `cori promote`; a local
# run gets its own seeded `local` database.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
: "${CORI_STORE_DATABASE_URL:?set CORI_STORE_DATABASE_URL, e.g. postgres://cori@127.0.0.1:55432/postgres}"
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; E=$(e2e_app); DATA=$(mktemp -d); S=http://127.0.0.1:8093
$B up --listen 127.0.0.1:8093 --data-dir "$DATA" --store-database-url "$CORI_STORE_DATABASE_URL" --no-mock tickets >/dev/null 2>&1 & UP=$!
trap 'kill $UP 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
A="Authorization: Bearer dev:alice:support_agent"; L="Authorization: Bearer dev:sam:support_lead,approver"; J="Authorization: Bearer dev:jean:it_admin"
JSON='content-type: application/json'
$B apps create support-desk --builders bo --server $S >/dev/null 2>&1 || true
# deploy: the sandbox is created, migrated and seeded; nothing is promoted, no live database exists yet
HV=$($B deploy $E --server $S --json | python3 -c "import sys,json; d=json.load(sys.stdin); s=d['sandbox'][0]; assert d['status']=='deployed' and s['created'] and s['seeds_applied']==['0001_demo.sql'], d; print(d['hash'])")
SHORT=${HV#sha256:}; SHORT=${SHORT:0:12}
$B store list --app support-desk --server $S | has "@$SHORT"
$B store list --app support-desk --server $S | lacks " live "
$B store tools support_tickets --app support-desk --version $SHORT --server $S --token dev:alice:support_agent | has getCustomer
$B store query support_tickets "select ticket_number from tickets where status = 'open'" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has T-1001
refute $B store query support_tickets "select email from customers" --app support-desk --version $SHORT --server $S --token dev:alice:support_agent 2>/dev/null
refute $B store query support_tickets "select 1" --app support-desk --server $S --token dev:sam:support_lead 2>/dev/null   # no live database before a promotion
# store_browse (platform MCP) pages a table in primary key order
curl -sf -X POST $S/mcp -H "$L" -H "$JSON" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"store_browse\",\"arguments\":{\"app\":\"support-desk\",\"version\":\"$SHORT\",\"store\":\"support_tickets\",\"table\":\"tickets\",\"limit\":2}}}" \
  | python3 -c "import sys,json; r=json.load(sys.stdin)['result']['structuredContent']['rows']; k=[x['ticket_number'] for x in r]; assert len(k)==2 and k==sorted(k), r"
# a preview run reads the sandbox (the worker's store call is routed by the run)
curl -sf -m 60 -X POST $S/v1/apps/support-desk/versions/$HV/wf/support_ticket_triage/runs -H "$L" -H "$JSON" -d '{"ticket_id":"T-1042"}' | has "getCustomer via cori store"
# a parked change in the sandbox: support_agent sets refund_requests.status=approved (requires_approval) → platform inbox → lead approves → executed
SB=$S/v1/apps/support-desk/versions/$HV/stores/support_tickets/mcp
RR=$(curl -sf -X POST $SB -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"createRefundRequest","arguments":{"ticket_id":"T-1010","invoice_id":"INV-10","amount_eur":15,"status":"pending","requested_by":"alice@local"}}}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['structuredContent']['id'])")
curl -sf -X POST $SB -H "$A" -H "$JSON" -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"updateRefundRequest\",\"arguments\":{\"id\":\"$RR\",\"status\":\"approved\"}}}" | has pending_approval
# the inbox shows what was asked, never the change it keeps (the requester's row as their role read it)
AID=$(curl -sf $S/v1/apps/support-desk/approvals -H "$L" | python3 -c "import sys,json; a=[x for x in json.load(sys.stdin) if x['id'].startswith('store:support-desk:')]; assert a and all('change' not in x['payload'] and x['payload']['fields'] for x in a), a; print(a[0]['id'])")
# roles are per app: the same role in another app does not decide a change parked on support-desk's store
$B apps create other-desk --server $S >/dev/null 2>&1 || true
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$S/v1/apps/other-desk/approvals/$AID" -H "$L" -H "$JSON" -d '{"approved":true}')" = 404
curl -sf -X POST "$S/v1/apps/support-desk/approvals/$AID" -H "$L" -H "$JSON" -d '{"approved":true}' | has '"approved":true'
$B store query support_tickets "select status from refund_requests where id = '$RR'" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has approved
# a change is decided once; a rejected one is not applied
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$S/v1/apps/support-desk/approvals/$AID" -H "$L" -H "$JSON" -d '{"approved":true}')" = 404
RR2=$(curl -sf -X POST $SB -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"createRefundRequest","arguments":{"ticket_id":"T-1010","invoice_id":"INV-11","amount_eur":16,"status":"pending","requested_by":"alice@local"}}}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['structuredContent']['id'])")
curl -sf -X POST $SB -H "$A" -H "$JSON" -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"updateRefundRequest\",\"arguments\":{\"id\":\"$RR2\",\"status\":\"approved\"}}}" | has pending_approval
AID2=$(curl -sf $S/v1/apps/support-desk/approvals -H "$L" | python3 -c "import sys,json; print([x for x in json.load(sys.stdin) if x['id'].startswith('store:support-desk:')][0]['id'])")
curl -sf -X POST "$S/v1/apps/support-desk/approvals/$AID2" -H "$L" -H "$JSON" -d '{"approved":false}' | has '"approved":false'
$B store query support_tickets "select status from refund_requests where id = '$RR2'" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has pending
# people never reach the workers' run-scoped path: its handler sends a builder to the app-scoped paths, and a member who
# builds nothing does not get that far
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/stores/support_tickets/mcp -H "Authorization: Bearer dev:bo:builder" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" = 400
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/v1/stores/support_tickets/mcp -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" = 403
# promote: the live database is created and migrated, never seeded; the sandbox row written above is not there
$B promote support-desk $SHORT --server $S --token dev:jean:it_admin | has "1 migration(s) applied: support_tickets/0001_init.sql"
$B store list --app support-desk --server $S | has " live "
$B store query support_tickets "select count(*) as n from tickets" --app support-desk --server $S --token dev:sam:support_lead | has '"n":0'
$B store query support_tickets "select count(*) as n from refund_requests" --app support-desk --server $S --token dev:sam:support_lead | has '"n":0'
$B store query support_tickets "select count(*) as n from refund_requests" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has '"n":2'
# the console's personas. bo builds the app and holds no store role: the live data reads as the inspector (every column,
# every row, no write tool) and each read is audited; a builder of other apps gets nothing
$B store query support_tickets "select count(email) as n from customers" --app support-desk --server $S --token dev:bo:builder | has '"n":0'
TOOLS=$($B store tools support_tickets --app support-desk --server $S --token dev:bo:builder); echo "$TOOLS" | has query; ! echo "$TOOLS" | has createRefundRequest
curl -sf $S/v1/audit -H "$J" | has '"action":"store.inspect"'
# a direct change to live data (a person, outside any workflow run) is on the record too
curl -sf -X POST $S/v1/apps/support-desk/stores/support_tickets/mcp -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"createRefundRequest","arguments":{"ticket_id":"T-1","invoice_id":"INV-1","amount_eur":5,"status":"pending"}}}' | has '"isError":false'
curl -sf $S/v1/audit -H "$J" | has '"action":"store.write"'
refute $B store query support_tickets "select 1 from tickets" --app support-desk --server $S --token dev:zed:builder 2>/dev/null
# nor through the platform MCP's store_browse, which names no store, table or role of an app to a stranger
curl -sf -X POST $S/mcp -H "Authorization: Bearer dev:zed:builder" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"store_browse","arguments":{"app":"support-desk"}}}' | has "is for its owner, builders and users"
# alice uses the app: it is among her apps with her role, the console shows her the user's view, and releases are not hers
curl -sf $S/v1/me -H "$A" | has '"persona":"user"'
curl -sf $S/v1/me -H "Authorization: Bearer dev:bo:builder" | has '"persona":"builder"'
curl -sf $S/v1/me/apps -H "$A" | python3 -c "import sys,json; a=[x for x in json.load(sys.stdin) if x['id']=='support-desk'][0]; assert a['roles']==['support_agent'] and not a['build'], a"
test "$(curl -s -o /dev/null -w '%{http_code}' $S/v1/apps/support-desk/versions -H "$A")" = 403
curl -sf $S/v1/apps/support-desk/stores -H "$A" | python3 -c "import sys,json; s=json.load(sys.stdin); assert s and all(x['target']=='live' for x in s), s"
# what she sees of the data is what her role reaches: the stores where she holds a role, by name; of each, the tables and
# columns her role has a right on, of the roles hers, nothing of the policy. Its builders see it all.
curl -sf $S/v1/apps/support-desk/stores -H "$A" | python3 -c "import sys,json; s=json.load(sys.stdin); assert all('database' not in x and x['roles']==['support_agent'] for x in s), s"
curl -sf -X POST $S/v1/apps/support-desk/stores/support_tickets/mcp -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"cori/describe"}' > "$DATA/describe-alice.json"
curl -sf -X POST $S/v1/apps/support-desk/stores/support_tickets/mcp -H "$J" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"cori/describe"}' > "$DATA/describe-jean.json"
python3 -c "
import sys, json
a, j = (json.load(open(f))['result'] for f in sys.argv[1:3])
cols = lambda d, t: [c['name'] for x in d['tables'] if x['name'] == t for c in x['columns']]
assert cols(a, 'customers') == ['id', 'name', 'plan', 'arr_eur'] and 'body' not in cols(a, 'tickets') and 'created_at' not in cols(a, 'refund_requests'), a
assert [r['name'] for r in a['roles']] == ['support_agent'] and 'policy' not in a and not a['inspector'], a
assert 'email' in cols(j, 'customers') and 'policy' in j and j['inspector'] and len(j['roles']) == 2, j
" "$DATA/describe-alice.json" "$DATA/describe-jean.json"
# she reaches her apps only: the organization's connectors and the platform MCP are not hers
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $S/mcp -H "$A" -H "$JSON" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" = 403
# a second promote applies nothing; the deployment history records the migrations
$B promote support-desk $SHORT --server $S --token dev:jean:it_admin | has "no pending migration"
$B releases support-desk --server $S | has "1 migration(s) on the live stores"
# reseed: the sandbox is dropped and recreated from the migrations and the seed; the changes parked on it are withdrawn
curl -sf -X POST $SB -H "$A" -H "$JSON" -d "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"updateRefundRequest\",\"arguments\":{\"id\":\"$RR2\",\"status\":\"approved\"}}}" | has pending_approval
$B reseed support-desk $SHORT --server $S | has "recreated"
curl -sf $S/v1/apps/support-desk/approvals -H "$L" | python3 -c "import sys,json; a=[x['id'] for x in json.load(sys.stdin) if x['id'].startswith('store:support-desk:')]; assert not a, a"
$B store query support_tickets "select count(*) as n from refund_requests" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has '"n":0'
$B store query support_tickets "select ticket_number from tickets where ticket_number = 'T-1001'" --app support-desk --version $SHORT --server $S --token dev:sam:support_lead | has T-1001
# protected app: a builder's promotion of a new version (no new writes) parks for an approver; the lead's own `cori promote` makes it
# live and closes the request
$B apps protect support-desk --server $S --token dev:jean:it_admin | has "every promotion needs an approver"
V2=$(mktemp -d); cp -R $E $V2/app; sed_i 's/Invoice charged twice/Invoice charged twice (v2)/' $V2/app/fixtures/helpdesk.json
H2=$($B deploy $V2/app --server $S --token dev:bo:builder --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['protected'] and d['needs_approval'] and not d['review']['needs_approval'], d; print(d['hash'])")
$B promote support-desk $H2 --server $S --token dev:bo:builder | has "waits for an approver (protected app)"
curl -sf $S/v1/apps/support-desk/approvals -H "$L" | has '"kind":"promotion"'
$B promote support-desk $H2 --server $S --token dev:sam:support_lead,approver | has "promoted"
curl -sf $S/v1/apps/support-desk/approvals -H "$L" | lacks '"kind":"promotion"'
curl -sf $S/v1/apps/support-desk -H "$J" | has "\"promoted\":\"$H2\""
$B apps protect support-desk --off --server $S --token dev:jean:it_admin | has "not protected"
# the sandbox of the version promoted first is still there (the cap keeps the newest), the live tables unchanged
$B store query support_tickets "select count(*) as n from tickets" --app support-desk --server $S --token dev:sam:support_lead | has '"n":0'
# local run against the same Postgres: its own `local` database, seeded, never the live one
$B run $E/workflows/support_ticket_triage ticket_id=T-1042 --store-database-url "$CORI_STORE_DATABASE_URL" --user alice:support_agent | has "getCustomer via cori store"
$B store query support_tickets "select count(*) as n from customers" --app support-desk --server $S --token dev:sam:support_lead | has '"n":0'
echo "e2e-store: seeded sandbox per deploy, role-scoped tools and guarded SQL on it, preview run routed to it, parked change approved from the inbox, promote migrates an empty live database, sandbox writes stay behind, reseed, protected app parks a promotion that approve executes, local run on its own seeded database — passed"

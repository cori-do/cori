#!/usr/bin/env bash
# The starter template (templates/team-requests) on a server, as a new organization meets it: `cori init`, then
# `cori deploy` with nothing built by hand (its front ships built), and its preview used as a member and a manager.
# Its store is a real one (the embedded Postgres) and its llm step goes to the deployment's starter model (an
# OpenAI-compatible stand-in here), as on a server without dev mocks: the organization has no provider of its own.
# Needs a Temporal dev server on 127.0.0.1:7233 and Postgres's pg_ctl (`cori up` starts its own).
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)   # lib.sh: lacks
P=${CORI_E2E_PORT:-8188}; S=http://127.0.0.1:$P; LP=${CORI_E2E_LLM_PORT:-8191}
for p in $P $LP; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-template: port $p is taken" >&2; exit 1; fi; done
T=dev:jean:it_admin; JSON='content-type: application/json'
ALICE="Authorization: Bearer dev:alice:member"; SAM="Authorization: Bearer dev:sam:manager"
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval('d' + sys.argv[1]))" "$1"; }

# --- the starter model's stand-in: sorts by the prompt's words, each call logged (key, model)
python3 - $LP "$W/calls.jsonl" >"$W/llm.log" 2>&1 <<'PY' &
import http.server, json, sys
port, log = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        prompt = " ".join(m.get("content", "") for m in body.get("messages", []) if isinstance(m.get("content"), str))
        with open(log, "a") as f: f.write(json.dumps({"key": self.headers.get("authorization", ""), "model": body.get("model")}) + "\n")
        category = "travel" if "Lisbon" in prompt else "equipment"
        content = json.dumps({"category": category, "summary": "Sorted by the starter model."})
        out = json.dumps({"choices": [{"message": {"content": content}}], "usage": {"prompt_tokens": 40, "completion_tokens": 12}}).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
LLM=$!

# --- `cori up` with a starter model; the template's store is real (--no-mock requests)
env CORI_LLM_STARTER_MODEL=free-model CORI_LLM_STARTER_BASE_URL=http://127.0.0.1:$LP/v1 CORI_LLM_STARTER_KEY=deployment-key \
  $B up --listen 127.0.0.1:$P --data-dir "$W/up" --no-mock requests >"$W/up.log" 2>&1 & UP=$!
trap 'kill -INT $UP 2>/dev/null || true; kill $LLM 2>/dev/null || true; wait $UP 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
curl -sf $S/healthz >/dev/null || { cat "$W/up.log" >&2; exit 1; }

# --- the app, as `cori init` ships it; registered by init. Under `cori up` dev mocks stay on: without its [llm.dev],
# the llm step goes where it goes on a server without them (no provider of the org's own: the starter)
A="$W/team-requests"
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" $B init "$A" --template team-requests --server $S --token $T --no-git | has "registered"
python3 - "$A/cori.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
open(p, "w").write(re.sub(r"\n\[llm\.dev\][^\[]*", "\n", s))
PY
$B deploy "$A" --server $S --token $T --json > "$W/deploy.json" 2> "$W/deploy.err"
V=$(json "['hash']" < "$W/deploy.json"); PREVIEW=$(json "['preview_url']" < "$W/deploy.json")
lacks "warning" < "$W/deploy.err"    # no stale front: dist/ ships as built
python3 -c "import sys,json; d=json.load(open(sys.argv[1])); assert d['warnings'] == [] and [s['store'] for s in d['sandbox'] if s.get('store')] == ['requests'], d" "$W/deploy.json"

# --- the preview serves the built front: its page, its assets, its config naming the version
curl -sf "$PREVIEW" > "$W/index.html"
has "<title>Team Requests</title>" "$W/index.html"
ASSET=$(python3 -c "import re,sys; print(re.search(r'src=\"\./(assets/[^\"]+\.js)\"', open(sys.argv[1]).read()).group(1))" "$W/index.html")
curl -sf "$PREVIEW$ASSET" | has 'app:.team-requests.'
curl -sf "${PREVIEW}__cori/config.json" | python3 -c "import sys,json; c=json.load(sys.stdin); assert c['app'] == 'team-requests' and c['preview'] and c['version'] == sys.argv[1], c" "$V"

RUNS=$S/v1/apps/team-requests/versions/$V/wf
start() { curl -sf -X POST "$RUNS/submit_request/runs" -H "$1" -H "$JSON" -H "idempotency-key: $2" -d "$3" | json "['run_id']"; }
wait_run() { local h=${2:-$ALICE}; for i in $(seq 1 120); do s=$(curl -sf $S/v1/apps/team-requests/runs/$1 -H "$h" | json "['status']"); [ "$s" != running ] && break; sleep 0.5; done; curl -sf $S/v1/apps/team-requests/runs/$1 -H "$h"; }

# --- a member's small request: sorted by the starter model, approved by the rule, recorded
R1=$(start "$ALICE" k1 '{"title":"USB-C hub","amount_eur":40}')
wait_run "$R1" | python3 -c "import sys,json; r=json.load(sys.stdin); v=r['result']['values']; assert r['status'] == 'succeeded' and v['status'] == 'approved' and v['decided_by'] == 'rule' and v['category'] == 'equipment', r"
python3 - "$W/calls.jsonl" <<'PY'
import json, sys
calls = [json.loads(l) for l in open(sys.argv[1])]
assert calls and all(c == {"key": "Bearer deployment-key", "model": "free-model"} for c in calls), calls
PY
# the same submission again (a double click): the first run, replayed
test "$(start "$ALICE" k1 '{"title":"USB-C hub","amount_eur":40}')" = "$R1"
# a manager files one too: `scope: all` reads every member's rows, and the row it files is its own (requested_by)
R0=$(start "$SAM" k0 '{"title":"Desk lamp","amount_eur":30}')
wait_run "$R0" "$SAM" | python3 -c "import sys,json; r=json.load(sys.stdin); v=r['result']['values']; assert r['status'] == 'succeeded' and v['status'] == 'approved' and v['decided_by'] == 'rule', r"

# --- travel waits for a manager: in the manager's inbox, not the member's; a member cannot decide it
R2=$(start "$ALICE" k2 '{"title":"Conference in Lisbon","amount_eur":300,"details":"Two days"}')
for i in $(seq 1 60); do n=$(curl -sf $S/v1/apps/team-requests/approvals -H "$SAM" | json "[0]['id'] if d else ''" 2>/dev/null || true); [ -n "$n" ] && break; sleep 0.5; done
curl -sf $S/v1/apps/team-requests/approvals -H "$SAM" > "$W/approvals.json"
python3 - "$W/approvals.json" <<'PY'
import json, sys
a = json.load(open(sys.argv[1]))
assert len(a) == 1 and a[0]["workflow"] == "submit_request" and a[0]["step"] == "approve_request", a
ask = a[0]["payload"]["payload"]
assert ask["title"] == "Conference in Lisbon" and ask["category"] == "travel" and ask["requester"] == "alice@local" and ask["reason"].startswith("Travel"), ask
PY
AID=$(json "[0]['id']" < "$W/approvals.json")
# the run, under way: its steps so far (route and record run side by side) and the person it waits for (what the
# front's New request shows)
curl -sf $S/v1/apps/team-requests/runs/$R2 -H "$ALICE" | python3 -c "import sys,json; r=json.load(sys.stdin); assert r['status'] == 'running' and sorted(s['name'] for s in r['steps']) == ['record', 'route', 'triage'] and r['pending_approval']['step'] == 'approve_request' and r['pending_approval']['to'] == 'manager' and r['pending_approval']['id'] == sys.argv[1], r" "$AID"
curl -sf $S/v1/apps/team-requests/approvals -H "$ALICE" | python3 -c "import sys,json; assert json.load(sys.stdin) == []"
test "$(code -X POST $S/v1/apps/team-requests/approvals/$AID -H "$ALICE" -H "$JSON" -d '{"approved":true}')" = 403
curl -sf -X POST $S/v1/apps/team-requests/approvals/$AID -H "$SAM" -H "$JSON" -d '{"approved":false,"note":"Not this quarter"}' >/dev/null
wait_run "$R2" | python3 -c "import sys,json; r=json.load(sys.stdin); v=r['result']['values']; assert r['status'] == 'succeeded' and v['status'] == 'declined' and v['decided_by'] == 'manager' and v['note'] == 'Not this quarter', r"

# --- the store decides who sees which rows: alice her own (2 seeded, 2 new), sam every member's (bob's too)
list() { curl -sf -X POST "$RUNS/list_requests/runs" -H "$1" -H "$JSON" -d "$2"; }
list "$ALICE" '{}' | python3 -c "
import sys,json; r=json.load(sys.stdin); rows=r['outputs']['latest']['rows']
assert r['status'] == 'succeeded' and len(rows) == 4 and {x['requester'] for x in rows} == {'alice@local', 'alice@example.com'}, r
assert [x['status'] for x in rows if x['title'] == 'Conference in Lisbon'] == ['declined'] and all(isinstance(x['amount_eur'], (int, float)) for x in rows), rows"
list "$SAM" '{"status":"declined"}' | python3 -c "
import sys,json; rows=json.load(sys.stdin)['outputs']['latest']['rows']
assert sorted(x['title'] for x in rows) == ['Conference in Lisbon', 'Conference in Lisbon'] and {x['requester'] for x in rows} == {'alice@local', 'bob@example.com'}, rows"
list "$SAM" '{"status":"approved"}' | python3 -c "
import sys,json; rows=json.load(sys.stdin)['outputs']['latest']['rows']
assert [x['requester'] for x in rows if x['title'] == 'Desk lamp'] == ['sam@local'], rows"

# --- the path the plugin's cori-start skill takes: the MCP creates the app from the template and hands a session whose
# bootstrap writes the folder from that template; the agent session deploys it. Every tool has a title and says
# whether it writes.
MB="Authorization: Bearer dev:bo:builder"
mcp() { curl -sf -X POST $S/mcp -H "$MB" -H "$JSON" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
mcp tools/list '{}' | python3 -c "
import sys,json; tools=json.load(sys.stdin)['result']['tools']
assert all(t['title'] and isinstance(t['annotations']['readOnlyHint'], bool) for t in tools), tools
assert sorted(t['name'] for t in tools if not t['annotations']['readOnlyHint']) == ['create_app', 'login', 'request_connector'], tools"
mcp tools/call '{"name":"create_app","arguments":{"id":"team-mcp","template":"team-requests"}}' | python3 -c "import sys,json; r=json.load(sys.stdin)['result']; assert not r.get('isError') and '--template team-requests' in r['structuredContent']['next'], r"
BOOT=$(mcp tools/call '{"name":"login","arguments":{"app":"team-mcp"}}' | python3 -c "import sys,json; sc=json.load(sys.stdin)['result']['structuredContent']; assert sc['bootstrap'].endswith('cori init team-mcp --app team-mcp --template team-requests'), sc; print(sc['bootstrap'])")
BIN=$(cd "$(dirname "$B")" && pwd)
(cd "$W" && env -u CORI_SERVER -u CORI_TOKEN HOME="$W" PATH="$BIN:$PATH" bash -c "$BOOT --no-git" | has "session written")
$B deploy "$W/team-mcp" --json 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['app_id'] == 'team-mcp' and d['preview_url'].endswith('/apps/team-mcp/@v' + str(d['seq']) + '/'), d"

echo "e2e-template: the starter template deploys as init ships it (its front built), sorts with the starter model, approves by its rule or a manager's decision, and its store shows each role its rows — passed"

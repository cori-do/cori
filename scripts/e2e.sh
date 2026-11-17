#!/usr/bin/env bash
# Runs the three demo workflows end to end on a local Temporal dev server with dev mocks.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori
source scripts/lib.sh
E=$(e2e_app)
expect() { local out; out="$("${@:2}" 2>&1)"; echo "$out" >&2; grep -q -- "$1" <<<"$out" || { echo "e2e: expected '$1' in output of: ${*:2}" >&2; exit 1; }; }
# deploy a folder as a version (its own sandbox stores) and promote it as an approver: `ship <app> <dir> [token]`
ship() { local H; H=$($B deploy "$2" --server http://127.0.0.1:8091 --token "${3:-dev:jean:it_admin}" --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])"); $B promote "$1" "$H" --server http://127.0.0.1:8091 --token "${3:-dev:jean:it_admin}"; }
$B check $E   # the app folder: every workflow under workflows/
# a step file that `steps:` does not list, and a listed step without its file, are errors with the fix in the message
LAY=$(mktemp -d); cp -R $E $LAY/app; cp $LAY/app/workflows/bulk_triage/steps/summary.ts $LAY/app/workflows/bulk_triage/steps/extra.ts
expect "steps/extra.ts is not listed in workflow.yaml" bash -c "$B check $LAY/app/workflows/bulk_triage --no-deno 2>&1 || true"
rm $LAY/app/workflows/bulk_triage/steps/extra.ts $LAY/app/workflows/bulk_triage/steps/summary.ts
expect "step \`summary\` is listed in \`steps:\` but steps/summary.ts is missing" bash -c "$B check $LAY/app/workflows/bulk_triage --no-deno 2>&1 || true"
expect "succeeded" $B run $E/workflows/support_ticket_triage ticket_id=T-1042
expect "refunded" $B run $E/workflows/refund_request ticket_id=T-1042 invoice_id=INV-42 amount_eur=350 --decide 'approve_refund={"approved":true,"note":"ok"}'
expect "succeeded" $B run $E/workflows/stale_tickets_digest
# child workflows: bulk_triage maps over ids and runs support_ticket_triage as a Temporal child per item
expect "3 tickets triaged" $B run $E/workflows/bulk_triage 'ticket_ids=["T-1042","T-1001","T-1010"]' --user sam:support_lead
echo "e2e: all four workflows passed (incl. child workflows)"

# --- hosted flow: cori up (background) → deploy → endpoints ---
DATA=$(mktemp -d)
CORI_SMTP_URL=smtp://127.0.0.1:8125 CORI_SMTP_FROM=cori@local $B up --listen 127.0.0.1:8091 --data-dir "$DATA" --no-store >/dev/null 2>&1 &
UP=$!
trap 'kill $UP 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8091/healthz >/dev/null && break; sleep 0.5; done
# apps exist explicitly: an unknown app is refused with the fix; an admin creates it with bo as a builder
expect "unknown app \`support-desk\`" bash -c "CORI_TOKEN=dev:bo:builder $B deploy $E --server http://127.0.0.1:8091 2>&1 || true"
refute env CORI_TOKEN=dev:alice:support_agent $B apps create support-desk --server http://127.0.0.1:8091 >/dev/null 2>&1   # creating needs builder or admin
expect "app support-desk created" $B apps create support-desk --builders bo --server http://127.0.0.1:8091 --token dev:jean:it_admin
expect "already exists" $B apps create support-desk --server http://127.0.0.1:8091 --token dev:jean:it_admin
expect "owned by jean" bash -c "CORI_TOKEN=dev:zed:builder $B deploy $E --server http://127.0.0.1:8091 2>&1 || true"   # a builder who is not a member
# change review gate: a builder deploys a version (first version = 10 new writes; a sandbox, no traffic), its promotion parks for an
# approver, the approver's own `cori promote` makes it live and closes the request; a builder cannot decide it; nothing is live before that
CORI_TOKEN=dev:bo:builder expect "an approver" $B deploy $E --server http://127.0.0.1:8091
FULL=$(curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions -H "Authorization: Bearer dev:jean:it_admin" | python3 -c "import sys,json; print([v['hash'] for v in json.load(sys.stdin) if v['status']=='deployed'][0])")
DRAFT=${FULL:0:23}
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:alice:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}')" = 404
CORI_TOKEN=dev:bo:builder expect "waits for an approver" $B promote support-desk $DRAFT --server http://127.0.0.1:8091
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/approvals -H "Authorization: Bearer dev:sam:support_lead,approver" | grep -q '"kind":"promotion"'
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:8091/v1/apps/support-desk/approvals/promote:support-desk:$FULL" -H "Authorization: Bearer dev:bo:builder" -H 'content-type: application/json' -d '{"approved":true}')" = 403
CORI_TOKEN=dev:sam:support_lead,approver expect "promoted" $B promote support-desk $DRAFT --server http://127.0.0.1:8091
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/approvals -H "Authorization: Bearer dev:sam:support_lead,approver" | lacks '"kind":"promotion"'
curl -sf http://127.0.0.1:8091/v1/apps/support-desk -H "Authorization: Bearer dev:jean:it_admin" | grep -q '"promoted":"sha256:'
A="Authorization: Bearer dev:alice:support_agent"
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "$A" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | grep -q '"status":"succeeded"'
# idempotency: parameters ⇒ the same call again replays the first run instead of starting one
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "$A" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | grep -q '"idempotent_replay":true'
curl -s -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:bob:viewer" -H 'content-type: application/json' -d '{"ticket_id":"T-1"}' | grep -q "requires one of roles"
RID=$(curl -sf -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/refund_request/runs -H "$A" -H 'content-type: application/json' -d '{"ticket_id":"T-1042","invoice_id":"INV-42","amount_eur":350}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
sleep 4
# the approval is routed to support_lead: the agent must not see it, the lead sees and decides it
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/approvals -H "$A" | grep -q '^\[\]$'
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/approvals -H "Authorization: Bearer dev:sam:support_lead" | grep -q '"step":"approve_refund"'
curl -sf -X POST "http://127.0.0.1:8091/v1/apps/support-desk/approvals/$RID:approve_refund" -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"approved":true}' >/dev/null
sleep 3
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RID -H "$A" | grep -q '"status":"succeeded"'
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RID/events -H "$A" | grep -q '"kind":"approval_pending"'
curl -sf http://127.0.0.1:8091/apps/support-desk/__cori/config.json | grep -q '"app":"support-desk"'
# a front's files are read below its dist/ only: a path naming the server's own files, or climbing out, is refused
test "$(curl -s --path-as-is -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/apps/support-desk//etc/hosts)" = 400
test "$(curl -s --path-as-is -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/apps/support-desk/assets/../../cori.toml)" = 400
# the hosted front must fetch its config next to the app, never at the site root
curl -sf http://127.0.0.1:8091/apps/support-desk/ | lacks 'fetch("/__cori'
refute grep -rq 'fetch("/__cori' packages/client/index.ts
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/client.ts -H "Authorization: Bearer dev:jean:it_admin" | grep -q "export const supportTicketTriage"
test "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/v1/apps/support-desk/client.ts -H "$A")" = 403   # the workflow schemas are for the app's builders
# real transports: stdio MCP fixture and a native activity worker on its own queue
expect "succeeded" $B run $E/workflows/support_ticket_triage ticket_id=T-1042 --no-mock helpdesk
cargo build -p cori-engine --example billing_worker -q
./target/debug/examples/billing_worker >/dev/null 2>&1 & BW=$!
trap 'kill $UP $BW 2>/dev/null || true' EXIT
sleep 2
expect "refunded" $B run $E/workflows/refund_request ticket_id=T-77 invoice_id=INV-77 amount_eur=90 --no-mock billing --user alice:support_agent
# LLM bindings: a primary binding routes the step's calls (the mock model `mock-primary` answers from the step's fixture);
# the evaluation loop (shadow bindings, eval, dataset, finetune) is the LLM Ops module's, absent here: the cli says so
J="Authorization: Bearer dev:jean:it_admin"
expect '"binding":"support-desk:support_ticket_triage:classify:primary"' $B llm bind support-desk support_ticket_triage classify --provider mock --model mock-primary --server http://127.0.0.1:8091 --token dev:jean:it_admin
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | grep -q "tier small · mock-primary"
expect "has no LLM Ops" bash -c "$B llm bind support-desk support_ticket_triage classify --provider mock --model small-ft-v1 --mode shadow --server http://127.0.0.1:8091 --token dev:jean:it_admin 2>&1 || true"
expect "has no LLM Ops" bash -c "$B llm eval support-desk --server http://127.0.0.1:8091 --token dev:jean:it_admin 2>&1 || true"
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/meters -H "$J" | grep -q '"llm"'
# rate limit: bulk_triage allows 3/min/principal ⇒ the 4th quick call is a 429 with Retry-After
for i in 1 2 3; do curl -s -o /dev/null -m 90 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/bulk_triage/runs -H "Authorization: Bearer dev:lea:support_lead" -H 'content-type: application/json' -d '{"ticket_ids":["T-1042"]}'; done
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/bulk_triage/runs -H "Authorization: Bearer dev:lea:support_lead" -H 'content-type: application/json' -d '{"ticket_ids":["T-1042"]}')" = 429
# budget: max_duration 0s fails the run between levels with BudgetExceeded (local copy)
BUD=$(mktemp -d); cp -R $E $BUD/app; sed_i 's/budget: { llm_eur_per_run: 0.02, max_duration: 5m }/budget: { max_duration: 0s }/' $BUD/app/workflows/support_ticket_triage/workflow.yaml
expect "exceeded max_duration" bash -c "$B run $BUD/app/workflows/support_ticket_triage ticket_id=T-1042 2>&1 || true"   # a failed run exits 1 by design
# webhooks: a signed run.completed lands on a local receiver (secret from the broker)
HOOK_DIR=$(mktemp -d); python3 - "$HOOK_DIR" <<'PY2' &
import sys, json, hmac, hashlib, http.server
d = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('content-length', 0)))
        sig = self.headers.get('x-cori-signature', '')
        ok = hmac.compare_digest(sig, 'sha256=' + hmac.new(b'hook-secret-1', body, hashlib.sha256).hexdigest())
        open(d + '/hooks.jsonl', 'a').write(json.dumps({"event": self.headers.get('x-cori-event'), "sig_ok": ok, "body": json.loads(body)}) + "\n")
        self.send_response(204); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(('127.0.0.1', 8123), H).serve_forever()
PY2
HOOK=$!
CORI_WEBHOOK_SECRET=hook-secret-1 expect "hook" $B secrets set hook --from-env CORI_WEBHOOK_SECRET --server http://127.0.0.1:8091 --token dev:jean:it_admin
WH=$(mktemp -d); cp -R $E $WH/app; printf '\n[[webhooks]]\nurl = "http://127.0.0.1:8123/cori"\nevents = ["run.completed", "run.failed", "approval.pending"]\nsecret = "hook"\n' >> $WH/app/cori.toml
expect "promoted" ship support-desk $WH/app
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:noa:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' >/dev/null
for i in $(seq 1 20); do grep -q '"event": "run.completed"' $HOOK_DIR/hooks.jsonl 2>/dev/null && break; sleep 0.5; done
grep -q '"event": "run.completed", "sig_ok": true' $HOOK_DIR/hooks.jsonl
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/webhooks/deliveries -H "$J" | grep -q '"status":204'
# slack format: the same receiver gets a Block Kit message with a console button
printf '\n[[webhooks]]\nurl = "http://127.0.0.1:8123/slack"\nevents = ["run.completed"]\nformat = "slack"\n' >> $WH/app/cori.toml
expect "promoted" ship support-desk $WH/app
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:ivo:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' >/dev/null
for i in $(seq 1 20); do grep -q '"blocks"' $HOOK_DIR/hooks.jsonl 2>/dev/null && break; sleep 0.5; done
grep -q '"type": "button"' $HOOK_DIR/hooks.jsonl
# email format: a minimal SMTP sink receives the notification
python3 - "$HOOK_DIR" <<'PY3' &
import asyncio, sys
d = sys.argv[1]
async def handle(r, w):
    w.write(b"220 sink\r\n"); await w.drain(); data=False; buf=[]
    while True:
        line = await r.readline()
        if not line: break
        s = line.decode(errors="replace").rstrip("\r\n")
        if data:
            if s == ".": data=False; open(d+"/mail.txt","a").write("\n".join(buf)+"\n----\n"); buf=[]; w.write(b"250 ok\r\n")
            else: buf.append(s)
        elif s.upper().startswith("EHLO") or s.upper().startswith("HELO"): w.write(b"250-sink\r\n250 8BITMIME\r\n")
        elif s.upper().startswith("DATA"): data=True; w.write(b"354 go\r\n")
        elif s.upper().startswith("QUIT"): w.write(b"221 bye\r\n"); await w.drain(); break
        else: w.write(b"250 ok\r\n")
        await w.drain()
    w.close()
async def main():
    srv = await asyncio.start_server(handle, "127.0.0.1", 8125); await srv.serve_forever()
asyncio.run(main())
PY3
SMTP=$!
printf '\n[[webhooks]]\nurl = "mailto:"\nevents = ["run.completed"]\nformat = "email"\nto = ["sam@acme.example"]\n' >> $WH/app/cori.toml
expect "promoted" ship support-desk $WH/app
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:uma:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' >/dev/null
for i in $(seq 1 30); do grep -q "Subject: \[Cori\] support-desk" $HOOK_DIR/mail.txt 2>/dev/null && break; sleep 0.5; done
grep -q "Subject: \[Cori\] support-desk: support_ticket_triage succeeded" $HOOK_DIR/mail.txt
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/webhooks/deliveries -H "$J" | grep -q '"url":"mailto:sam@acme.example"'
kill $SMTP 2>/dev/null || true
# monthly usage rollup
curl -sf "http://127.0.0.1:8091/v1/usage" -H "$J" | python3 -c "import sys,json; u=json.load(sys.stdin); a=[x for x in u['apps'] if x['app']=='support-desk'][0]; assert a['runs']>3 and a['llm']['calls']>0, a"
kill $HOOK 2>/dev/null || true
expect "promoted" ship support-desk $E
# temporal schedules: the promotion created cori.support-desk.stale_tickets_digest; a manual trigger runs on cori up's workers
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/schedules -H "$J" | grep -q '"mode":"temporal"'
# grep -c reads it all: the action holds the whole run input, and a SIGPIPE would fail the pipeline
temporal schedule describe --schedule-id cori.support-desk.stale_tickets_digest 2>/dev/null | grep -c "Europe/Paris" >/dev/null
# the local Temporal dev server outlives test runs: scheduled workflows left running by earlier sessions (their servers are
# gone) would make the trigger a skipped overlap, so they are terminated first
temporal workflow terminate --query 'WorkflowId STARTS_WITH "sched-" AND ExecutionStatus = "Running"' --reason e2e-cleanup -y >/dev/null 2>&1 || true
sleep 2
temporal schedule trigger --schedule-id cori.support-desk.stale_tickets_digest >/dev/null 2>&1
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs -H "$J" | python3 -c "import sys,json; rs=json.load(sys.stdin); sys.exit(0 if any(r['workflow']=='stale_tickets_digest' and r['status']=='succeeded' and r['user']=='schedule-support-digest' for r in rs) else 1)" && break; sleep 1; done
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs -H "$J" | python3 -c "import sys,json; rs=json.load(sys.stdin); assert any(r['workflow']=='stale_tickets_digest' and r['status']=='succeeded' and r['user']=='schedule-support-digest' for r in rs), [r for r in rs if r['workflow']=='stale_tickets_digest']"
# redaction: the manifest's `redact: [steps.load_ticket.ticket.body]` blanks the value in stored outputs and captured LLM inputs
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/llm/calls -H "$J" | python3 -c "import sys,json; cs=[c for c in json.load(sys.stdin) if c['step']=='classify']; assert cs and cs[0]['input'].get('body')=='<redacted>', cs[:1]"
RID1=$(curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs -H "$J" | python3 -c "import sys,json; print([r for r in json.load(sys.stdin) if r['workflow']=='support_ticket_triage' and r['status']=='succeeded'][0]['run_id'])")
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RID1 -H "$J" | python3 -c "import sys,json; j=json.load(sys.stdin); assert j['outputs']['load_ticket']['ticket']['body']=='<redacted>', j['outputs']['load_ticket']"
# deployed versions, previews (the front of a preview calls the version's endpoints), promotions, rollback
V2=$(mktemp -d); cp -R $E $V2/app; sed_i 's/Invoice charged twice/Invoice charged twice (v2)/' $V2/app/fixtures/helpdesk.json
expect "deployed" $B deploy $V2/app --server http://127.0.0.1:8091 --token dev:jean:it_admin
H2=$(curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions -H "$J" | python3 -c "import sys,json; print([v['hash'] for v in json.load(sys.stdin) if v['status']=='deployed'][0])")
curl -sf "http://127.0.0.1:8091/apps/support-desk/@${H2#sha256:}/" | grep -q 'versions/${cfg.version}/wf'
curl -sf "http://127.0.0.1:8091/apps/support-desk/@${H2#sha256:}/__cori/config.json" | grep -q '"preview":true'
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/v1/apps/support-desk/versions/$H2/wf/support_ticket_triage/runs -H "$A" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}')" = 200   # dev auth: a preview may be tried as the app's personas (api.rs start_preview_run)
curl -sf -m 60 -X POST http://127.0.0.1:8091/v1/apps/support-desk/versions/$H2/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead,approver" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | grep -q "(v2)"
# promoting a newer version passes over the older one still waiting (skipped); a skipped version stays promotable
V3=$(mktemp -d); cp -R $E $V3/app; sed_i 's/Invoice charged twice/Invoice charged twice (v3)/' $V3/app/fixtures/helpdesk.json
H3=$($B deploy $V3/app --server http://127.0.0.1:8091 --token dev:jean:it_admin --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
curl -sf -X POST http://127.0.0.1:8091/v1/apps/support-desk/versions/$H3/promote -H "$J" | python3 -c "import sys,json; r=json.load(sys.stdin); assert r['status']=='promoted' and '$H2' in r['skipped'], r"
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions -H "$J" | python3 -c "import sys,json; v={x['hash']: x for x in json.load(sys.stdin)}; assert v['$H2']['status']=='skipped' and v['$H2']['action']=='promote' and v['$H3']['action'] is None, v['$H2']"
curl -sf -X POST http://127.0.0.1:8091/v1/apps/support-desk/versions/$H2/promote -H "$J" | grep -q '"status":"promoted"'
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions -H "$J" | python3 -c "import sys,json; v=json.load(sys.stdin); assert [x for x in v if x['status']=='promoted'][0]['hash']=='$H2' and [x for x in v if x['status']=='superseded'], v"
expect "rolled back to" $B rollback support-desk --server http://127.0.0.1:8091 --token dev:jean:it_admin
expect "rollback" $B releases support-desk --server http://127.0.0.1:8091 --token dev:jean:it_admin
# releases: numbered in deploy order, one live, `v<N>` names one; going back to a release that was live needs no approval
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions -H "$J" | python3 -c "import sys,json; v=json.load(sys.stdin); assert all(x['release']=='v%d' % x['seq'] for x in v) and [x['section'] for x in v].count('live')==1 and all(x['gate']['state']=='ready' for x in v if x['was_live'] and not x['live']), v"
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/versions/v1 -H "$J" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['release']=='v1' and d['events'][0]['kind'] in ('deployed', 'redeployed'), d"
CORI_TOKEN=dev:bo:builder expect "rolled back to" $B rollback support-desk --server http://127.0.0.1:8091
curl -sf http://127.0.0.1:8091/v1/apps/support-desk -H "$J" | grep -q "$(curl -sf http://127.0.0.1:8091/v1/apps/support-desk/promotions -H "$J" | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['hash'])")"
# console: the app served at /, the apps list and the audit log behind it
curl -sf http://127.0.0.1:8091/ | grep -c "Cori console" >/dev/null   # grep -q would close the pipe early on the 380 KB page (pipefail)
curl -sf http://127.0.0.1:8091/v1/apps -H "$J" | grep -q '"id":"support-desk"'
curl -sf http://127.0.0.1:8091/v1/audit -H "$J" | grep -q '"action":"version.deploy"'
test "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/v1/audit -H "$A")" = 403
# hosted child workflows through the sync endpoint
curl -sf -m 90 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/bulk_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_ids":["T-1042","T-1001"]}' | grep -q "2 tickets triaged"
# children are rows of the run index, linked to their parent
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs -H "$J" | python3 -c "import sys,json; rs=json.load(sys.stdin); assert sum(1 for r in rs if r.get('parent_run') and r['status']=='succeeded') >= 2, [r for r in rs if r.get('parent_run')]"
# connector registry: an admin declares the helpdesk MCP server ahead; an agent discovers it through the cli and the
# remote MCP (schemas, scaffold); a hosted run resolves it from the registry with no cori.toml block.
E_ABS=$(cd $E && pwd)
expect "tools: getTicket, addReply" $B connectors add --name helpdesk --command deno run "$E_ABS/fixtures/helpdesk_mcp.ts" --description "Company helpdesk" --server http://127.0.0.1:8091 --token dev:jean:it_admin
CORI_TOKEN=dev:bo:builder expect "helpdesk" $B connectors list --server http://127.0.0.1:8091
# a member who holds no power and builds no app reaches their apps only: not the connectors, nor the remote MCP
test "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/v1/connectors -H "$A")" = 403
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/mcp -H "$A" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" = 403
# scaffold from the registry: the cli (to disk or --print) and the remote MCP (`scaffold` tool, for hosts without a shell)
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder $B scaffold helpdesk getTicket --workflow support_ticket_triage --dir $E --print | grep -q 'status: z.enum'
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder expect "getTicket" $B connectors find read a ticket
MCP="http://127.0.0.1:8091/mcp"; MA="Authorization: Bearer dev:bo:builder"
curl -sf -X POST $MCP -H "$MA" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' | grep -q '"serverInfo"'
curl -sf -X POST $MCP -H "$MA" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | python3 -c "import sys,json; t={x['name']: x for x in json.load(sys.stdin)['result']['tools']}; assert {'list_apps','get_app','create_app','login','list_connectors','find_tool','scaffold','app_settings','store_browse','preview'} <= set(t), t.keys(); assert t['preview']['_meta']['ui']['resourceUri']=='ui://cori/preview'; assert 'write_file' not in t and 'add_step' not in t"
curl -sf -X POST $MCP -H "$MA" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"scaffold","arguments":{"connector":"helpdesk","tool":"getTicket"}}}' | grep -q 'status: z.enum'
curl -sf -X POST $MCP -H "$MA" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"ui://cori/settings"}}' | grep -q 'text/html;profile=mcp-app'
curl -sf http://127.0.0.1:8091/.well-known/oauth-protected-resource | grep -q '"resource":"http://127.0.0.1:8091/mcp"'
curl -sf http://127.0.0.1:8091/v1/auth/config | grep -q '"mode":"dev"'
# the first steps tick themselves: an MCP host connected (the initialize above), the cli used (every `cori` call so
# far), an app and a release to try (support-desk); and every API answer names the oldest cli the server serves
curl -sf http://127.0.0.1:8091/v1/start -H "Authorization: Bearer dev:jean:it_admin" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['agent']['done'] and d['agent']['detail'] == 'e2e 0' and d['cli']['done'] and d['cli']['detail'].startswith('cori/') and d['app']['done'] and d['release']['done'] and '/@' in d['release']['preview_url'], d"
curl -sf -D - -o /dev/null http://127.0.0.1:8091/healthz | grep -qi '^x-cori-cli-min: '
NOCONN=$(mktemp -d); cp -R $E $NOCONN/app; python3 - "$NOCONN/app/cori.toml" <<'PY2'
import re,sys; p=sys.argv[1]; s=open(p).read(); s=re.sub(r'\[connectors\.helpdesk\]\n(?:.*\n)*?\[connectors\.helpdesk\.dev\]\nmock = "fixtures/helpdesk.json"\n', '', s); assert '[connectors.helpdesk]' not in s; open(p,'w').write(s)
PY2
expect "promoted" ship support-desk $NOCONN/app
curl -sf -m 90 -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:mia:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' | grep -q "getTicket via mcp stdio"
# native activities from activities.yaml: declared as a connector with typed declarations; scaffold carries the version;
# a hosted refund resolves `billing` from the registry (app copy without its billing block) and reaches the real worker
expect "activities: IssueRefund, GetInvoice" $B connectors add activities --yaml $E/billing-activities/activities.yaml --name billing --server http://127.0.0.1:8091 --token dev:jean:it_admin
SC=$(CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder $B scaffold billing IssueRefund --workflow refund_request --dir $E --print)
grep -q 'version: 2' <<<"$SC" && grep -q 'refund_id: z.string()' <<<"$SC"
NOBILL=$(mktemp -d); cp -R $E $NOBILL/app; python3 - "$NOBILL/app/cori.toml" <<'PY2'
import re,sys; p=sys.argv[1]; s=open(p).read(); s=re.sub(r'\[connectors\.billing\]\n(?:.*\n)*?\[connectors\.billing\.dev\]\nmock = "fixtures/billing.json"\n', '', s); assert '[connectors.billing]' not in s; open(p,'w').write(s)
PY2
expect "promoted" ship support-desk $NOBILL/app
RIDB=$(curl -sf -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/refund_request/runs -H "Authorization: Bearer dev:zoe:support_agent" -H 'content-type: application/json' -d '{"ticket_id":"T-1042","invoice_id":"INV-9","amount_eur":50}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RIDB -H "$J" | grep -q '"status":"succeeded"' && break; sleep 0.5; done
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RIDB -H "$J" | grep -q "IssueRefund@2 on cori.acme.native.billing"
expect "removed" $B connectors remove billing --server http://127.0.0.1:8091 --token dev:jean:it_admin
expect "promoted" ship support-desk $E
# an agent starts a new app with the cli alone: the remote MCP gives the app and the session, the folder does the rest
WS=$(mktemp -d); MB="Authorization: Bearer dev:bo:builder"; BA="$PWD/${B#./}"   # absolute: the commands below run from inside the app folder
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_app","arguments":{"id":"hello-app","roles":["member"]}}}' | python3 -c "import sys,json; r=json.load(sys.stdin)['result']; assert not r.get('isError'), r; assert r['structuredContent']['owner']=='bo', r"
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}' | python3 -c "import sys,json; r=json.load(sys.stdin)['result']['structuredContent']; ids=[a['id'] for a in r['apps']]; assert ids==['hello-app','support-desk'], ids"   # bo sees his app and the one he builds
curl -sf -X POST $MCP -H "Authorization: Bearer dev:zed:builder" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}' | python3 -c "import sys,json; r=json.load(sys.stdin)['result']['structuredContent']; assert r['apps']==[], r"   # a non-member sees nothing
LOGIN=$(curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"login","arguments":{"app":"hello-app"}}}')
AT=$(python3 -c "import sys,json; r=json.loads(sys.argv[1])['result']; sc=r['structuredContent']; assert sc['kind']=='agent' and 'approve' not in sc['scopes'] and 'cori init hello-app --app hello-app' in sc['bootstrap'], sc; print(sc['token'])" "$LOGIN")
# a harness credential sink: the token travels in _meta only, never in content
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"login","arguments":{"app":"hello-app"},"_meta":{"cori.dev/credential-sink":"session-file"}}}' | python3 -c "import sys,json; r=json.load(sys.stdin)['result']; c=r['_meta']['cori.dev/credential']; assert c['token'] and 'session_toml' in c, r; assert 'token' not in r['structuredContent'] and r['structuredContent']['delivery']=='sink', r; assert c['token'] not in r['content'][0]['text']"
# an agent token cannot mint another one, nor create apps
curl -sf -X POST $MCP -H "Authorization: Bearer $AT" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"login","arguments":{"app":"hello-app"}}}' | grep -q '"isError":true'
curl -sf -X POST $MCP -H "Authorization: Bearer $AT" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"create_app","arguments":{"id":"other"}}}' | grep -q 'needs a human session'
# the bootstrap form: env once, then .cori/session carries server + token for every later command in the folder
(cd $WS && CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=$AT CORI_SESSION_KIND=agent expect "session written to" $BA init hello-app --app hello-app --roles member)
test -f $WS/hello-app/.cori/session && grep -q 'kind = "agent"' $WS/hello-app/.cori/session && grep -q '^\.cori/' $WS/hello-app/.gitignore
(cd $WS/hello-app && git rev-parse HEAD >/dev/null 2>&1)   # init made a repository with a first commit
(cd $WS/hello-app && expect "created workflows/greet/" $BA workflow new greet --params who:string --roles member)
cat > $WS/hello-app/workflows/greet/steps/greet.ts <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "greet", description: "Greet", input: (ctx: Ctx<"greet">) => ({ who: ctx.params.who }), output: z.object({ text: z.string() }), run: ({ who }) => ({ text: `hello ${who}` }) });
TS
# an unlisted step file is refused with the fix; listing it makes check pass (no env: the session file authenticates)
(cd $WS/hello-app && env -u CORI_SERVER -u CORI_TOKEN bash -c "$BA check workflows/greet --no-deno 2>&1 || true" | grep -q '`steps:` is empty; list the top-level steps')
sed_i 's/steps: \[\]/steps: [greet]/' $WS/hello-app/workflows/greet/workflow.yaml
(cd $WS/hello-app && expect "warnings" env -u CORI_SERVER -u CORI_TOKEN $BA check workflows/greet)
(cd $WS/hello-app && expect "succeeded" env -u CORI_SERVER -u CORI_TOKEN $BA run workflows/greet who=cori)
(cd $WS/hello-app && git add -A && git -c user.name=t -c user.email=t@l commit -qm greet)
# the agent deploys a version (the folder is the default); promoting with the agent session is refused (human action); an admin promotes
(cd $WS/hello-app && expect "deployed" env -u CORI_SERVER -u CORI_TOKEN $BA deploy)
HH=$(curl -sf http://127.0.0.1:8091/v1/apps/hello-app/versions -H "$J" | python3 -c "import sys,json; v=json.load(sys.stdin); assert v and v[0]['status']=='deployed', v; print(v[0]['hash'])")
(cd $WS/hello-app && env -u CORI_SERVER -u CORI_TOKEN bash -c "$BA promote hello-app $HH 2>&1 || true" | grep -q "needs a human session")
curl -sf http://127.0.0.1:8091/v1/apps/hello-app -H "Authorization: Bearer $AT" | grep -q '"owner":"bo"'
curl -sf http://127.0.0.1:8091/v1/me -H "Authorization: Bearer $AT" | grep -q '"kind":"agent"'
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/v1/connectors -H "Authorization: Bearer $AT" -H 'content-type: application/json' -d '{"name":"x","kind":"mcp"}')" = 403
# an agent session holds no platform power and reaches its own app only: not the audit log, not support-desk (which bo
# also builds), not a live run
AJ=$(curl -sf -X POST $MCP -H "$J" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"login","arguments":{"app":"hello-app"}}}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['structuredContent']['token'])")
test "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/v1/audit -H "Authorization: Bearer $AJ")" = 403   # minted by an admin, still no admin power
test "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/v1/apps/support-desk/versions -H "Authorization: Bearer $AT")" = 403
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8091/v1/apps/hello-app/wf/greet/runs -H "Authorization: Bearer $AT" -H 'content-type: application/json' -d '{"who":"x"}')" = 403
expect "promoted" $BA promote hello-app $HH --server http://127.0.0.1:8091 --token dev:jean:it_admin
H1=$(curl -sf http://127.0.0.1:8091/v1/apps/hello-app -H "$J" | python3 -c "import sys,json; print(json.load(sys.stdin)['promoted'])")
test "$H1" = "$HH"
# git metadata rides with the version
curl -sf http://127.0.0.1:8091/v1/audit -H "$J" | python3 -c "import sys,json; a=[x for x in json.load(sys.stdin) if x['action']=='version.deploy' and x['object'].startswith('hello-app@')]; assert a and a[0]['details']['git_commit'], a[:1]"
# pull reproduces the tree and its hash; --require-clean refuses a dirty tree
(cd $WS && expect "pulled hello-app@" $BA pull hello-app ./pulled --server http://127.0.0.1:8091 --token dev:jean:it_admin)
test -f $WS/pulled/workflows/greet/steps/greet.ts && test -f $WS/pulled/.cori/session && (cd $WS/pulled && git rev-parse HEAD >/dev/null)
(cd $WS/pulled && expect "is live (the same content was deployed again) · version $(echo ${H1#sha256:} | cut -c1-12)" env -u CORI_SERVER -u CORI_TOKEN $BA deploy .)   # the same tree: the same release, already live
echo "// change" >> $WS/pulled/workflows/greet/types.ts
(cd $WS/pulled && env -u CORI_SERVER -u CORI_TOKEN bash -c "$BA deploy . --require-clean 2>&1 || true" | grep -q "uncommitted changes")
# the preview and settings tools answer over the remote MCP; settings change only in the folder, so no tool writes them
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"preview","arguments":{"app":"hello-app"}}}' | python3 -c "import sys,json; sc=json.load(sys.stdin)['result']['structuredContent']; assert sc['promoted'] and sc['url'].endswith('/apps/hello-app/') and [w for w in sc['workflows'] if w['id']=='greet'], sc"
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"app_settings","arguments":{"app":"hello-app"}}}' | python3 -c "import sys,json; sc=json.load(sys.stdin)['result']['structuredContent']; assert sc['settings']['auth']['roles']['member']=='group:member', sc['settings']"
curl -sf -X POST $MCP -H "$MB" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":9,"method":"tools/list"}' | python3 -c "import sys,json; t={x['name'] for x in json.load(sys.stdin)['result']['tools']}; assert 'update_app_settings' not in t, t"
# revoking the agent session ends it; the cli says whom to ask
for JTI in $(curl -sf http://127.0.0.1:8091/v1/agent-sessions -H "$MB" | python3 -c "import sys,json; [print(x['jti']) for x in json.load(sys.stdin) if x['app']=='hello-app' and not x['revoked_at']]"); do curl -sf -X DELETE http://127.0.0.1:8091/v1/agent-sessions/$JTI -H "$MB" | grep -q '"revoked"'; done
(cd $WS/hello-app && env -u CORI_SERVER -u CORI_TOKEN bash -c "$BA apps get hello-app 2>&1 || true" | grep -q "call the \`login\` tool")
test -f $WS/hello-app/cori.toml && test ! -d $WS/hello-app/cori
# typed tool inputs: the generated context types `input` against the snapshotted schema; a wrong argument fails check.
# The registry is read with a builder's token: a member who builds nothing is refused it, which check says (typing nothing)
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:alice:support_agent expect "refused the connector registry, so the tool inputs" $B check $E/workflows/support_ticket_triage
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder expect "warnings      0" $B check $E/workflows/support_ticket_triage
grep -q "readonly getTicket: { id: string }" $E/workflows/support_ticket_triage/.cori/context.ts
TYPED=$(mktemp -d); cp -R $E $TYPED/app; sed_i 's/({ id: ctx.params.ticket_id })/({ ticket: ctx.params.ticket_id })/' $TYPED/app/workflows/support_ticket_triage/steps/load_ticket.ts
refute env CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder $B check $TYPED/app/workflows/support_ticket_triage >/dev/null 2>&1
# record: a live registry connector becomes a dev fixture plus an override block; the replay uses the mock
rm -f $NOCONN/app/fixtures/helpdesk.json
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder expect "recorded fixtures/helpdesk.json" $B run $NOCONN/app/workflows/support_ticket_triage ticket_id=T-1042 --record --user alice:support_agent
grep -q '\[connectors.helpdesk.dev\]' $NOCONN/app/cori.toml
CORI_SERVER=http://127.0.0.1:8091 CORI_TOKEN=dev:bo:builder expect "getTicket (mock)" $B run $NOCONN/app/workflows/support_ticket_triage ticket_id=T-1042 --user alice:support_agent
# export to an MCP host config; request inbox
expect "wrote 1 server" $B connectors export --to $NOCONN/app/.mcp.json --server http://127.0.0.1:8091 --token dev:jean:it_admin
curl -sf -X POST http://127.0.0.1:8091/v1/connector-requests -H "Authorization: Bearer dev:bo:builder" -H 'content-type: application/json' -d '{"url":"https://crm.acme.internal/mcp","reason":"customer accounts"}' | grep -q '"queued":true'
expect "crm.acme.internal" $B connectors requests --server http://127.0.0.1:8091 --token dev:jean:it_admin
expect "removed" $B connectors remove helpdesk --server http://127.0.0.1:8091 --token dev:jean:it_admin
# store, when a Postgres is configured (CORI_STORE_DATABASE_URL): checked by e2e-store.sh
# a SIGTERM drains `cori up` (as Kubernetes stops a pod); a run whose watcher went away with the server gets its outcome
# from the reconcile of the restarted one
RIDR=$(curl -sf -X POST http://127.0.0.1:8091/v1/apps/support-desk/wf/refund_request/runs -H "$A" -H 'content-type: application/json' -d '{"ticket_id":"T-1042","invoice_id":"INV-44","amount_eur":350}' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")
for i in $(seq 1 30); do curl -sf http://127.0.0.1:8091/v1/apps/support-desk/approvals -H "Authorization: Bearer dev:sam:support_lead" | grep -q "$RIDR" && break; sleep 0.5; done
kill -TERM $UP; for i in $(seq 1 40); do kill -0 $UP 2>/dev/null || break; sleep 0.5; done
refute kill -0 $UP 2>/dev/null
CORI_SMTP_URL=smtp://127.0.0.1:8125 CORI_SMTP_FROM=cori@local $B up --listen 127.0.0.1:8091 --data-dir "$DATA" --no-store >/dev/null 2>&1 &
UP=$!
trap 'kill $UP $BW 2>/dev/null || true' EXIT
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8091/healthz >/dev/null && break; sleep 0.5; done
curl -sf -X POST "http://127.0.0.1:8091/v1/apps/support-desk/approvals/$RIDR:approve_refund" -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"approved":true}' >/dev/null
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RIDR -H "$A" | grep -q '"status":"succeeded"' && break; sleep 1; done
curl -sf http://127.0.0.1:8091/v1/apps/support-desk/runs/$RIDR -H "$A" | grep -q '"status":"succeeded"'
echo "e2e: hosted flow passed (deploy → promotion parked → approve promotes, sync run, role denial, async run + routed approval, events, front config, client, native activity, LLM bindings + meters, child workflows + run index, redaction, previews on the version's endpoints + promote + rollback, console, temporal schedules, activities from yaml, rate limit, idempotency, budget, webhooks + slack + email, usage, finetune file, connector registry + remote MCP (create_app, login → agent session, cli-only new app, scaffold, preview, settings → new version, revocation), explicit apps with owners, SIGTERM + run outcomes reconciled after a restart, tar.zst bundles + pull + git metadata, layout v3 checks, typed tool inputs, record, export, requests)"

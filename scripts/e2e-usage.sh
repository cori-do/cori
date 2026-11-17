#!/usr/bin/env bash
# Usage and limits (crates/cori-server/src/usage.rs, llm.rs, lifecycle.rs): what the deployment's limits (CORI_MAX_*)
# refuse, the usage ledger that counts runs, deploys and model calls, every LLM call priced by the server itself at the
# org's prices (whatever the worker reports), a budget set aside atomically so that calls running together cannot spend
# it twice, the export of everything, and storage per app (the organization's cap for each app, an app's own limit).
# Needs a Temporal dev server on 127.0.0.1:7233, Postgres's pg_ctl (`cori up` starts its own), and zstd.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)
P=${CORI_E2E_PORT:-8184}; S=http://127.0.0.1:$P; LP=${CORI_E2E_LLM_PORT:-8185}
for p in $P $LP; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-usage: port $p is taken" >&2; exit 1; fi; done
T=dev:jean:it_admin,app_user; H="Authorization: Bearer $T"; JSON='content-type: application/json'
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval('d' + sys.argv[1]))" "$1"; }

# --- an OpenAI-compatible provider: every answer {"label":"ok"}, 10 prompt and 500 completion tokens, two seconds each
python3 - $LP "$W/calls" >"$W/llm.log" 2>&1 <<'PY' &
import http.server, json, sys, threading, time
port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        with lock:
            with open(log, "a") as f: f.write("call\n")
        time.sleep(2)
        body = json.dumps({"choices": [{"message": {"content": json.dumps({"label": "ok"})}}], "usage": {"prompt_tokens": 10, "completion_tokens": 500}}).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
LLM=$!

# --- `cori up` with the deployment's limits; a call sets aside 500 answer tokens
env CORI_MAX_APPS=1 CORI_MAX_VERSIONS_PER_APP=2 CORI_MAX_RUNS_PER_MONTH=9 CORI_MAX_BUNDLE_BYTES=200000 CORI_MAX_STORAGE_BYTES_PER_APP=50000000 CORI_LLM_RESERVE_OUTPUT_TOKENS=500 \
  $B up --listen 127.0.0.1:$P --data-dir "$W/up" --no-store >"$W/up.log" 2>&1 & UP=$!
trap 'kill -INT $UP 2>/dev/null || true; kill $LLM 2>/dev/null || true; wait $UP 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
curl -sf $S/healthz >/dev/null || { cat "$W/up.log" >&2; exit 1; }

# --- the app: one llm step at the org's default small model (no [llm] of its own), a run of its own on every call
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" $B init "$W/ask" --no-register --no-git --front none >/dev/null
python3 - "$W/ask/cori.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
open(p, "w").write(re.sub(r"\n\[llm\][^\[]*", "\n", s))
PY
$B workflow new ask --dir "$W/ask" --endpoint async >/dev/null
sed_i 's/^steps: \[\]/steps: [label]/; s/^  mode: async/  mode: async\
  idempotency: none/' "$W/ask/workflows/ask/workflow.yaml"
cat > "$W/ask/workflows/ask/steps/label.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.llm({ name: "label", description: "Label", tier: "small", input: (_ctx: Ctx<"label">) => ({ q: "hi" }), output: z.object({ label: z.string() }), prompt: ({ q }) => `Label ${q}` });
TS
# the org's provider: 1000 € a million prompt tokens, 2000 € a million answer tokens (a call here: 1.01 €)
curl -sf -X PUT $S/v1/llm/providers/fake -H "$H" -H "$JSON" -d "$(printf '{"kind":"openai","base_url":"http://127.0.0.1:%s/v1","api_key":"k","models":[{"id":"m1","input_eur_per_mtok":1000,"output_eur_per_mtok":2000}]}' $LP)" >/dev/null
curl -sf -X PUT $S/v1/llm/tiers -H "$H" -H "$JSON" -d '{"small":{"provider":"fake","model":"m1"}}' >/dev/null

# --- limits: apps, the size of a bundle, the versions an app keeps
$B apps create ask --server $S --token $T | has "created"
test "$(code -X POST $S/v1/apps -H "$H" -H "$JSON" -d '{"id":"second"}')" = 402
deploy() { $B deploy "$W/ask" --server $S --token $T --json; }
V1=$(deploy | json "['hash']")
head -c 400000 /dev/urandom | base64 > "$W/ask/fixtures/big.txt"
refute deploy >"$W/big.out" 2>&1
has "413" "$W/big.out"
rm "$W/ask/fixtures/big.txt"
echo "// v2" >> "$W/ask/workflows/ask/steps/label.ts"
V2=$(deploy | json "['hash']")
echo "// v3" >> "$W/ask/workflows/ask/steps/label.ts"
refute deploy >"$W/v3.out" 2>&1
has "keeps 2 versions" "$W/v3.out"
sed_i '/^\/\/ v3$/d' "$W/ask/workflows/ask/steps/label.ts"
test "$(deploy | json "['hash']")" = "$V2"   # deploying a version the app has takes no more room
$B promote ask "$V2" --server $S --token $T | has "promoted"

# --- a budget of 3.2 € a month: a call sets aside about 1.03 €, so of five calls running together three go out, the
# others are refused (BudgetExceeded) without reaching the provider
curl -sf -X PUT $S/v1/apps/ask/llm/budget -H "$H" -H "$JSON" -d '{"monthly_eur":3.2}' >/dev/null
start() { curl -sf -X POST $S/v1/apps/ask/wf/ask/runs -H "$H" -H "$JSON" -d '{}' | json "['run_id']"; }
RUNS=""; for i in 1 2 3 4 5; do RUNS="$RUNS $(start)"; done
for r in $RUNS; do for i in $(seq 1 60); do curl -sf $S/v1/apps/ask/runs/$r -H "$H" | has '"status":"running"' || break; sleep 1; done; done
python3 - "$S" "$T" $RUNS <<'PY'
import json, sys, urllib.request
s, t, runs = sys.argv[1], sys.argv[2], sys.argv[3:]
get = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(s + p, headers={"authorization": "Bearer " + t})))
out = [get(f"/v1/apps/ask/runs/{r}") for r in runs]
ok = [o for o in out if o["status"] == "succeeded"]
refused = [o for o in out if o["status"] == "failed" and "BudgetExceeded" in json.dumps(o)]
assert len(ok) == 3 and len(refused) == 2, [(o["status"], o.get("error")) for o in out]
PY
test "$(wc -l < "$W/calls" | tr -d ' ')" = 3
# what the month spent, as the server priced it, and nothing left set aside
curl -sf $S/v1/apps/ask/meters -H "$H" | python3 -c "import sys,json; m=json.load(sys.stdin)['llm']['month']; assert abs(m['spent_eur'] - 3.03) < 1e-6 and m['calls'] == 3 and m['reserved_eur'] == 0 and m['blocked'] is False, m"
# a worker's figure is not trusted: a capture reporting 0 € is priced 1.01 € all the same
R1=$(echo $RUNS | cut -d' ' -f1)
curl -sf -X POST $S/v1/ingest/events -H "$JSON" -d "$(printf '{"run_id":"%s","kind":"llm_call","at":"%s","payload":{"workflow":"ask","step":"label","tier":"small","provider":"fake","model":"m1","mode":"primary","input":{},"output":{},"schema_valid":true,"latency_ms":1,"tokens_in":10,"tokens_out":500,"cost_eur":0}}' "$R1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)")"
curl -sf $S/v1/apps/ask/llm/calls -H "$H" | python3 -c "import sys,json; c=json.load(sys.stdin); assert len(c) == 4 and all(abs(x['cost_eur'] - 1.01) < 1e-9 for x in c), c"
curl -sf $S/v1/apps/ask/meters -H "$H" | python3 -c "import sys,json; m=json.load(sys.stdin)['llm']['month']; assert abs(m['spent_eur'] - 4.04) < 1e-6 and m['blocked'] is True, m"

# --- the month's runs (CORI_MAX_RUNS_PER_MONTH=9): five so far; four more start (their call is refused: the budget is
# spent), the tenth is refused (402)
for i in 6 7 8 9; do start >/dev/null; done
test "$(code -X POST $S/v1/apps/ask/wf/ask/runs -H "$H" -H "$JSON" -d '{}')" = 402

# --- the ledger: the month's counters, the storage the leading replica measured, the limits
for i in $(seq 1 90); do curl -sf "$S/v1/usage" -H "$H" | python3 -c "import sys,json; l=json.load(sys.stdin)['ledger']; sys.exit(0 if l['storage_bytes'] > 0 else 1)" && break; sleep 1; done
curl -sf "$S/v1/usage" -H "$H" | python3 -c "
import sys, json
l = json.load(sys.stdin)['ledger']; c = l['counters']
assert c['runs'] == 9 and c['deploys'] == 3 and c['llm_calls'] == 4 and abs(c['llm_eur'] - 4.04) < 1e-6 and c['llm_tokens_out'] == 2000, c
assert l['storage_bytes'] > 0 and l['limits']['apps'] == 1 and l['limits']['runs_per_month'] == 9 and l['limits']['llm_eur_per_month'] is None, l"

# --- everything, exported: the platform's tables (sealed values withheld), each version's bundle
curl -sf $S/v1/export -H "$H" -o "$W/export.tar.zst"
test "$(code $S/v1/export -H 'Authorization: Bearer dev:ann:app_user')" = 403
python3 - "$W/export.tar.zst" "$V1" "$V2" <<'PY'
import io, json, subprocess, sys, tarfile
t = tarfile.open(fileobj=io.BytesIO(subprocess.run(["zstd", "-dc", sys.argv[1]], capture_output=True, check=True).stdout))
names = t.getnames()
text = lambda n: t.extractfile(n).read().decode()
m = json.loads(text("manifest.json"))
assert m["format"] == "cori-export/1" and m["tables"]["apps"] == 1 and m["tables"]["llm_calls"] == 4 and m["tables"]["usage"] > 0, m["tables"]
for v in sys.argv[2:]: assert f"bundles/{v.replace(':', '_')}.tar.zst" in names, names
secrets = text("tables/secrets.jsonl")
assert "llm.fake" in secrets and "ciphertext" not in secrets and "dek_wrapped" not in secrets, secrets
PY

# --- storage per app: measured with the organization's, under the organization's cap for each app (50 MB here), and an
# app's own limit, lower, set by an admin
curl -sf $S/v1/apps/ask/meters -H "$H" | python3 -c "import sys,json; s=json.load(sys.stdin)['storage']; assert s['bytes'] > 0 and s['per_app'] == 50000000 and s['limit'] == 50000000 and s['own'] is None and s['organization']['bytes'] >= s['bytes'], s"
curl -sf "$S/v1/usage" -H "$H" | python3 -c "import sys,json; l=json.load(sys.stdin)['ledger']; assert l['storage_bytes_by_app']['ask'] > 0 and l['limits']['storage_bytes_per_app'] == 50000000, l"
test "$(code -X PUT $S/v1/apps/ask/limits -H "$H" -H "$JSON" -d '{"storage_bytes":"60MB"}')" = 400
test "$(code -X PUT $S/v1/apps/ask/limits -H 'Authorization: Bearer dev:ann:app_user' -H "$JSON" -d '{"storage_bytes":"1KB"}')" = 403
curl -sf -X PUT $S/v1/apps/ask/limits -H "$H" -H "$JSON" -d '{"storage_bytes":"1KB"}' | python3 -c "import sys,json; s=json.load(sys.stdin); assert s['own'] == 1000 and s['limit'] == 1000, s"
# room for another version, then one more is refused: the app takes more than its own 1 KB
curl -sf -X DELETE "$S/v1/apps/ask/versions/$V1" -H "$H" >/dev/null
echo "// v4" >> "$W/ask/workflows/ask/steps/label.ts"
refute deploy >"$W/v4.out" 2>&1
has '`ask` takes' "$W/v4.out"
# its own limit lifted, the organization's cap for each app still applies, and the version deploys
curl -sf -X PUT $S/v1/apps/ask/limits -H "$H" -H "$JSON" -d '{"storage_bytes":null}' | python3 -c "import sys,json; s=json.load(sys.stdin); assert s['own'] is None and s['limit'] == 50000000, s"
deploy >/dev/null
echo "e2e-usage: limits on apps, bundles, versions and runs; a ledger that counts; LLM calls priced by the server, a budget set aside atomically (three calls of five); everything exported; storage per app — passed"

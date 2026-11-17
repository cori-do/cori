#!/usr/bin/env bash
# The deployment's starter model (CORI_LLM_STARTER_*, crates/cori-server/src/llm.rs): an organization with no provider of
# its own runs its llm steps at once. The starter answers the tiers the org set no default for, on the deployment's key
# (which no run carries and only the starter's endpoint receives), with its one model, a few calls a day; the org's own
# provider takes a tier back. Its model does not hold to the step's schema (prose, then a code fence): it is asked once
# more with the schema in the prompt. The minute's limit is a unit test (llm.rs), not to wait a minute here.
# Needs a Temporal dev server on 127.0.0.1:7233 and Postgres's pg_ctl (`cori up` starts its own).
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)   # lib.sh: lacks, sed_i
P=${CORI_E2E_PORT:-8178}; S=http://127.0.0.1:$P; LP=${CORI_E2E_LLM_PORT:-8179}
for p in $P $LP; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-starter: port $p is taken" >&2; exit 1; fi; done
T=dev:jean:it_admin,app_user; H="Authorization: Bearer $T"; JSON='content-type: application/json'
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval('d' + sys.argv[1]))" "$1"; }

# --- OpenAI-compatible endpoints, each request logged (path, key, model, response_format): /starter/v1, the
# deployment's, whose model answers a json_schema request in prose and a JSON-mode one in a code fence; /own/v1, the org's;
# /busy/v1, /down/v1 and /flaky/v1, the org's when its provider refuses (429, OpenRouter's shape): twice, every time, and
# once before a 400
python3 - $LP "$W/calls.jsonl" >"$W/llm.log" 2>&1 <<'PY' &
import http.server, json, sys
port, log = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    calls = {}
    def answer(self, status, doc):
        out = json.dumps(doc).encode()
        self.send_response(status); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        mode = (body.get("response_format") or {}).get("type")
        key = self.headers.get("authorization", "")
        with open(log, "a") as f:
            f.write(json.dumps({"path": self.path, "key": key, "model": body.get("model"), "mode": mode}) + "\n")
        name = self.path.split("/")[1]
        H.calls[name] = n = H.calls.get(name, 0) + 1
        # its upstream's words and name, which carry the caller's key here: no run may show it
        if name == "down" or (name == "busy" and n <= 2) or (name == "flaky" and n == 1):
            raw = f"{body.get('model')} is temporarily rate-limited upstream ({key})"
            return self.answer(429, {"error": {"message": "Provider returned error", "code": 429, "metadata": {"raw": raw, "provider_name": "Upstream AI"}}})
        if name == "flaky":
            return self.answer(400, {"error": {"message": f"{body.get('model')} does not take this request"}})
        if name == "starter":
            content = "The label is ok." if mode == "json_schema" else "```json\n{\"label\": \"starter\"}\n```"
        else:
            content = json.dumps({"label": name})
        self.answer(200, {"choices": [{"message": {"content": content}}], "usage": {"prompt_tokens": 7, "completion_tokens": 3}})
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
LLM=$!

# --- `cori up` with a starter model: three calls a day for an organization
env CORI_LLM_STARTER_MODEL=free-model CORI_LLM_STARTER_BASE_URL=http://127.0.0.1:$LP/starter/v1 CORI_LLM_STARTER_KEY=deployment-key \
  CORI_LLM_STARTER_CALLS_PER_MINUTE=100 CORI_LLM_STARTER_CALLS_PER_DAY=3 CORI_LLM_STARTER_NOTE="free, shared, rate-limited" \
  $B up --listen 127.0.0.1:$P --data-dir "$W/up" --no-store >"$W/up.log" 2>&1 & UP=$!
trap 'kill -INT $UP 2>/dev/null || true; kill $LLM 2>/dev/null || true; wait $UP 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
curl -sf $S/healthz >/dev/null || { cat "$W/up.log" >&2; exit 1; }

# --- the app: one llm step at the small tier, no [llm] of its own; a run of its own on every call
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" $B init "$W/ask" --no-register --no-git --front none >/dev/null
python3 - "$W/ask/cori.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
open(p, "w").write(re.sub(r"\n\[llm\][^\[]*", "\n", s))
PY
$B workflow new ask --dir "$W/ask" --endpoint sync >/dev/null
sed_i 's/^steps: \[\]/steps: [label]/; s/^  mode: sync/  mode: sync\
  idempotency: none/' "$W/ask/workflows/ask/workflow.yaml"
cat > "$W/ask/workflows/ask/steps/label.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.llm({ name: "label", description: "Label", tier: "small", input: (_ctx: Ctx<"label">) => ({ q: "hi" }), output: z.object({ label: z.string() }), prompt: ({ q }) => `Label ${q}` });
TS
$B apps create ask --server $S --token $T | has "created"
V=$($B deploy "$W/ask" --server $S --token $T --json | json "['hash']")
$B promote ask "$V" --server $S --token $T | has "promoted"

# --- no provider of its own: the first steps need none, the starter model answers every tier
curl -sf $S/v1/start -H "$H" | python3 -c "import sys,json; l=json.load(sys.stdin)['llm']; assert l['done'] and not l['own'] and l['starter']['model'] == 'free-model' and l['starter']['calls_per_day'] == 3 and l['starter']['note'] == 'free, shared, rate-limited', l"
curl -sf $S/v1/llm -H "$H" | python3 -c "import sys,json; s=json.load(sys.stdin)['starter']; assert s['tiers'] == ['small', 'medium', 'large'] and s['calls_today'] == 0, s"

# --- a run: the starter's endpoint, the deployment's key, its one model; prose to a json_schema request, so once more in
# JSON mode with the schema in the prompt, whose fenced answer is the step's output
run() { curl -s -X POST $S/v1/apps/ask/wf/ask/runs -H "$H" -H "$JSON" -d '{}'; }
run > "$W/r1.json"
python3 - "$W/r1.json" "$W/calls.jsonl" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); calls = [json.loads(l) for l in open(sys.argv[2])]
assert r["status"] == "succeeded" and r["outputs"]["label"] == {"label": "starter"}, r
assert [c["mode"] for c in calls] == ["json_schema", "json_object"], calls
assert all(c["path"] == "/starter/v1/chat/completions" and c["key"] == "Bearer deployment-key" and c["model"] == "free-model" for c in calls), calls
PY
# the run carries no key: neither its record nor its captured call
lacks deployment-key < "$W/r1.json"
curl -sf $S/v1/apps/ask/llm/calls -H "$H" | lacks deployment-key
curl -sf $S/v1/apps/ask/llm/calls -H "$H" | python3 -c "import sys,json; c=json.load(sys.stdin); assert c and c[0]['model'] == 'free-model' and c[0]['cost_eur'] == 0 and c[0]['schema_valid'], c"

# --- nobody points the starter elsewhere: another of its models, a tier default on it, a provider of its name
test "$(code -X POST $S/v1/apps/ask/llm/bindings -H "$H" -H "$JSON" -d '{"workflow":"ask","step":"label","provider":"starter","model":"paid-model"}')" = 400
test "$(code -X PUT $S/v1/llm/tiers -H "$H" -H "$JSON" -d '{"small":{"provider":"starter","model":"free-model"}}')" = 400
test "$(code -X PUT $S/v1/llm/providers/starter -H "$H" -H "$JSON" -d '{"kind":"openai","models":[{"id":"x"}]}')" = 400

# --- the day's three calls: two more runs, then the fourth is refused (BudgetExceeded, not retried) before the endpoint
run | has '"status":"succeeded"'
run | has '"status":"succeeded"'
run > "$W/r4.json"
python3 - "$W/r4.json" "$W/calls.jsonl" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); calls = [json.loads(l) for l in open(sys.argv[2])]
assert r["status"] == "failed" and "BudgetExceeded" in json.dumps(r) and "calls a day" in json.dumps(r), r
assert len([c for c in calls if c["path"].startswith("/starter/")]) == 6, calls
PY
curl -sf $S/v1/llm -H "$H" | python3 -c "import sys,json; s=json.load(sys.stdin)['starter']; assert s['calls_today'] == 3, s"

# --- the org's own provider takes the small tier back: its endpoint and its key; the starter keeps the others
curl -sf -X PUT $S/v1/llm/providers/own -H "$H" -H "$JSON" -d "$(printf '{"kind":"openai","base_url":"http://127.0.0.1:%s/own/v1","api_key":"own-key","models":[{"id":"own-model"}]}' $LP)" >/dev/null
curl -sf -X PUT $S/v1/llm/tiers -H "$H" -H "$JSON" -d '{"small":{"provider":"own","model":"own-model"}}' >/dev/null
run > "$W/r5.json"
python3 - "$W/r5.json" "$W/calls.jsonl" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); last = [json.loads(l) for l in open(sys.argv[2])][-1]
assert r["status"] == "succeeded" and r["outputs"]["label"] == {"label": "own"}, r
assert last == {"path": "/own/v1/chat/completions", "key": "Bearer own-key", "model": "own-model", "mode": "json_schema"}, last
PY
curl -sf $S/v1/start -H "$H" | python3 -c "import sys,json; l=json.load(sys.stdin)['llm']; assert l['done'] and l['own'], l"
curl -sf $S/v1/llm -H "$H" | python3 -c "import sys,json; s=json.load(sys.stdin)['starter']; assert s['tiers'] == ['medium', 'large'], s"

# --- the org's provider refuses: Temporal's attempts, counted on the run's trace whether the last is answered, refused as
# the others (three), or refused for good (a 400 on the second: the step says which attempt it was); the run's error then
# says what the provider said (its upstream's words and name), the key blanked
own() { curl -sf -X PUT $S/v1/llm/providers/own -H "$H" -H "$JSON" -d "$(printf '{"kind":"openai","base_url":"http://127.0.0.1:%s/%s/v1","api_key":"own-key","models":[{"id":"own-model"}]}' $LP "$1")" >/dev/null; }
own busy; run > "$W/r6.json"
own down; run > "$W/r7.json"
own flaky; run > "$W/r8.json"
python3 - "$W/r6.json" "$W/r7.json" "$W/r8.json" <<'PY'
import json, sys
busy, down, flaky = (json.load(open(p)) for p in sys.argv[1:])
label = lambda r: [(s["status"], s["attempts"]) for s in r["steps"] if s["name"] == "label"]
assert busy["status"] == "succeeded" and busy["outputs"]["label"] == {"label": "busy"} and label(busy) == [("ok", 3)], busy
assert down["status"] == "failed" and label(down) == [("failed", 3)], down
assert down["error"] == "provider returned 429: own-model is temporarily rate-limited upstream (Bearer <redacted>) (Upstream AI)", down
assert flaky["status"] == "failed" and label(flaky) == [("failed", 2)], flaky
assert flaky["error"] == "provider returned 400: own-model does not take this request", flaky
PY
lacks own-key < "$W/r7.json"

echo "e2e-starter: the starter model answers an organization without a provider (the deployment's key, its one model, three calls a day), a model that ignores the schema is asked again, the org's own provider takes a tier back, a refused call's attempts and reason reach the run — passed"

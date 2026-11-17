#!/usr/bin/env bash
# Files runs are given (crates/cori-server/src/api/files.rs): an upload announced for a workflow's file
# parameter and checked against its limits, its bytes sent once and their type read from them, the run given the
# server's reference (never the bytes), the model reading the file (a PDF as a document) through the worker, downloads
# for who may, a release's files kept to its own runs and removed with it, the generated client's types, and a local
# run reading a file from disk.
# Needs a Temporal dev server on 127.0.0.1:7233 and Postgres's pg_ctl (`cori up` starts its own).
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)
P=${CORI_E2E_PORT:-8186}; S=http://127.0.0.1:$P; LP=${CORI_E2E_LLM_PORT:-8187}
for p in $P $LP; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-files: port $p is taken" >&2; exit 1; fi; done
ADMIN=dev:jean:it_admin; ALICE=dev:alice:app_user; BOB=dev:bob:app_user
JSON='content-type: application/json'
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval('d' + sys.argv[1]))" "$1"; }
auth() { echo "Authorization: Bearer $1"; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
size() { wc -c < "$1" | tr -d ' '; }

# --- an OpenAI-compatible provider that keeps every request it is sent, and answers the receipt's fields
python3 - $LP "$W/llm.jsonl" >"$W/llm.log" 2>&1 <<'PY' &
import http.server, json, sys
port, log = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with open(log, "ab") as f: f.write(body + b"\n")
        out = json.dumps({"choices": [{"message": {"content": json.dumps({"supplier": "ACME", "total_eur": 12.5})}}], "usage": {"prompt_tokens": 1500, "completion_tokens": 10}}).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
LLM=$!

# --- `cori up` with files of at most 1 MiB, on a task queue of its own
env CORI_TASK_QUEUE=local-e2e-files.shared CORI_FILES_MAX_BYTES=1MiB $B up --listen 127.0.0.1:$P --data-dir "$W/up" --no-store >"$W/up.log" 2>&1 & UP=$!
trap 'kill -INT $UP 2>/dev/null || true; kill $LLM 2>/dev/null || true; wait $UP 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done
curl -sf $S/healthz >/dev/null || { cat "$W/up.log" >&2; exit 1; }

# --- the files: a PDF receipt, a zip of the same size, a PNG, a file past the parameter's 512 KB
python3 - "$W" <<'PY'
import sys, os
w = sys.argv[1]
pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n"
open(os.path.join(w, "receipt.pdf"), "wb").write(pdf)
open(os.path.join(w, "all.zip"), "wb").write((b"PK\x03\x04" + b"\x00" * len(pdf))[:len(pdf)])
open(os.path.join(w, "photo.png"), "wb").write(b"\x89PNG\r\n\x1a\n" + b"\x00" * 64)
open(os.path.join(w, "big.pdf"), "wb").write(b"%PDF-1.7\n" + b"a" * 600_000)
PY

# --- the app: a receipt (a PDF or an image, 512 KB at most) the model reads, and up to two attachments
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" $B init "$W/claims" --no-register --no-git --front none >/dev/null
# hosted runs answer from the org's model (no [llm] of the app's own); the local run answers from fixtures/llm.json
python3 - "$W/claims/cori.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
open(p, "w").write(re.sub(r"\n\[llm\][^\[]*", "\n", s))
PY
$B workflow new claim --dir "$W/claims" --endpoint sync >/dev/null
python3 - "$W/claims/workflows/claim/workflow.yaml" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
s = s.replace("parameters:\n  []", """parameters:
  - { name: title, type: string }
  - { name: receipt, type: file, accept: [application/pdf, image/*], max_size: 512KB }
  - { name: attachments, type: files, required: false, max_files: 2 }""")
s = s.replace("steps: []", "steps: [read, check]")
s = s.replace("  mode: sync\n", "  mode: sync\n  idempotency: none\n")
s = s.replace('  headline: "claim"\n  fields: []', '  headline: "{{ steps.read.supplier }}"\n  fields: [{ label: Receipt, path: params.receipt }]')
open(p, "w").write(s)
PY
cat > "$W/claims/workflows/claim/steps/read.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.llm({
  name: "read",
  description: "Read the receipt",
  tier: "small",
  input: (ctx: Ctx<"read">) => ({ receipt: ctx.params.receipt, title: ctx.params.title }),
  files: ({ receipt }) => [receipt],
  output: z.object({ supplier: z.string(), total_eur: z.number() }),
  prompt: ({ receipt, title }) => `Read the receipt ${receipt.name} for "${title}": its supplier and its total in euros.`,
});
TS
cat > "$W/claims/workflows/claim/steps/check.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({
  name: "check",
  description: "Check the receipt's type and the attachments",
  input: (ctx: Ctx<"check">) => ({ receipt: ctx.params.receipt, attachments: ctx.params.attachments }),
  output: z.object({ pdf: z.boolean(), bytes: z.number(), attachments: z.number() }),
  run: ({ receipt, attachments }) => ({ pdf: receipt.content_type === "application/pdf", bytes: receipt.size, attachments: attachments?.length ?? 0 }),
});
TS
printf '{ "read": { "supplier": "LOCAL", "total_eur": 1 } }\n' > "$W/claims/fixtures/llm.json"
$B check "$W/claims" >"$W/check.out" 2>&1 || { cat "$W/check.out" >&2; exit 1; }
# a step other than llm given `files`: refused at check
cp "$W/claims/workflows/claim/steps/check.ts" "$W/check.ts.bak"
sed_i 's/  output: z.object({ pdf/  files: ({ receipt }) => [receipt],\
  output: z.object({ pdf/' "$W/claims/workflows/claim/steps/check.ts"
refute $B check "$W/claims" >"$W/check2.out" 2>&1
has '`files` is for `llm` steps' "$W/check2.out"
cp "$W/check.ts.bak" "$W/claims/workflows/claim/steps/check.ts"

# --- the generated client: a File may be given for a file parameter, the result names the reference
$B gen client --dir "$W/claims" --out "$W/cori.ts" >/dev/null
has 'type RunView, type CoriFile, type CoriFileInput }' "$W/cori.ts"
has 'receipt: CoriFileInput;' "$W/cori.ts"
has 'attachments?: readonly CoriFileInput\[\];' "$W/cori.ts"
has 'receipt: CoriFile;' "$W/cori.ts"

# --- the org's model, the app deployed and promoted
curl -sf -X PUT $S/v1/llm/providers/fake -H "$(auth $ADMIN)" -H "$JSON" -d "$(printf '{"kind":"openai","base_url":"http://127.0.0.1:%s/v1","api_key":"k","models":[{"id":"m1","input_eur_per_mtok":1,"output_eur_per_mtok":2}]}' $LP)" >/dev/null
curl -sf -X PUT $S/v1/llm/tiers -H "$(auth $ADMIN)" -H "$JSON" -d '{"small":{"provider":"fake","model":"m1"}}' >/dev/null
$B apps create claims --server $S --token $ADMIN | has "created"
deploy() { $B deploy "$W/claims" --server $S --token $ADMIN --json; }
V1=$(deploy | json "['hash']")
$B promote claims "$V1" --server $S --token $ADMIN | has "promoted"

# --- an upload announced for the receipt, its bytes sent once
announce() { printf '{"workflow":"claim","parameter":"%s","name":"%s","size":%s,"content_type":"%s"}' "$1" "$2" "$3" "$4"; }
A=$(curl -sf -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce receipt receipt.pdf $(size "$W/receipt.pdf") application/pdf)")
echo "$A" | has '"status":"pending"'
ID=$(echo "$A" | json "['id']"); URL=$(echo "$A" | json "['upload']['url']")
test "$URL" = "/v1/apps/claims/files/$ID/content"
# someone else's upload is unknown to them
test "$(code -X PUT "$S$URL" -H "$(auth $BOB)" --data-binary @"$W/receipt.pdf")" = 404
# fewer bytes than announced: refused, the upload still open
head -c 10 "$W/receipt.pdf" > "$W/short.pdf"
test "$(code -X PUT "$S$URL" -H "$(auth $ALICE)" --data-binary @"$W/short.pdf")" = 400
R=$(curl -sf -X PUT "$S$URL" -H "$(auth $ALICE)" -H 'content-type: application/octet-stream' --data-binary @"$W/receipt.pdf")
echo "$R" | has '"status":"ready"'
test "$(echo "$R" | json "['sha256']")" = "$(sha "$W/receipt.pdf")"
test "$(echo "$R" | json "['content_type']")" = application/pdf
# a file never changes
test "$(code -X PUT "$S$URL" -H "$(auth $ALICE)" --data-binary @"$W/receipt.pdf")" = 409

# --- the parameter's limits, when announced and when the bytes say what they are
test "$(code -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce receipt big.pdf $(size "$W/big.pdf") application/pdf)")" = 413
test "$(code -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce receipt all.zip $(size "$W/all.zip") application/zip)")" = 415
test "$(code -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce title t.txt 3 text/plain)")" = 400
# a zip named .pdf: its bytes are a zip, refused; the right bytes are then taken
F=$(curl -sf -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce receipt fake.pdf $(size "$W/all.zip") application/pdf)")
FURL=$(echo "$F" | json "['upload']['url']")
curl -s -X PUT "$S$FURL" -H "$(auth $ALICE)" --data-binary @"$W/all.zip" -w '\n%{http_code}' > "$W/zip.out"
has '415' "$W/zip.out"; has 'application/zip' "$W/zip.out"
test "$(code -X PUT "$S$FURL" -H "$(auth $ALICE)" --data-binary @"$W/receipt.pdf")" = 200

# --- the run: its parameters hold the server's reference; the model reads the PDF as a document
RUN=$(printf '{"title":"Taxi","receipt":"%s"}' "$ID")
OUT=$(curl -sf -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $ALICE)" -H "$JSON" -d "$RUN")
echo "$OUT" | has '"status":"succeeded"' || { echo "$OUT" >&2; exit 1; }
test "$(echo "$OUT" | json "['outputs']['read']['supplier']")" = ACME
test "$(echo "$OUT" | json "['outputs']['check']['pdf']")" = True
test "$(echo "$OUT" | json "['outputs']['check']['bytes']")" = "$(size "$W/receipt.pdf")"
RID=$(echo "$OUT" | json "['run_id']")
curl -sf $S/v1/apps/claims/runs/$RID -H "$(auth $ALICE)" | python3 -c "
import sys, json
p = json.load(sys.stdin)['params']['receipt']
assert sorted(p) == ['content_type', 'id', 'name', 'sha256', 'size'], p
assert p['id'] == '$ID' and p['name'] == 'receipt.pdf' and p['sha256'] == '$(sha "$W/receipt.pdf")', p
"
python3 - "$W/llm.jsonl" "$W/receipt.pdf" <<'PY'
import sys, json, base64
calls = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
content = calls[-1]["messages"][0]["content"]
pdf = base64.b64encode(open(sys.argv[2], "rb").read()).decode()
assert content[0] == {"type": "file", "file": {"filename": "receipt.pdf", "file_data": "data:application/pdf;base64," + pdf}}, content[0]
assert content[-1]["type"] == "text" and "receipt.pdf" in content[-1]["text"], content[-1]
PY
# a reference handed in as an object is the server's all the same: a name of the caller's own is not kept
RUN2=$(printf '{"title":"Taxi","receipt":{"id":"%s","name":"renamed.exe","size":1,"content_type":"text/plain","sha256":"x"}}' "$ID")
OUT2=$(curl -sf -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $ALICE)" -H "$JSON" -d "$RUN2")
test "$(curl -sf $S/v1/apps/claims/runs/$(echo "$OUT2" | json "['run_id']") -H "$(auth $ALICE)" | json "['params']['receipt']['name']")" = receipt.pdf
# a list of files, two at most
P2=$(curl -sf -X POST $S/v1/apps/claims/files -H "$(auth $ALICE)" -H "$JSON" -d "$(announce attachments photo.png $(size "$W/photo.png") image/png)")
PID=$(echo "$P2" | json "['id']")
curl -sf -X PUT "$S$(echo "$P2" | json "['upload']['url']")" -H "$(auth $ALICE)" --data-binary @"$W/photo.png" | has '"content_type":"image/png"'
OUT3=$(curl -sf -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $ALICE)" -H "$JSON" -d "$(printf '{"title":"Taxi","receipt":"%s","attachments":["%s"]}' "$ID" "$PID")")
test "$(echo "$OUT3" | json "['outputs']['check']['attachments']")" = 1
test "$(code -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $ALICE)" -H "$JSON" -d "$(printf '{"title":"T","receipt":"%s","attachments":["%s","%s","%s"]}' "$ID" "$PID" "$PID" "$PID")")" = 400
# someone else's file: not theirs to run with
curl -s -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $BOB)" -H "$JSON" -d "$RUN" > "$W/bob.out"
has 'of yours' "$W/bob.out"

# --- downloads: who uploaded it and who sees every run, as an attachment that never runs on the host; nobody else
curl -sf -D "$W/dl.headers" -o "$W/dl.pdf" $S/v1/apps/claims/files/$ID/content -H "$(auth $ALICE)"
cmp "$W/dl.pdf" "$W/receipt.pdf"
has -i 'content-disposition: attachment; filename="receipt.pdf"' "$W/dl.headers"
has -i 'content-security-policy: sandbox' "$W/dl.headers"
has -i 'x-content-type-options: nosniff' "$W/dl.headers"
test "$(code $S/v1/apps/claims/files/$ID/content -H "$(auth $BOB)")" = 404
test "$(code $S/v1/apps/claims/files/$ID -H "$(auth $BOB)")" = 404
test "$(code $S/v1/apps/claims/files/$ID/content -H "$(auth $ADMIN)")" = 200
curl -sf $S/v1/apps/claims/files/$ID -H "$(auth $ALICE)" | has '"expires_at":null'   # a run holds it

# --- a release's files: for its own runs only, and gone with it
echo "// v2" >> "$W/claims/workflows/claim/steps/read.ts"
V2=$(deploy | json "['hash']")
RA=$(curl -sf -X POST $S/v1/apps/claims/versions/$V2/files -H "$(auth $ADMIN)" -H "$JSON" -d "$(announce receipt receipt.pdf $(size "$W/receipt.pdf") application/pdf)")
RFID=$(echo "$RA" | json "['id']")
curl -sf -X PUT "$S$(echo "$RA" | json "['upload']['url']")" -H "$(auth $ADMIN)" --data-binary @"$W/receipt.pdf" | has '"status":"ready"'
RRUN=$(printf '{"title":"Preview","receipt":"%s"}' "$RFID")
curl -s -X POST $S/v1/apps/claims/wf/claim/runs -H "$(auth $ADMIN)" -H "$JSON" -d "$RRUN" > "$W/cross.out"
has 'uploaded for release' "$W/cross.out"
curl -sf -X POST $S/v1/apps/claims/versions/$V2/wf/claim/runs -H "$(auth $ADMIN)" -H "$JSON" -d "$RRUN" | has '"status":"succeeded"'
# and a live file is not for a release's runs
LA=$(curl -sf -X POST $S/v1/apps/claims/files -H "$(auth $ADMIN)" -H "$JSON" -d "$(announce receipt receipt.pdf $(size "$W/receipt.pdf") application/pdf)")
curl -sf -X PUT "$S$(echo "$LA" | json "['upload']['url']")" -H "$(auth $ADMIN)" --data-binary @"$W/receipt.pdf" >/dev/null
curl -s -X POST $S/v1/apps/claims/versions/$V2/wf/claim/runs -H "$(auth $ADMIN)" -H "$JSON" -d "$(printf '{"title":"P","receipt":"%s"}' "$(echo "$LA" | json "['id']")")" | has 'uploaded for the live app'
curl -sf -X DELETE "$S/v1/apps/claims/versions/$V2?force=true" -H "$(auth $ADMIN)" >/dev/null
test "$(code $S/v1/apps/claims/files/$RFID -H "$(auth $ADMIN)")" = 404
test "$(code $S/v1/apps/claims/files/$ID -H "$(auth $ADMIN)")" = 200

# --- a local run: the file read from disk, kept in .cori/files/, its type checked before the run
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" CORI_TASK_QUEUE=local-e2e-files.local $B run "$W/claims/workflows/claim" title=Local receipt="$W/receipt.pdf" --mock-llm --no-store >"$W/local.out" 2>&1 || { cat "$W/local.out" >&2; exit 1; }
has 'LOCAL' "$W/local.out"
ls "$W/claims/.cori/files" | has '^file_local'
refute env -u CORI_SERVER -u CORI_TOKEN HOME="$W" $B run "$W/claims/workflows/claim" title=Local receipt="$W/all.zip" --mock-llm --no-store >"$W/local-zip.out" 2>&1
has 'application/zip' "$W/local-zip.out"
echo "e2e-files: ok"

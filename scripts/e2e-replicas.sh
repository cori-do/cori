#!/usr/bin/env bash
# Two server replicas on one platform database, one bundle store and one file store: a version deployed on one replica
# previews, promotes, serves its front and gives its archive back on the other; a file uploaded on one is kept in its own
# bucket, never the bundles', and read back and run with on the other; a sandbox reseeded on one is queried again on the
# other; one replica leads the background work and the other takes over when it stops. The stores are S3 as Cori speaks
# it (signed path-style PUT, GET, DELETE), here a small stand-in that keeps objects in a folder and does not check
# signatures.
# Needs a Temporal dev server on 127.0.0.1:7233 and CORI_E2E_DATABASE_URL (else CORI_STORE_DATABASE_URL): a Postgres admin
# URL, on which the `cori_e2e_replicas` database is created again and the stores are provisioned.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; E=$(e2e_app); W=$(mktemp -d)
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-replicas: set CORI_E2E_DATABASE_URL to a Postgres admin URL" >&2; exit 1; }
PSQL=${PSQL:-psql}
$PSQL "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_replicas WITH (FORCE)" -c "CREATE DATABASE cori_e2e_replicas"
# the deploy must create the sandbox: drop the store databases an earlier run left
printf '%s\n' "SELECT format('DROP DATABASE %I WITH (FORCE)', datname) FROM pg_database WHERE datname LIKE 'cori\_store\_support\_desk%' \gexec" | $PSQL "$ADMIN" -q
PA=${CORI_E2E_PORT_A:-8181}; PB=${CORI_E2E_PORT_B:-8182}; PS=${CORI_E2E_PORT_S3:-8183}
SA=http://127.0.0.1:$PA; SB=http://127.0.0.1:$PB
# the bundle store: a real S3 server that e2e-s3.sh started (CORI_E2E_S3_CONTAINER), else a stand-in that checks no signature
if [ -n "${CORI_E2E_S3_CONTAINER:-}" ]; then
  for p in $PA $PB; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-replicas: port $p is taken" >&2; exit 1; fi; done
  S3_KEY=e2e S3_SECRET=e2e-secret-key; S3=
  objects() { docker exec "$CORI_E2E_S3_CONTAINER" find /gw/bundles -type f; }
  file_objects() { docker exec "$CORI_E2E_S3_CONTAINER" find /gw/files -type f; }
else
for p in $PA $PB $PS; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-replicas: port $p is taken" >&2; exit 1; fi; done
S3_KEY=e2e S3_SECRET=e2e
# objects as the stand-in keeps them: <bucket>__<key, its slashes as __>
objects() { ls "$W/s3" | grep '^bundles__' || true; }
file_objects() { ls "$W/s3" | grep '^files__' || true; }
mkdir -p "$W/s3"
python3 - "$W/s3" $PS >"$W/s3.log" 2>&1 <<'PY' &
import http.server, os, sys, urllib.parse
root, port = sys.argv[1], int(sys.argv[2])
class H(http.server.BaseHTTPRequestHandler):
    def f(self): return os.path.join(root, urllib.parse.urlparse(self.path).path.lstrip('/').replace('/', '__'))
    def do_PUT(self):
        data = self.rfile.read(int(self.headers.get('content-length', 0)))
        open(self.f(), 'wb').write(data); self.send_response(200); self.end_headers()
    def do_GET(self):
        if not os.path.exists(self.f()): self.send_response(404); self.end_headers(); return
        b = open(self.f(), 'rb').read(); self.send_response(200); self.send_header('content-length', str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_DELETE(self):
        if os.path.exists(self.f()): os.remove(self.f())
        self.send_response(204); self.end_headers()
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(('127.0.0.1', port), H).serve_forever()
PY
S3=$!
fi

# two replicas: one platform database, one bundle store, one master key (it seals the stores' logins, among the secrets),
# data dirs of their own
MK=$(openssl rand -hex 32)
replica() {
  RUST_LOG=cori_server=info CORI_MASTER_KEY=$MK CORI_BUNDLE_STORE=s3://bundles/e2e CORI_FILE_STORE=s3://files/e2e CORI_S3_ENDPOINT=http://127.0.0.1:$PS CORI_S3_ACCESS_KEY_ID=$S3_KEY CORI_S3_SECRET_ACCESS_KEY=$S3_SECRET \
    $B serve --listen 127.0.0.1:$1 --data-dir "$W/$2" --database-url "${ADMIN%/*}/cori_e2e_replicas" --store-database-url "$ADMIN" \
    --auth dev --dev-mocks --ingest-token e2e >"$W/$2.log" 2>&1 &
}
replica $PA a; RA=$!
replica $PB b; RB=$!
CORI_WORKER_TOKEN=e2e $B work --queue cori.shared >"$W/worker.log" 2>&1 & WK=$!
CORI_WORKER_TOKEN=e2e $B work --queue cori.pool.sandbox >"$W/sandbox.log" 2>&1 & SX=$!
trap 'kill $RA $RB $WK $SX ${S3:-} 2>/dev/null || true' EXIT
for s in $SA $SB; do for i in $(seq 1 60); do curl -sf $s/healthz >/dev/null && break; sleep 0.5; done; curl -sf $s/healthz >/dev/null; done

J="Authorization: Bearer dev:jean:it_admin"; L="Authorization: Bearer dev:sam:support_lead"; JSON='content-type: application/json'
# a workflow that takes a file, beside the template's
$B workflow new attach --dir "$E" --endpoint sync >/dev/null
python3 - "$E/workflows/attach/workflow.yaml" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
s = s.replace("parameters:\n  []", "parameters:\n  - { name: doc, type: file }").replace("steps: []", "steps: [note]").replace("  mode: sync\n", "  mode: sync\n  idempotency: none\n")
open(p, "w").write(s)
PY
cat > "$E/workflows/attach/steps/note.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "note", description: "Note the file", input: (ctx: Ctx<"note">) => ({ doc: ctx.params.doc }), output: z.object({ name: z.string(), bytes: z.number() }), run: ({ doc }) => ({ name: doc.name, bytes: doc.size }) });
TS
$B apps create support-desk --server $SA --token dev:jean:it_admin | has "created"
# deployed on a: its archive is in the bundle store before the version exists
HV=$($B deploy $E --server $SA --token dev:jean:it_admin --json | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['sandbox'][0]['created'], d; print(d['hash'])")
SHORT=${HV#sha256:}; SHORT=${SHORT:0:12}
objects | has "${HV/:/_}.tar.zst"
test ! -e "$W/b/bundles/${HV/:/_}"
# a preview run on b: b builds it from the bundle store, its worker calls b, its store calls reach a's sandbox
curl -sf -m 60 -X POST $SB/v1/apps/support-desk/versions/$HV/wf/support_ticket_triage/runs -H "$L" -H "$JSON" -d '{"ticket_id":"T-1042"}' | has "getCustomer via cori store"
test -f "$W/b/bundles/${HV/:/_}/cori.toml"
# promoted on b, its front served by a and b, its archive pulled from b
$B promote support-desk $HV --server $SB --token dev:jean:it_admin | has "promoted"
for s in $SA $SB; do curl -sf $s/apps/support-desk/ | has "<"; done
curl -sf $SB/v1/apps/support-desk/versions/$HV/bundle -H "$J" -o "$W/pulled.tar.zst"; test -s "$W/pulled.tar.zst"
# a file announced and sent on a: in the files' bucket, never the bundles'; read back and run with on b
printf 'hello\n' > "$W/note.txt"
FA=$(curl -sf -X POST $SA/v1/apps/support-desk/files -H "$J" -H "$JSON" -d '{"workflow":"attach","parameter":"doc","name":"note.txt","size":6,"content_type":"text/plain"}')
FID=$(echo "$FA" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
curl -sf -X PUT "$SA/v1/apps/support-desk/files/$FID/content" -H "$J" --data-binary @"$W/note.txt" | has '"status":"ready"'
file_objects | has "$FID"
objects | lacks "$FID"
test ! -e "$W/a/files/$FID" && test ! -e "$W/b/files/$FID"
curl -sf "$SB/v1/apps/support-desk/files/$FID/content" -H "$J" -o "$W/note.back"
cmp "$W/note.txt" "$W/note.back"
FRUN=$(printf '{"doc":"%s"}' "$FID")
curl -sf -m 60 -X POST $SB/v1/apps/support-desk/wf/attach/runs -H "$J" -H "$JSON" -d "$FRUN" | has '"bytes":6'
# a opens the sandbox, b reseeds it (drop + create), a sees the new database, not its closed pool
$B store query support_tickets "select ticket_number from tickets where ticket_number = 'T-1001'" --app support-desk --version $SHORT --server $SA --token dev:sam:support_lead | has T-1001
$B reseed support-desk $SHORT --server $SB --token dev:jean:it_admin | has "recreated"
$B store query support_tickets "select ticket_number from tickets where ticket_number = 'T-1001'" --app support-desk --version $SHORT --server $SA --token dev:sam:support_lead | has T-1001
# one leader; when it stops, the other replica takes the lead
LEAD="this replica runs the background work"
test "$(cat "$W/a.log" "$W/b.log" | grep -c "$LEAD")" = 1
if grep -q "$LEAD" "$W/a.log"; then kill $RA; OTHER="$W/b.log"; else kill $RB; OTHER="$W/a.log"; fi
for i in $(seq 1 60); do grep -q "$LEAD" "$OTHER" && break; sleep 1; done
grep -q "$LEAD" "$OTHER"
echo "e2e-replicas: deploy on one replica, preview, promote, front and pull on the other through the bundle store; a file in its own bucket, read and run with on the other; a reseed seen across replicas; leader handover — passed"

#!/usr/bin/env bash
# Builders' code in a sandbox (CORI_SANDBOX_URL): `cori sandbox serve`, and `cori up` whose server and in-process worker
# send it every piece of an app's code. A probe app's steps report where they run: a `cli` program, an MCP server over
# stdio and step code run in the sandbox, in its folders and environment, never the worker's; what a program leaves
# running stops with its step; the deploy-time type check runs there too. Then the sandbox's own door: its token, its
# lease (one app at a time), paths below its bundles. Once idle it starts over: its folders emptied, Cori's runtime files
# written again, free for another lease; the app's next run sends it the bundle again.
# Needs a Temporal dev server on 127.0.0.1:7233 and Postgres's pg_ctl (`cori up` starts its own), and deno.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
logged() { perl -pe 's/\e\[[0-9;]*m//g' "$W/sandbox.log" | has "$1"; }  # the sandbox's log, without its colours
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=./target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)
P=${CORI_E2E_PORT:-8186}; S=http://127.0.0.1:$P
SP=${CORI_E2E_SANDBOX_PORT:-8187}; SB=http://127.0.0.1:$SP
for p in $P $SP; do if lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-sandbox: port $p is taken" >&2; exit 1; fi; done
T=dev:jean:it_admin,app_user
KEY=sandbox-$RANDOM$RANDOM$RANDOM
IDLE=5

# --- the probe app: a `cli` program and an MCP server over stdio say where they run, a code step answers
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" "$B" init "$W/probe" --no-register --no-git --front none >/dev/null
"$B" workflow new probe --dir "$W/probe" --connectors sh,where --roles app_user >/dev/null
sed_i 's/^steps: \[\]/steps: [shell, mcp, mark]/; s/^  mode: sync/  mode: sync\
  idempotency: none/' "$W/probe/workflows/probe/workflow.yaml"   # every call a run of its own
cat >> "$W/probe/cori.toml" <<'TOML'

[connectors.sh]
kind = "cli"
command = ["sh"]

[connectors.where]
kind = "mcp"
command = ["deno", "run", "--quiet", "--allow-read=.", "mcp_where.ts"]
TOML
cat > "$W/probe/mcp_where.ts" <<'TS'
// an MCP server over stdio whose one tool answers with its working folder
const enc = new TextEncoder(); const dec = new TextDecoder(); let buf = "";
const reply = (id: unknown, result: unknown) => Deno.stdout.write(enc.encode(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n"));
for await (const chunk of Deno.stdin.readable) {
  buf += dec.decode(chunk); let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const m = JSON.parse(buf.slice(0, nl)); buf = buf.slice(nl + 1);
    if (m.method === "initialize") await reply(m.id, { protocolVersion: "2025-03-26", capabilities: { tools: {} }, serverInfo: { name: "where", version: "1" } });
    else if (m.method === "tools/call") await reply(m.id, { structuredContent: { cwd: Deno.cwd() }, content: [] });
  }
}
TS
D="$W/probe/workflows/probe/steps"
cat > "$D/shell.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
// its folder, its home, the names in its environment, and a process it leaves behind
export default step.cli({
  name: "shell", description: "Where a program runs", connector: "sh",
  input: (_ctx: Ctx<"shell">) => ({}),
  output: z.object({ pwd: z.string(), home: z.string(), env: z.array(z.string()), left: z.string() }),
  command: () => ["sh", "-c", "pwd; echo \"$HOME\"; env | cut -d= -f1 | tr '\\n' ' '; echo; sleep 300 >/dev/null 2>&1 & echo $!"],
  parse: (stdout: string) => { const [pwd, home, env, left] = stdout.trim().split("\n"); return { pwd, home, env: env.trim().split(" "), left }; },
});
TS
cat > "$D/mcp.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.mcp({
  name: "mcp", description: "Where an MCP server over stdio runs", connector: "where", tool: "where",
  input: (_ctx: Ctx<"mcp">) => ({}),
  output: z.object({ cwd: z.string() }),
});
TS
cat > "$D/mark.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "mark", description: "Answer", input: (_ctx: Ctx<"mark">) => ({}), output: z.object({ ok: z.boolean() }), run: () => ({ ok: true }) });
TS
# a second workflow: a deploy checks all of an app's workflows in one call
"$B" workflow new echo --dir "$W/probe" --roles app_user >/dev/null
sed_i 's/^steps: \[\]/steps: [answer]/' "$W/probe/workflows/echo/workflow.yaml"
cat > "$W/probe/workflows/echo/steps/answer.ts" <<'TS'
import { step } from "@cori-do/sdk";
import type { Ctx } from "../.cori/context";
import { z } from "zod";
export default step.code({ name: "answer", description: "Answer", input: (_ctx: Ctx<"answer">) => ({}), output: z.object({ ok: z.boolean() }), run: () => ({ ok: true }) });
TS

# --- Deno's cache as a container image has it: the runtime's dependencies cached beforehand, read-only, as the
# sandbox pod's root filesystem is
DD="$W/deno"
DENO_DIR="$DD" deno cache --quiet --config packages/runner/deno.json packages/runner/runner.ts packages/runner/schemas.ts
DENO_DIR="$DD" deno cache --quiet --config packages/sdk/deno.json packages/sdk/index.ts
# without a lockfile, as the runner resolves zod: it reads zod's registry entry, which a lockfile install does not wait for
DENO_DIR="$DD" deno cache --quiet --no-lock --config packages/runner/deno.json packages/runner/runner.ts
chmod -R a-w "$DD"

# --- the sandbox, and `cori up` sending it the app's code; each has a variable of its own in its environment
CORI_SANDBOX_TOKEN=$KEY CORI_SANDBOX_IDLE_SECS=$IDLE E2E_SANDBOX_ONLY=1 RUST_LOG=cori_engine=debug DENO_DIR="$DD" \
  $B sandbox serve --listen 127.0.0.1:$SP --work "$W/sandbox" >"$W/sandbox.log" 2>&1 & SBX=$!
env CORI_SANDBOX_URL=$SB CORI_SANDBOX_TOKEN=$KEY E2E_WORKER_ONLY=1 $B up --listen 127.0.0.1:$P --data-dir "$W/up" --no-store >"$W/up.log" 2>&1 & UP=$!
down() {
  local rc=$?
  kill -INT $UP 2>/dev/null || true; kill $SBX 2>/dev/null || true; wait $UP 2>/dev/null || true; wait $SBX 2>/dev/null || true; chmod -R u+w "$DD" 2>/dev/null || true
  # a failure shows what the sandbox and the server logged: a runner that would not start says why there (its stderr)
  if [ "$rc" != 0 ]; then for f in sandbox up; do echo "--- $f.log (last 80 lines)" >&2; perl -pe 's/\e\[[0-9;]*m//g' "$W/$f.log" | tail -80 >&2; done; fi
}
trap down EXIT
for i in $(seq 1 120); do curl -sf $S/healthz >/dev/null && curl -sf $SB/healthz >/dev/null && break; sleep 0.5; done
curl -sf $S/healthz >/dev/null || { cat "$W/up.log" >&2; exit 1; }
curl -sf $SB/healthz >/dev/null || { cat "$W/sandbox.log" >&2; exit 1; }
WORK=$(cd "$W/sandbox" && pwd -P)

# --- deploy: the type check and the schema export run in the sandbox, under the app's lease
$B apps create probe --server $S --token $T | has "created"
H=$($B deploy "$W/probe" --server $S --token $T --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
# one call for both workflows, none per workflow
logged 'lease=/probe path=/v1/check-app'
refute logged 'path=/v1/check$'
$B promote probe "$H" --server $S --token $T | has "promoted"
# a type error is found there, and refuses the version: the workflows do not pass together, so they are checked one at a
# time, which names the one that fails
cp "$D/mark.ts" "$W/mark.ts.ok"
sed_i 's/({ ok: true })/({ ok: "yes" })/' "$D/mark.ts"
refute $B deploy "$W/probe" --server $S --token $T >"$W/broken.out" 2>&1
has -i "workflows/probe: type check" "$W/broken.out"
logged 'lease=/probe path=/v1/check$'
cp "$W/mark.ts.ok" "$D/mark.ts"

# --- a run: every step's code in the sandbox, its programs in its folders and environment
run() { curl -sf -X POST $S/v1/apps/probe/wf/probe/runs -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{}'; }
run > "$W/run1.json"
python3 - "$W/run1.json" "$WORK" <<'PY'
import json, sys
r, work = json.load(open(sys.argv[1])), sys.argv[2]
assert r["status"] == "succeeded", r
o = r["outputs"]
sh, mcp = o["shell"], o["mcp"]
assert sh["pwd"].startswith(work + "/runs/"), f"the cli program ran outside the sandbox's folders: {sh['pwd']}"
assert sh["home"] == sh["pwd"], sh
assert "E2E_SANDBOX_ONLY" in sh["env"] and "E2E_WORKER_ONLY" not in sh["env"], f"the program's environment is not the sandbox's: {sh['env']}"
assert not [k for k in sh["env"] if k.startswith("CORI_")], f"Cori's variables reached the program: {sh['env']}"
assert mcp["cwd"].startswith(work + "/bundles/"), f"the MCP server ran outside the sandbox: {mcp['cwd']}"
assert o["mark"] == {"ok": True}, o["mark"]
PY
logged 'lease=/probe path=/v1/runner'
logged 'lease=/probe path=/v1/exec'
logged 'lease=/probe path=/v1/mcp'
LEFT=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['outputs']['shell']['left'])" "$W/run1.json")
# what the program left running stopped with its step
refute kill -0 "$LEFT" 2>/dev/null
# Deno's cache is the work folder's (it keeps its transpiled modules there), its packages the read-only cache's, linked
test -L "$WORK/deno/npm" && test -d "$WORK/deno/gen"
# the workflow's next run takes the runner started ahead for it, which ran no step's code
run > "$W/run1b.json"
python3 -c "import json,sys; r=json.load(open(sys.argv[1])); assert r['status'] == 'succeeded', r" "$W/run1b.json"
logged "the workflow's spare taken"

# --- the sandbox's door: its token, one lease at a time, paths below its bundles
call() {  # <path> <lease> <body> [token] → the status code
  curl -s -o "$W/call.out" -w '%{http_code}' -X POST "$SB$1" -H 'content-type: application/json' -H "x-cori-lease: $2" -H "authorization: Bearer ${4-$KEY}" -d "$3"
}
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST $SB/v1/lease -H 'x-cori-lease: /probe' -d '{}')" = 401
test "$(call /v1/lease /probe '{}' not-the-key)" = 401
test "$(call /v1/lease /probe '{}')" = 200
test "$(call /v1/lease /other '{}')" = 409
test "$(call /v1/lease /probe '{}')" = 200
test "$(call /v1/runner /probe "$(printf '{"bundle":"%s","workflow":"../../../..","req":{}}' "$H")")" = 400
test "$(call /v1/exec /probe '{"bundle":"../x","workflow":"workflows/probe","argv":["sh"]}')" = 400

# --- idle, it starts over: what a lease wrote is gone (even in Cori's runtime files), the runtime is written again
RT=$(ls -d "$WORK"/cache/runtime/*/ | head -1)
echo 'export const planted = true;' > "$RT/runner/planted.ts"
for i in $(seq 1 $((IDLE * 4 + 20))); do [ -d "$WORK/bundles" ] || break; sleep 0.5; done
logged 'sandbox started over'
test "$(ls "$WORK" | sort | tr '\n' ' ')" = "cache deno home tmp "
test "$(ls "$WORK/deno")" = "npm" && test -L "$WORK/deno/npm"
test -f "$RT/runner/runner.ts"
refute test -e "$RT/runner/planted.ts"

# --- the next run sends the bundle again, and once idle the sandbox takes another lease
run > "$W/run2.json"
python3 -c "import json,sys; r=json.load(open(sys.argv[1])); assert r['status'] == 'succeeded' and not r.get('idempotent_replay'), r" "$W/run2.json"
perl -pe 's/\e\[[0-9;]*m//g' "$W/sandbox.log" | sed -n '/sandbox started over/,$p' | has 'lease=/probe path=/v1/bundle'
sleep $((IDLE + 4))
test "$(call /v1/lease /other '{}')" = 200
echo "e2e-sandbox: steps, programs, MCP servers and the deploy checks run in the sandbox (its folders and environment, never the worker's); a program's leftovers stop with its step; the sandbox answers its token only, one lease at a time, below its bundles; idle, it starts over clean — passed"

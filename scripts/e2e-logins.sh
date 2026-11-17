#!/usr/bin/env bash
# Your logins, one per server and organization (`cori login`), and app folders that each work with a server (and an
# organization) of their own, against four stand-in servers (`staging`, `prod`, `other`, and `multi`, which hosts two
# organizations) that answer as whom each request came: the sequence that once sent `cori init` to staging right after a
# login to prod, a stored token that never reaches another server, and a folder's commands that never act in another
# organization.
# Needs nothing running (no Temporal, no Postgres).
set -euo pipefail
has() { grep -c "$@" >/dev/null; }  # reads all input: no SIGPIPE for the producer
cd "$(dirname "$0")/.."
cargo build -p cori-cli -q
B=$PWD/target/debug/cori; source scripts/lib.sh; W=$(mktemp -d)   # lib.sh: refute, lacks

# --- the stand-ins: the app routes init and `apps list` call, each request logged as `<server> <method> <path> <token>`
python3 - "$W" >"$W/servers.log" 2>&1 <<'PY' &
import http.server, json, sys, threading
w = sys.argv[1]
# a token names its organization on `multi` (`acme-…`, `globex-…`); the other servers host one, and name none
def org_of(name, token):
    return {"id": "o-" + token.split("-")[0], "slug": token.split("-")[0]} if name == "multi" and token.split("-")[0] in ("acme", "globex") else None
def server(name):
    apps = {}
    class H(http.server.BaseHTTPRequestHandler):
        def answer(self, code, body):
            out = json.dumps(body).encode()
            self.send_response(code); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
        def handle_one(self, method):
            token = self.headers.get("authorization", "").removeprefix("Bearer ").strip()
            body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
            with open(f"{w}/requests", "a") as f: f.write(f"{name} {method} {self.path} {token or '-'}\n")
            if self.path == "/healthz": return self.answer(200, {"ok": True})
            if not token: return self.answer(401, {"error": "no token"})
            org = org_of(name, token)
            # a folder's command names its organization; the server refuses a token of another one, as a tenancy does
            want = self.headers.get("x-cori-organization")
            if name == "multi":
                with open(f"{w}/organizations", "a") as f: f.write(f"{method} {self.path} {token} {want or '-'}\n")
            if want and org and want != org["id"]: return self.answer(409, {"error": f"this command is for organization `{want}`, and your token is for another organization (`{org['id']}`): sign in to the folder's organization"})
            if self.path == "/v1/me": return self.answer(200, dict({"sub": token, "roles": ["owner"]}, **({"organization": org} if org else {})))
            if self.path == "/v1/me/organizations": return self.answer(200, {"organizations": [{"id": "o-acme", "slug": "acme"}, {"id": "o-globex", "slug": "globex"}] if org else []})
            mine = apps.setdefault(org["id"] if org else "", set())
            if self.path == "/v1/apps" and method == "GET": return self.answer(200, [{"id": a, "owner": token} for a in sorted(mine)] if org else [{"id": name, "owner": token}])
            if self.path == "/v1/apps" and method == "POST": mine.add(body["id"]); return self.answer(201, {"id": body["id"], "owner": token})
            app = self.path.removeprefix("/v1/apps/")
            if app in mine: return self.answer(200, {"id": app, "owner": token, "roles": []})
            return self.answer(404, {"error": f"unknown app `{app}`"})
        def do_GET(self): self.handle_one("GET")
        def do_POST(self): self.handle_one("POST")
        def log_message(self, *a): pass
    s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s.server_address[1]
ports = [server(n) for n in ("staging", "prod", "other", "multi")]
with open(f"{w}/ports.tmp", "w") as f: f.write(" ".join(map(str, ports)) + "\n")
import os; os.rename(f"{w}/ports.tmp", f"{w}/ports")
threading.Event().wait()
PY
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT
for _ in $(seq 100); do [ -s "$W/ports" ] && break; sleep 0.1; done
read -r PS PP PO PM < "$W/ports"
# addressed as 0.0.0.0 (this machine, but not loopback): remote servers to the cli, where the dev identity never goes
STAGING=http://0.0.0.0:$PS; PROD=http://0.0.0.0:$PP; OTHER=http://0.0.0.0:$PO; MULTI=http://0.0.0.0:$PM
# the cli as a person runs it: nothing in the environment, a home of its own
c() { env -u CORI_SERVER -u CORI_TOKEN HOME="$W/home" "$B" "$@"; }
mkdir -p "$W/home" "$W/work"; cd "$W/work"

# --- a login to staging, an app, then a login to prod inside it, the folder deleted, init again
c login $STAGING --token staging-tok | has "the default outside app folders"
c init old --template team-requests --no-git | has "registered on $STAGING"
grep -qx "server = \"$STAGING\"" old/.cori/session && grep -qx 'token = ""' old/.cori/session   # the folder holds no token
(cd old && c login $PROD --token prod-tok) | tee "$W/login.out" | has "old/.cori/session now works with $PROD"
has "your other logins, for the app folders that work with them: $STAGING" "$W/login.out"
grep -qx "server = \"$PROD\"" old/.cori/session
rm -rf old
c init my-app --template team-requests --no-git | tee "$W/init.out" | has "registered on $PROD"
has "session written to my-app/.cori/session · $PROD, with your login to it" "$W/init.out"
(cd my-app && c apps list) | has -E "^prod +owner prod-tok"
# the file: prod (the default) at the top, which an older cli reads as its one login; staging kept under [[logins]]
test "$(awk '!/^#/ && NF { print; exit }' "$W/home/.cori/config.toml")" = "server = \"$PROD\""
grep -A1 -x '\[\[logins\]\]' "$W/home/.cori/config.toml" | has -x "server = \"$STAGING\""
test "$(python3 -c 'import os, sys; print(oct(os.stat(sys.argv[1]).st_mode & 0o777))' "$W/home/.cori/config.toml")" = 0o600

# --- each folder works with its own server whatever the default; a server named on the command line takes your login to it
c init stage-app --server $STAGING --front none --no-git | has "registered on $STAGING"
(cd stage-app && c apps list) | has -E "^staging +owner staging-tok"
(cd stage-app && c apps list --server $PROD) | has -E "^prod +owner prod-tok"
c apps list | has -E "^prod +owner prod-tok"   # outside a folder: the default, the last login

# --- a server you are not signed in to: the folder still works with it alone, and says how to sign in
c init lone --server $OTHER --front none --no-git 2>&1 | has "not signed in to $OTHER"
grep -qx "server = \"$OTHER\"" lone/.cori/session
(cd lone && c apps list 2>&1 || true) | has "run \`cori login $OTHER\`"
(cd lone && refute c apps list >/dev/null 2>&1)

# --- a folder's own token comes first: an agent's bootstrap (the env), and a session written by an older cli
env -u CORI_SERVER -u CORI_TOKEN HOME="$W/home" CORI_SERVER=$PROD CORI_TOKEN=agent-tok CORI_SESSION_KIND=agent "$B" init agent-app --front none --no-git | has "$PROD, with the agent session given"
grep -qx 'kind = "agent"' agent-app/.cori/session
(cd agent-app && c apps list) | has -E "^prod +owner agent-tok"
printf 'server = "%s"\ntoken = "legacy-tok"\n' "$PROD" > stage-app/.cori/session
(cd stage-app && c apps list) | has -E "^prod +owner legacy-tok"
(cd stage-app && c login $PROD --token prod-tok-2) | has "now works with $PROD"
(cd stage-app && c apps list) | has -E "^prod +owner prod-tok-2"
(cd my-app && c apps list) | has -E "^prod +owner prod-tok-2"   # one login per server: every prod folder uses the new one

# --- a file of an older cli (its one login at the top) reads as the default, and a login to another server keeps it
printf 'server = "%s"\ntoken = "staging-tok"\nkind = "human"\n' "$STAGING" > "$W/home/.cori/config.toml"
c apps list | has -E "^staging +owner staging-tok"
c login $PROD --token prod-tok | has "your other logins, for the app folders that work with them: $STAGING"
(cd stage-app && c apps list --server $STAGING) | has -E "^staging +owner staging-tok"

# --- a server hosting two organizations: a login per organization, folders that belong to one, never a mix-up
c login $MULTI --token acme-tok | tee "$W/multi.out" | has "logged in to $MULTI in organization acme"
has "you also belong to globex here" "$W/multi.out"
c init desk --server $MULTI --front none --no-git | has "registered on $MULTI"
grep -A2 -x '\[organization\]' desk/.cori/session | has -x 'id = "o-acme"'
c login $MULTI --token globex-tok | has "in organization globex"
test "$(grep -c "^server = \"$MULTI\"" "$W/home/.cori/config.toml")" = 2   # one login per organization
c login --list | has -E "^\* $MULTI +organization globex"
# the default login is globex's; the folder of acme still uses acme's, and names its organization on every call
(cd desk && c apps list) | has -E "^desk +owner acme-tok"
grep -q "^GET /v1/apps acme-tok o-acme$" "$W/organizations"
lacks "globex-tok o-acme" < "$W/organizations"
c apps list --server $MULTI | lacks desk   # outside the folder: globex, the last login, which has no app desk
# signing in to another organization inside the folder keeps the folder in its own
(cd desk && c login $MULTI --token globex-tok) | has "belongs to acme, and keeps it"
grep -q 'id = "o-acme"' desk/.cori/session
# asking for an organization the token is not for: refused, and nothing saved
(cd desk && refute c login $MULTI --token globex-tok --org acme >/dev/null 2>&1)
(cd desk && c login $MULTI --token globex-tok --org acme 2>&1 || true) | has "you signed in to globex, not acme"
# without a login to the folder's organization: no token at all, rather than another organization's, and how to sign in
c login --forget $MULTI --org acme | has "forgot your login to $MULTI (acme)"
(cd desk && c apps list 2>&1 || true) | has "run \`cori login $MULTI --org acme\`"
lacks "^GET /v1/apps globex-tok o-acme$" < "$W/organizations"
# a token of another organization given anyway (an older login naming none): the server refuses it, and the cli says how
printf 'server = "%s"\ntoken = "globex-tok"\nkind = "human"\n' "$MULTI" > "$W/home/.cori/config.toml"
(cd desk && c apps list 2>&1 || true) | tee "$W/mixup.out" | has "409"
has "this folder belongs to acme: run \`cori login $MULTI --org acme\`" "$W/mixup.out"

# --- no token ever reached a server it was not given for
lacks "^prod .* staging-tok$" < "$W/requests"
for t in prod-tok agent-tok legacy-tok; do lacks "^staging .* $t" < "$W/requests"; done
grep "^other " "$W/requests" | lacks "tok"
echo "e2e-logins: ok"

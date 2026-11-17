#!/usr/bin/env bash
# Each app's credentials under a Vault key of its own, against a real Vault (dev server, in Docker):
# - Transit (the default): Cori logs in with AppRole and a policy that lets it use its keys only; `cori serve --preflight`
#   says so;
# - an app's store databases get logins of their own, kept as the app's credentials, their data keys wrapped by the app's
#   key (`vault-transit:cori-app-<app>:1`); its store answers through them; a secret set for the app goes the same way; the
#   deployment's own secrets keep the master key;
# - the key rotated in Vault: at its next lead the server rewraps every data key under version 2;
# - the key withdrawn from Cori's policy: writing the app's credentials is refused, saying why;
# - without Transit (CORI_VAULT_ENGINE=kv), Cori logged in with an account of Vault's (userpass): the app's master key made
#   in the KV engine (secret/cori/apps/<app>), never written again by Cori, whose policy is create and read; at the next
#   lead every Transit row moves under it (`vault-kv:secret/cori/apps/<app>:1`) and the store answers with Transit gone; a
#   new version of the key is a rotation; the read withdrawn refuses the app's credentials;
# - the app deleted: its secrets go, its keys stay for the organization.
# Needs Docker, CORI_E2E_DATABASE_URL (a Postgres admin URL, the store server too), Temporal on 127.0.0.1:7233.
set -euo pipefail
has() { grep -c "$@" >/dev/null; }
cd "$(dirname "$0")/.."
cargo build -q -p cori-cli
B=./target/debug/cori; source scripts/lib.sh; E=$(e2e_app); DATA=$(mktemp -d)
VP=${CORI_E2E_VAULT_PORT:-8200}; P=${CORI_E2E_PORT:-8176}; S=http://127.0.0.1:$P; VA=http://127.0.0.1:$VP
for port in $VP $P; do if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-vault: port $port is taken" >&2; exit 1; fi; done
ADMIN=${CORI_E2E_DATABASE_URL:-${CORI_STORE_DATABASE_URL:-}}
[ -n "$ADMIN" ] || { echo "e2e-vault: set CORI_E2E_DATABASE_URL to a Postgres admin URL" >&2; exit 1; }
PSQL=${PSQL:-psql}
$PSQL "$ADMIN" -qc "DROP DATABASE IF EXISTS cori_e2e_vault WITH (FORCE)" -c "CREATE DATABASE cori_e2e_vault"
printf '%s\n' "SELECT format('DROP DATABASE %I WITH (FORCE)', datname) FROM pg_database WHERE datname LIKE 'cori\_store\_support\_desk%' \gexec" | $PSQL "$ADMIN" -q
DBURL="${ADMIN%/*}/cori_e2e_vault"
NAME=cori-e2e-vault-$$
docker run -d --rm --name "$NAME" --cap-add=IPC_LOCK -p 127.0.0.1:"$VP":8200 -e VAULT_DEV_ROOT_TOKEN_ID=root -e VAULT_DEV_LISTEN_ADDRESS=0.0.0.0:8200 "${CORI_E2E_VAULT_IMAGE:-hashicorp/vault:1.20}" >/dev/null
SRV=; WK=; SX=; trap 'kill ${SRV:-} ${WK:-} ${SX:-} 2>/dev/null || true; docker rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
for i in $(seq 1 60); do curl -sf "$VA/v1/sys/health" >/dev/null && break; sleep 0.5; done
vault() { docker exec -i -e VAULT_ADDR=http://127.0.0.1:8200 -e VAULT_TOKEN=root "$NAME" vault "$@"; }
vault secrets enable transit >/dev/null
vault auth enable approle >/dev/null
# Cori's policy: its keys only (cori-app-*, cori-preflight); creating one is Transit's upsert on encrypt; in the KV engine
# its apps' keys (create and read, never update) and the preflight's
policy() {
  local kv=${3:-'"deny"'}
  vault policy write cori - >/dev/null <<HCL
path "transit/encrypt/cori-app-*"  { capabilities = [$1] }
path "transit/decrypt/cori-app-*"  { capabilities = [$2] }
path "transit/encrypt/cori-preflight" { capabilities = ["create", "update"] }
path "transit/decrypt/cori-preflight" { capabilities = ["update"] }
path "secret/data/cori/apps/*"     { capabilities = [$kv] }
path "secret/data/cori/preflight"  { capabilities = ["create", "read"] }
HCL
}
policy '"create", "update"' '"update"'
vault write auth/approle/role/cori token_policies=cori token_ttl=20m token_max_ttl=1h >/dev/null
ROLE_ID=$(vault read -field=role_id auth/approle/role/cori/role-id)
SECRET_ID=$(vault write -f -field=secret_id auth/approle/role/cori/secret-id)
export CORI_VAULT_ADDR=$VA CORI_VAULT_AUTH=approle CORI_VAULT_ROLE_ID=$ROLE_ID CORI_VAULT_SECRET_ID=$SECRET_ID CORI_VAULT_CACHE_SECS=0
export CORI_MASTER_KEY=$(openssl rand -hex 32)

# the preflight: the login and a Transit round trip
$B serve --preflight --data-dir "$DATA" --database-url "$DBURL" --store-database-url "$ADMIN" > "$DATA/preflight.txt" 2>&1 || true
has "^ok   vault login" "$DATA/preflight.txt"; has "^ok   vault transit" "$DATA/preflight.txt"

start() { $B serve --listen 127.0.0.1:$P --data-dir "$DATA" --database-url "$DBURL" --store-database-url "$ADMIN" --auth dev --dev-mocks >>"$DATA/serve.log" 2>&1 & SRV=$!
  for i in $(seq 1 60); do curl -sf $S/healthz >/dev/null && break; sleep 0.5; done; kill -0 $SRV 2>/dev/null || { cat "$DATA/serve.log" >&2; exit 1; }; }
start
$B work --queue cori.shared >"$DATA/worker.log" 2>&1 & WK=$!
$B work --queue cori.pool.sandbox >"$DATA/sandbox.log" 2>&1 & SX=$!
T=dev:jean:it_admin
q() { $PSQL "$DBURL" -Atc "$1"; }

# the app, its store's sandbox: logins of its own whose passwords the app's key wraps
$B apps create support-desk --server $S --token $T >/dev/null
V1=$($B deploy "$E" --server $S --token $T --json | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
test "$(q "SELECT DISTINCT key_by FROM secrets WHERE scope = 'support-desk' AND kind = 'store_login'")" = "vault-transit:cori-app-support-desk:1"
vault read -format=json transit/keys/cori-app-support-desk | has '"exportable": false'
# its store answers, through its runtime login
curl -s -m 60 -X POST $S/v1/apps/support-desk/versions/$V1/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_id":"T-1042"}' > "$DATA/run1.json"
has "getCustomer via cori store" "$DATA/run1.json" || { cat "$DATA/run1.json" >&2; exit 1; }
# an app's secret and a deployment secret
CRM=s3cret $B secrets set crm.token --app support-desk --from-env CRM --server $S --token $T >/dev/null
GLOBAL=g $B secrets set global.token --from-env GLOBAL --server $S --token $T >/dev/null
test "$(q "SELECT key_by FROM secrets WHERE scope = 'support-desk' AND name = 'crm.token'")" = "vault-transit:cori-app-support-desk:1"
test "$(q "SELECT key_by FROM secrets WHERE scope = '' AND name = 'global.token'")" = "master"
$B secrets list --app support-desk --server $S --token $T | has "vault-transit:cori-app-support-desk:1"

# the organization rotates the key; at its next lead (here a restart), the server rewraps every data key under version 2
vault write -f transit/keys/cori-app-support-desk/rotate >/dev/null
kill $SRV; wait $SRV 2>/dev/null || true
start
for i in $(seq 1 60); do [ "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-transit:cori-app-support-desk:2'")" = 0 ] && break; sleep 1; done
test "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-transit:cori-app-support-desk:2'")" = 0
curl -s -m 60 -X POST $S/v1/apps/support-desk/versions/$V1/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_id":"T-1043"}' > "$DATA/run2.json"
has "getCustomer via cori store" "$DATA/run2.json" || { cat "$DATA/run2.json" >&2; exit 1; }

# the key withdrawn from Cori's policy: the app's credentials can no longer be written, and the refusal says why
policy '"deny"' '"deny"'
OUT=$(CRM=x $B secrets set crm.token --app support-desk --from-env CRM --server $S --token $T 2>&1 || true)
echo "$OUT" | has "403"
policy '"create", "update"' '"update"'

# without Transit: each app's master key in the KV engine, Cori logged in with its account (userpass)
policy '"create", "update"' '"update"' '"create", "read"'
vault auth enable userpass >/dev/null
printf 'kv-%s' "$(openssl rand -hex 12)" > "$DATA/vault-password"
vault write auth/userpass/users/cori password="$(cat "$DATA/vault-password")" token_policies=cori token_ttl=20m >/dev/null
export CORI_VAULT_ENGINE=kv CORI_VAULT_AUTH=userpass CORI_VAULT_USERNAME=cori CORI_VAULT_PASSWORD_FILE=$DATA/vault-password
unset CORI_VAULT_ROLE_ID CORI_VAULT_SECRET_ID
$B serve --preflight --data-dir "$DATA" --database-url "$DBURL" --store-database-url "$ADMIN" > "$DATA/preflight-kv.txt" 2>&1 || true
has "^ok   vault login .*the account \`cori\` (userpass auth)" "$DATA/preflight-kv.txt"; has "^ok   vault kv .*secret/cori/preflight" "$DATA/preflight-kv.txt"
kill $SRV; wait $SRV 2>/dev/null || true
start
# at its next lead the server moves every Transit row under the app's KV key, which it makes (version 1)
for i in $(seq 1 60); do [ "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-kv:secret/cori/apps/support-desk:1'")" = 0 ] && break; sleep 1; done
test "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-kv:secret/cori/apps/support-desk:1'")" = 0
test "$(q "SELECT key_by FROM secrets WHERE scope = '' AND name = 'global.token'")" = "master"
vault kv get -format=json secret/cori/apps/support-desk | python3 -c "import sys,json,base64; d=json.load(sys.stdin)['data']; assert d['metadata']['version'] == 1 and len(base64.b64decode(d['data']['key'])) == 32"
# Transit withdrawn: the store still answers, through logins its KV key wraps
policy '"deny"' '"deny"' '"create", "read"'
curl -s -m 60 -X POST $S/v1/apps/support-desk/versions/$V1/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_id":"T-1044"}' > "$DATA/run3.json"
has "getCustomer via cori store" "$DATA/run3.json" || { cat "$DATA/run3.json" >&2; exit 1; }
CRM=kv-secret $B secrets set crm.token --app support-desk --from-env CRM --server $S --token $T >/dev/null
test "$(q "SELECT key_by FROM secrets WHERE scope = 'support-desk' AND name = 'crm.token' AND revoked_at IS NULL")" = "vault-kv:secret/cori/apps/support-desk:1"
# Cori never writes a key that exists: its policy has no update (here, with its own token)
CT=$(vault write -field=token auth/userpass/login/cori password="$(cat "$DATA/vault-password")")
curl -s -X POST "$VA/v1/secret/data/cori/apps/support-desk" -H "X-Vault-Token: $CT" -d '{"data":{"key":"x"}}' | has "permission denied"
# the organization rotates: a new version of the key; at the next lead every row moves to version 2
vault kv put secret/cori/apps/support-desk key="$(openssl rand -base64 32)" >/dev/null
kill $SRV; wait $SRV 2>/dev/null || true
start
for i in $(seq 1 60); do [ "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-kv:secret/cori/apps/support-desk:2'")" = 0 ] && break; sleep 1; done
test "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk' AND revoked_at IS NULL AND key_by <> 'vault-kv:secret/cori/apps/support-desk:2'")" = 0
# version 1 destroyed once nothing uses it: the store still answers
vault kv destroy -versions=1 secret/cori/apps/support-desk >/dev/null
curl -s -m 60 -X POST $S/v1/apps/support-desk/versions/$V1/wf/support_ticket_triage/runs -H "Authorization: Bearer dev:sam:support_lead" -H 'content-type: application/json' -d '{"ticket_id":"T-1045"}' > "$DATA/run4.json"
has "getCustomer via cori store" "$DATA/run4.json" || { cat "$DATA/run4.json" >&2; exit 1; }
# the read withdrawn from Cori's policy: the app's credentials are refused, saying why
policy '"deny"' '"deny"'
OUT=$(CRM=y $B secrets set crm.token --app support-desk --from-env CRM --server $S --token $T 2>&1 || true)
echo "$OUT" | has "403"; echo "$OUT" | has "secret/data/cori/apps/support-desk"
policy '"deny"' '"deny"' '"create", "read"'

# the app deleted: its secrets go, its keys stay for the organization to delete
$B apps delete support-desk --confirm support-desk --server $S --token $T >/dev/null
test "$(q "SELECT count(*) FROM secrets WHERE scope = 'support-desk'")" = 0
vault read transit/keys/cori-app-support-desk >/dev/null
vault kv get secret/cori/apps/support-desk >/dev/null
echo "e2e-vault: ok"

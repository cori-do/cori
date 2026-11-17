#!/usr/bin/env bash
# The bundle store and the file store against a real S3 server that checks signatures (the Versity S3 Gateway, in Docker),
# each in a bucket of its own: the stores' own tests (signed requests, a wrong secret refused, the preflight's round trip),
# then the replica suite on them.
# Needs Docker (versity/versitygw), and what e2e-replicas.sh needs (CORI_E2E_DATABASE_URL, Temporal).
set -euo pipefail
cd "$(dirname "$0")/.."
PS=${CORI_E2E_PORT_S3:-8183}
if lsof -nP -iTCP:"$PS" -sTCP:LISTEN >/dev/null 2>&1; then echo "e2e-s3: port $PS is taken (CORI_E2E_PORT_S3)" >&2; exit 1; fi
NAME=cori-e2e-s3-$$
docker run -d --rm --name "$NAME" -p 127.0.0.1:"$PS":7070 --entrypoint sh "${CORI_E2E_S3_IMAGE:-versity/versitygw:v1.8.0}" \
  -c 'mkdir -p /gw/bundles /gw/files /meta && exec versitygw --access e2e --secret e2e-secret-key --port :7070 posix --sidecar /meta /gw' >/dev/null
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
for i in $(seq 1 40); do curl -s -o /dev/null http://127.0.0.1:"$PS"/ && break; sleep 0.5; done
CORI_E2E_S3_ENDPOINT=http://127.0.0.1:$PS CORI_E2E_S3_BUCKET=bundles CORI_E2E_S3_FILES_BUCKET=files CORI_E2E_S3_KEY=e2e CORI_E2E_S3_SECRET=e2e-secret-key \
  cargo test -q -p cori-server --lib -- bundles::tests::a_real_s3_server --nocapture
CORI_E2E_S3_CONTAINER=$NAME scripts/e2e-replicas.sh
echo "e2e-s3: ok"

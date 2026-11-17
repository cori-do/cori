#!/usr/bin/env bash
# The starter template's front (templates/team-requests). `cori init` writes the React scaffold, then the template's own
# files over it, among them its lockfile, its generated client (front/src/lib/cori.ts) and its build (dist/): a first
# `cori deploy` needs neither npm nor Deno. This script builds those three from the sources (the scaffold, the template,
# the vendored @cori-do/client, the workflows) and writes them back into the template, with the licences of the
# JavaScript in dist/ (dist/THIRD-PARTY-NOTICES.md). With --check it writes nothing and
# fails when they differ from what the template ships (CI, like the console's bundle).
#
#   scripts/template-front.sh            # after changing the template's front, its workflows, the scaffold or the client
#   scripts/template-front.sh --check
# The lockfile keeps the versions it has; to take newer ones, delete templates/team-requests/front/package-lock.json first.
# Needs node and npm, Deno (`cori gen client` compiles the workflows) and cargo, or a cori built from this tree (CORI_BIN).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
T=team-requests
B=${CORI_BIN:-$ROOT/target/debug/cori}
# the templates are embedded in the binary: build it from this tree (no-op when it is current)
[ -n "${CORI_BIN:-}" ] || cargo build -q -p cori-cli
CHECK=0; [ "${1:-}" = "--check" ] && CHECK=1
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# the folder name is the app id: the template's own, so nothing in it is renamed
env -u CORI_SERVER -u CORI_TOKEN HOME="$W" "$B" init "$W/$T" --template "$T" --no-register --no-git >/dev/null
cd "$W/$T/front"
if [ $CHECK = 1 ]; then npm ci --no-audit --no-fund >/dev/null; else npm install --no-audit --no-fund >/dev/null; fi
PATH="$(dirname "$B"):$PATH" npm run build >/dev/null
# the licences of the JavaScript in dist/ (minified away from it), which the cori binary embeds: scripts/cli-notices.sh
node "$ROOT/crates/cori-server/console/notices.mjs" . "$W/$T/dist/THIRD-PARTY-NOTICES.md" "JavaScript in the built front of the starter template (dist/)" >/dev/null

if [ $CHECK = 1 ]; then
  diff -r "$W/$T/dist" "$ROOT/templates/$T/dist"
  diff "$W/$T/front/src/lib/cori.ts" "$ROOT/templates/$T/front/src/lib/cori.ts"
  echo "template-front: templates/$T ships the build of its sources"
else
  rm -rf "$ROOT/templates/$T/dist"
  cp -R "$W/$T/dist" "$ROOT/templates/$T/dist"
  cp "$W/$T/front/src/lib/cori.ts" "$ROOT/templates/$T/front/src/lib/cori.ts"
  cp "$W/$T/front/package-lock.json" "$ROOT/templates/$T/front/package-lock.json"
  echo "template-front: wrote templates/$T/dist, front/src/lib/cori.ts and front/package-lock.json"
fi

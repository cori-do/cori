#!/usr/bin/env bash
# The cli's npm packages as people install them: packed for this machine's platform with a stand-in binary (or the one
# given), installed with install scripts off by npm (and pnpm and bun when they are here), then run through the
# launcher: arguments and exit status, a signal passed on, and a clear refusal without the platform's package.
#   packages/cli/test.sh [binary]
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;; Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
  Linux-aarch64) TARGET=aarch64-unknown-linux-musl ;; Linux-x86_64) TARGET=x86_64-unknown-linux-musl ;;
  *) echo "test.sh: no cli build for $(uname -sm)" >&2; exit 1 ;;
esac
BIN=${1:-}
if [ -z "$BIN" ]; then
  BIN="$W/fake-cori"
  printf '%s\n' '#!/bin/sh' 'case "$1" in exit) exit "$2" ;; wait) exec sleep 30 ;; *) echo "cori $*" ;; esac' > "$BIN"
fi
chmod +x "$BIN"
echo "# notices" > "$W/notices.md"
node "$HERE/pack.mjs" --out "$W/npm" --notices "$W/notices.md" "$TARGET=$BIN" >/dev/null
mkdir "$W/tgz"; for p in "$W"/npm/*; do (cd "$p" && npm pack --silent --pack-destination "$W/tgz" >/dev/null); done
ok() { printf '  ok  %s\n' "$*"; }

# npm, install scripts off: the launcher finds the platform's package, passes arguments and the exit status
mkdir "$W/npm-app"; (cd "$W/npm-app" && npm init -y >/dev/null && npm install --ignore-scripts --no-audit --no-fund --silent "$W"/tgz/*.tgz)
C="$W/npm-app/node_modules/.bin/cori"
if [ -z "${1:-}" ]; then
  test "$("$C" deploy . --message "a b")" = "cori deploy . --message a b"
  set +e; "$C" exit 7; s=$?; set -e; test $s = 7
  ok "npm (--ignore-scripts): arguments and exit status pass through"
  # a signal sent to the launcher reaches the binary, and the launcher ends by it
  "$C" wait & L=$!; sleep 1; kill -TERM $L
  set +e; { wait $L; } 2>/dev/null; s=$?; set -e; test $s = 143
  ! pgrep -f "sleep 30" -P "$L" >/dev/null 2>&1
  ok "a SIGTERM to the launcher ends the binary, and the launcher by the same signal"
else
  "$C" --version
  ok "npm (--ignore-scripts): $("$C" --version)"
fi

# without the platform's package (--omit=optional): a refusal that says what to do
mkdir "$W/bare"; (cd "$W/bare" && npm init -y >/dev/null && npm install --ignore-scripts --omit=optional --no-audit --no-fund --silent "$W"/tgz/cori-do-cli-[0-9]*.tgz)
set +e; out=$("$W/bare/node_modules/.bin/cori" --version 2>&1); s=$?; set -e
test $s = 1 && echo "$out" | grep -q "is not installed"
ok "without the platform package: exit 1, \"$(echo "$out" | head -1 | cut -c1-80)…\""

# pnpm and bun, which run no dependency's install script by default (the tarballs as `file:` dependencies: `bun add` takes
# no tarball path)
for pm in pnpm bun; do
  command -v $pm >/dev/null || continue
  mkdir "$W/$pm-app"; cp "$W"/tgz/*.tgz "$W/$pm-app/"
  deps=$(cd "$W/$pm-app" && for t in *.tgz; do n=${t%-[0-9]*.tgz}; printf '"@cori-do/%s":"file:./%s",' "${n#cori-do-}" "$t"; done)
  printf '{"name":"t","version":"1.0.0","dependencies":{%s}}\n' "${deps%,}" > "$W/$pm-app/package.json"
  (cd "$W/$pm-app" && $pm install >/dev/null 2>&1)
  if [ -z "${1:-}" ]; then test "$("$W/$pm-app/node_modules/.bin/cori" hello)" = "cori hello"; else "$W/$pm-app/node_modules/.bin/cori" --version >/dev/null; fi
  ok "$pm"
done
echo "packages/cli/test.sh: passed ($TARGET)"

# Helpers shared by the e2e scripts, sourced after they `cd` to the repository root and set `B` (the cori binary).

# `set -e` stops on a failed `grep -q` or `curl -sf` without a word: say which check it was
trap 'echo "e2e: stopped at line $LINENO: $BASH_COMMAND" >&2' ERR

# the `temporal` CLI's default address is localhost:7233, which can cost a 5 s connection deadline where localhost
# resolves to ::1 first; the dev server (and Cori's own default) listen on 127.0.0.1
export TEMPORAL_ADDRESS=${TEMPORAL_ADDRESS:-127.0.0.1:7233}

# Negative checks. `! command` asserts nothing under `set -e`: bash never stops on a negated command, so a check written
# that way passes whatever happens. `refute <command…>`: the command must fail. `<command> | lacks <pattern>`: its output
# must not match (and the command itself must succeed).
refute() { if "$@"; then echo "e2e: expected a failure: $*" >&2; exit 1; fi; }
lacks() { if grep -c -- "$1" >/dev/null; then echo "e2e: unexpected '$1' in the output" >&2; return 1; fi; }

# `sed -i` takes its backup suffix differently on BSD (macOS) and GNU (Linux) sed: edit through a temporary file.
sed_i() { local expr=$1 f; shift; for f in "$@"; do sed "$expr" "$f" > "$f.sed_i" && mv "$f.sed_i" "$f"; done; }

# The app the e2e scripts exercise: the support-desk template exactly as `cori init` ships it, in a fresh folder.
# CORI_E2E_APP names another folder.
e2e_app() {
  if [ -n "${CORI_E2E_APP:-}" ]; then echo "$CORI_E2E_APP"; return; fi
  local root; root=$(mktemp -d)
  # no server and no global login: init registers nothing and writes no session into the folder
  env -u CORI_SERVER -u CORI_TOKEN HOME="$root" "$B" init "$root/support-desk" --template support-desk --no-register --no-git >/dev/null
  echo "$root/support-desk"
}

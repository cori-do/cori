#!/usr/bin/env bash
# The cli's npm packages from binaries built on this machine, into target/npm (CI builds every platform and publishes:
# .github/workflows/release-cli.yml, on a tag v<version>). Needs cargo-about (scripts/cli-notices.sh).
#   scripts/release-cli.sh                                       # this machine's platform
#   TARGETS="aarch64-apple-darwin x86_64-apple-darwin" scripts/release-cli.sh
# The Linux targets are static (musl): build them on Linux with `cargo zigbuild` (BUILD=zigbuild), as CI does.
# Then packages/cli/test.sh target/<target>/release/cori, and to publish by hand: `npm publish target/npm/cli-<platform>`
# for each platform, then `npm publish target/npm/cli`.
set -euo pipefail
cd "$(dirname "$0")/.."
host=$(rustc -vV | sed -n 's/^host: //p'); host=${host/%-linux-gnu/-linux-musl}
TARGETS=${TARGETS:-$host}
BUILD=${BUILD:-build}
scripts/cli-notices.sh target/THIRD-PARTY-NOTICES.md
binaries=()
for t in $TARGETS; do
  cargo "$BUILD" --release --locked -p cori-cli --target "$t"
  binaries+=("$t=target/$t/release/cori")
done
node packages/cli/pack.mjs --out target/npm --notices target/THIRD-PARTY-NOTICES.md "${binaries[@]}"

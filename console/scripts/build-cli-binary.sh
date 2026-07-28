#!/usr/bin/env bash
# Populate console/src-tauri/binaries/ with the cori CLI sidecar for
# local dev by building it from the workspace. Release builds do the
# same per-target inside .github/workflows/desktop-release.yml.
#
# The sidecar is named `cori-cli` (not `cori`): the bare name would
# collide with the `Cori` app binary on case-insensitive filesystems.
# The launcher's "Install CLI" action exposes it as `cori` on PATH.
set -euo pipefail

cd "$(dirname "$0")/../.."

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  triple=aarch64-apple-darwin ;;
  Darwin-x86_64) triple=x86_64-apple-darwin ;;
  Linux-x86_64)  triple=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) triple=aarch64-unknown-linux-gnu ;;
  *) echo "✗ Unsupported host: $(uname -s)-$(uname -m)"; exit 1 ;;
esac

echo "→ cargo build --release -p cori-cli"
cargo build --release -p cori-cli

target="console/src-tauri/binaries/cori-cli-$triple"
cp target/release/cori "$target"
chmod +x "$target"
echo "✓ staged $target"

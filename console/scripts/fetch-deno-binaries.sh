#!/usr/bin/env bash
# Populate console/src-tauri/binaries/ with the Deno sidecar for local dev.
#
# Strategy: copy the system `deno` from PATH into the matching
# host-triple slot. This is for development only — release builds
# should download the official Deno binary from
# https://github.com/denoland/deno/releases per the matrix in
# .github/workflows/desktop-release.yml.
set -euo pipefail

cd "$(dirname "$0")/.."
BINS=src-tauri/binaries

if ! command -v deno >/dev/null 2>&1; then
  echo "✗ 'deno' not on PATH. Install via 'brew install deno' or"
  echo "  download from https://github.com/denoland/deno/releases"
  exit 1
fi

src="$(command -v deno)"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  triple=aarch64-apple-darwin ;;
  Darwin-x86_64) triple=x86_64-apple-darwin ;;
  Linux-x86_64)  triple=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) triple=aarch64-unknown-linux-gnu ;;
  *) echo "✗ Unsupported host: $(uname -s)-$(uname -m)"; exit 1 ;;
esac

target="$BINS/deno-$triple"
echo "→ copying $src to $target ($(du -h "$src" | cut -f1))"
cp "$src" "$target"
chmod +x "$target"
echo "✓ done"

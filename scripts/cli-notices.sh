#!/usr/bin/env bash
# THIRD-PARTY-NOTICES.md of the cori binary: the licences of its Rust crates (cargo-about, packages/cli/about.toml,
# which fails on a licence it does not accept), then of the JavaScript it embeds: the console's, written by its build
# (crates/cori-server/console/THIRD-PARTY-NOTICES.md), and the starter template's built front (scripts/template-front.sh). Every npm package of the cli carries it (packages/cli/pack.mjs).
#   scripts/cli-notices.sh <out file>        (needs cargo-about: `cargo install cargo-about --locked --features cli`)
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?usage: scripts/cli-notices.sh <out file>}
cargo about generate --locked --fail -m crates/cori-cli/Cargo.toml -c packages/cli/about.toml packages/cli/about.hbs > "$OUT"
cat crates/cori-server/console/THIRD-PARTY-NOTICES.md >> "$OUT"
printf '\n' >> "$OUT"; cat templates/team-requests/dist/THIRD-PARTY-NOTICES.md >> "$OUT"
echo "$OUT: $(grep -c '^- ' "$OUT") crates, $(grep -c '^### .* (' crates/cori-server/console/THIRD-PARTY-NOTICES.md) console packages, $(grep -c '^### .* (' templates/team-requests/dist/THIRD-PARTY-NOTICES.md) template packages"

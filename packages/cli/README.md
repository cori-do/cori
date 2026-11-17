# @cori-do/cli

The `cori` command line: create, check, run and deploy Cori apps from a terminal, a coding agent or CI. It is a prebuilt
binary for macOS and Linux (arm64 and x64): no Rust toolchain needed.

```bash
npm install -g @cori-do/cli      # or: pnpm add -g @cori-do/cli, bun add -g @cori-do/cli
cori login                       # opens your browser: sign in to Cori Cloud (https://app.cori.do), or create your account
cori init my-team --template team-requests
cd my-team && cori deploy        # a first release and its preview link
```

Another Cori server: `cori login https://<its address>`. Everything about Cori, and its source:
[github.com/cori-do/cori](https://github.com/cori-do/cori).

Without installing it (a sandbox, CI): `npx @cori-do/cli <command>`. In Claude Cowork's sandbox, npm is reachable by
default; the Cori server must be too (Organization settings › Capabilities › Code execution › Additional allowed domains).

## What it installs

Two packages: this one, a small launcher (`bin/cori.js`), and the binary for your platform, `@cori-do/cli-<os>-<cpu>`
(`darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`), which npm picks among this package's optional dependencies
by its `os` and `cpu`. Nothing comes from anywhere but npm and no install script runs, so it installs where scripts are
off (`--ignore-scripts`, pnpm, bun) and wherever npm is allowed through.

- The Linux binaries are static (musl): any distribution, Alpine included.
- Windows: install it in WSL.
- Installed with `--omit=optional` (or `--no-optional`), the binary is missing, and the launcher says so.
- `CORI_CLI_BINARY` runs another binary instead (one you built: `cargo install --path crates/cori-cli`).

## Versions

A Cori server names the oldest command line it serves. An older one prints a warning with the upgrade command, and
`cori doctor` shows both versions.

## Licence

Apache-2.0 (LICENSE, with its NOTICE). The binary includes open source software, listed with its licences in
THIRD-PARTY-NOTICES.md.

## Releasing

A tag `v<version>` (the workspace's version in Cargo.toml, which this package.json repeats) runs the `release-cli`
workflow: the four binaries, the packages (`pack.mjs`, with `scripts/cli-notices.sh`), then the platforms' packages
published, and this one last. Started by hand, the workflow is a dry run. `test.sh` checks the packages as npm, pnpm and
bun install them.

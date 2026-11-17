# Contributing

Issues and pull requests are welcome. For a change of any size, open an issue first, so that we can agree on the
approach before you write it. By contributing, you agree that your contribution is under this repository's licences
([LICENSING.md](LICENSING.md)).

## Building

- The Rust toolchain that `rust-toolchain.toml` names, and `protoc` (`brew install protobuf`, or `protobuf-compiler` on
  Debian and Ubuntu).
- Deno 2, the Temporal CLI and Postgres, for `cori up`, `cori check` and the end-to-end tests.
- Node.js 22, for the console (`crates/cori-server/console`) and the starter template's front.

```bash
cargo build -p cori-cli                      # target/debug/cori
cargo test --workspace                       # the store's tests also want CORI_STORE_DATABASE_URL, a Postgres admin URL
```

## Tests

`.github/workflows/ci.yml` runs everything a pull request needs; each `scripts/e2e-*.sh` says at its top what it covers
and what it needs (most want a Temporal dev server on 127.0.0.1:7233 and a Postgres admin URL in
`CORI_E2E_DATABASE_URL`). `scripts/e2e.sh` is the main end-to-end run; `scripts/e2e-logins.sh` needs nothing running.

Run them with your shell's `CORI_SERVER` and `CORI_TOKEN` unset (`env -u CORI_SERVER -u CORI_TOKEN …`), so that no
command reaches a real server with your login.

## Files that are built and committed

- **The console** is embedded in the binary from `crates/cori-server/console/dist/` and `dist-portal/`. After changing
  anything under its `src/`, or `packages/client`, run `npm ci && npm run build` there and commit the outputs and
  `THIRD-PARTY-NOTICES.md` with your change.
- **The starter template** (`templates/team-requests`) ships its front built, so that a first `cori deploy` needs
  neither npm nor Deno. After changing its front, its workflows or `packages/client`, run `scripts/template-front.sh`.

CI rebuilds both and fails when they differ from what is committed.

## The plugin for AI agents

This repository is also Cori's plugin and its marketplace, read by every assistant:

| File | Read by |
|---|---|
| `plugin.json`, `mcp.json` (`streamable-http`), `skills/` | the [Agent Plugins 1.0](https://agent-plugins.org/specification) clients: Codex, ChatGPT, Cursor, Copilot, Kiro, VS Code; `npx plugins add` |
| `.claude-plugin/plugin.json`, `.mcp.json` (`http`), `skills/` | Claude Code and Claude Cowork |
| `.claude-plugin/marketplace.json` | Claude's marketplaces (the plugin is this repository's root, `./`); also a catalog for other hosts |
| `skills/` alone | `npx skills add cori-do/cori` |

Both manifests carry the same name, description and version, and `mcp.json` and `.mcp.json` the same server: keep them
in step by hand (each schema is closed, so no file can be shared). Each skill's `name` is its folder's, and its
frontmatter is strict YAML (quote a description that holds `: `).

Rules:

- **No hooks**, here or in a `hooks/` folder: Codex discovers a root `hooks/hooks.json`, and plugins with hooks are kept
  out of OpenAI's public directory.
- **The MCP server stays OAuth-only**, without API keys or headers: each client does its own sign-in.
- **Skills use the MCP tools when they are connected**, and the command line otherwise (as `npx -y @cori-do/cli@latest`
  when `cori` is not installed): chat apps cannot run commands, and their connector is already signed in.

Check a change with:

```bash
claude plugin validate --strict .            # the Claude manifests and the marketplace
scripts/validate-agent-plugin.sh             # plugin.json and mcp.json against the Agent Plugins 1.0.0 schemas
```

## Releasing

A release is a tag `v<version>`, the version in `Cargo.toml`'s `[workspace.package]`, which `packages/cli/package.json`,
`plugin.json` and `.claude-plugin/plugin.json` repeat: bump all four together (CI fails when they differ). The tag runs
`.github/workflows/release-cli.yml`, which builds the binaries and publishes `@cori-do/cli` on npm. The plugin's version
pins what its installs get: a change to the skills reaches people with the next release.

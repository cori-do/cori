# Cori

Author and run typed TypeScript workflows from your terminal.

> **Status:** v0.1.0-dev

## Repo layout

```
crates/        Rust workspace
  cori-cli/        the `cori` binary
  cori-worker/     Temporal worker library
  cori-compiler/   manifest + TS validation, DAG generation
  cori-broker/     capability broker (cli, mcp, llm)
  cori-ledger/     cost ledger, trace recording
  cori-manifest/   YAML schema + parser, shared types
  cori-protocol/   wire types between CLI, worker, deno subprocess
packages/      TypeScript workspace
  sdk/             @cori/sdk — what user step files import
```

## Build

```bash
cargo build --workspace
pnpm install
pnpm build
```

```bash
cargo run -p cori-cli -- --version
# cori 0.1.0-dev
```

## Running workflows

Cori executes workflows on a local [Temporal](https://temporal.io/) server.
Start one in a separate terminal before invoking `cori run`:

```bash
# install once: brew install temporal (macOS) or see https://docs.temporal.io/cli
temporal server start-dev --port 7233
```

Then, in another terminal:

```bash
cargo run -p cori-cli -- run examples/hello_world
```

Override the Temporal target via `CORI_TEMPORAL_TARGET` (defaults to
`http://localhost:7233`).

## License

MIT License

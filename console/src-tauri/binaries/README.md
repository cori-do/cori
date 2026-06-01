# Tauri sidecar binaries

This directory holds the bundled `temporal` CLI for each supported target triple. The current files are **placeholder stubs** — they let the Tauri build script verify the paths exist, but they cannot actually start a Temporal dev server.

## Before shipping

Procure the real Temporal CLI binary for each target triple and replace the stubs:

```
binaries/
  temporal-aarch64-apple-darwin
  temporal-x86_64-apple-darwin
  temporal-x86_64-pc-windows-msvc.exe
  temporal-x86_64-unknown-linux-gnu
```

Download from <https://github.com/temporalio/cli/releases> — pick a recent stable release, verify the SHA against the GitHub release page, and `chmod +x` the Unix binaries. License: Apache 2.0; add an entry to `NOTICE` in this repo when shipping.

## Why platform-suffixed names

Tauri's bundler resolves `bundle.externalBin: ["binaries/temporal"]` per host triple at build time by appending the current target triple. The `tauri.conf.json` `bundle.externalBin` field stays as `binaries/temporal` — the per-triple files in this directory are what it resolves to.

## For development

`cargo tauri dev` reads the host triple's file. On Apple Silicon that's `temporal-aarch64-apple-darwin`. With the placeholder stub the supervisor will start, fail readiness probe, log a Degraded status, and back off — exactly the supervisor failure path. To exercise the real path locally, either:

- Replace the stub with a real Temporal binary, **or**
- Run `temporal server start-dev` in another terminal so `temporal_endpoint::resolve()` finds it on `127.0.0.1:7233` and the supervisor short-circuits without ever spawning the sidecar.

# Tauri sidecar binaries

This directory holds three bundled binaries for each supported target triple — the **Temporal CLI** (used as the dev server sidecar), **Deno** (used by the broker to execute `code` step bodies), and the **cori CLI** (exposed on the user's PATH via the launcher's "Install CLI" action). All are declared in `tauri.conf.json` under `bundle.externalBin`.

The current files (when present) are **placeholder stubs** that let the Tauri build script verify the paths exist; they cannot actually run a Temporal dev server or execute a Deno script. Real binaries are populated by the fetch scripts (local dev) or by CI (release builds).

## Layout

```
binaries/
  temporal-aarch64-apple-darwin
  temporal-x86_64-apple-darwin
  temporal-x86_64-pc-windows-msvc.exe
  temporal-x86_64-unknown-linux-gnu
  temporal-aarch64-unknown-linux-gnu
  deno-aarch64-apple-darwin
  deno-x86_64-apple-darwin
  deno-x86_64-pc-windows-msvc.exe
  deno-x86_64-unknown-linux-gnu
  deno-aarch64-unknown-linux-gnu
  cori-cli-aarch64-apple-darwin
  cori-cli-x86_64-apple-darwin
  cori-cli-x86_64-pc-windows-msvc.exe
  cori-cli-x86_64-unknown-linux-gnu
  cori-cli-aarch64-unknown-linux-gnu
```

The CLI sidecar is named `cori-cli`, not `cori` — the bare name would collide with the `Cori` app binary on case-insensitive filesystems (macOS, Windows). The launcher's "Install CLI" action (`src/cli_install.rs`) exposes it as `cori` on the user's PATH.

Tauri's bundler resolves `bundle.externalBin: ["binaries/temporal", "binaries/deno"]` per host triple at build time by appending the current target triple to each name. The per-triple files in this directory are what those entries resolve to.

## Local dev

Three helper scripts populate the matching host-triple slot (the fetch scripts copy your system binaries; the build script compiles the CLI from the workspace):

```bash
./scripts/fetch-temporal-binaries.sh
./scripts/fetch-deno-binaries.sh
./scripts/build-cli-binary.sh
```

The scripts only populate the slot for your current host; cross-target slots stay as stubs (or empty). That's fine — `cargo tauri dev` only reads the host's slot.

## Procurement (release builds)

Replace the stubs with the real binaries for each target triple. Upstream releases:

- **Temporal CLI** — <https://github.com/temporalio/cli/releases>. License: Apache 2.0; add an entry to `NOTICE` when shipping.
- **Deno** — <https://github.com/denoland/deno/releases>. License: MIT; add an entry to `NOTICE` when shipping.

Verify the SHA against each release page, `chmod +x` the Unix binaries.

## Why platform-suffixed names

Tauri's bundler picks the right host-triple file at build time. The `tauri.conf.json` `bundle.externalBin` entries stay as the bare names (`binaries/temporal`, `binaries/deno`) — the per-triple files in this directory are what they resolve to. At install time, Tauri strips the triple suffix so the production app sees `<exe_dir>/temporal` and `<exe_dir>/deno`.

## How the Console finds each binary at runtime

- **Temporal** is spawned by `supervisor.rs` via `app.shell().sidecar("temporal")` — the Tauri shell plugin handles path resolution.
- **Deno** is invoked by the broker (in the `cori-broker` crate, which is Tauri-agnostic) via `std::process::Command`. The console's `worker.rs` computes the bundled deno's filesystem path (see `src/sidecars.rs`) and sets `CORI_DENO` before the broker's `Runtime::resolve` runs — the broker then picks it up over `$PATH` lookup.
- **cori CLI** is never spawned by the app; `src/cli_install.rs` resolves its bundled path (via `src/sidecars.rs`) and links/copies it onto the user's PATH as `cori` when the user clicks "Install CLI" in the launcher.

## Without the binaries

`cargo tauri dev` will start, the Temporal supervisor will fail readiness probe and back off, and any workflow with a `code` step will fail dispatch with `RuntimeUnavailable`. To exercise either path locally without a full procurement:

- Run `temporal server start-dev` in another terminal so the supervisor short-circuits without spawning the sidecar, **and / or**
- Run `./scripts/fetch-deno-binaries.sh` (or `export CORI_DENO=$(which deno)`).

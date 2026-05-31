# @cori-do/console

Cori Console — local web UI served by `cori work`. React Router v7 in
SPA mode (`ssr: false`); the build output is embedded into the
`cori-console` Rust binary via `rust-embed`.

## Build

```bash
pnpm --filter @cori-do/console build      # → build/client/
cargo build -p cori-console               # embeds build/client/
```

CI must run the pnpm build **before** `cargo build -p cori-console`.
`cori-console/build.rs` checks for `build/client/index.html` and emits
a warning + writes a placeholder if it's missing, so a stale `cargo
build` still produces a working binary (with a "console not built"
splash) instead of failing.

## Dev mode

```bash
# Terminal A — start the worker + Console API. Note the printed
# tokenised URL.
cargo run -p cori-cli -- work

# Terminal B — start Vite. Paste the token from terminal A into the
# URL: http://localhost:5173/?t=<token>
pnpm --filter @cori-do/console dev
```

Vite proxies `/api/*` to `127.0.0.1:7878` so the SPA shares the
worker's session cookie.

## Layout

```
app/
  root.tsx                — HTML shell + session bootstrap
  routes.ts               — route table
  routes/
    dashboard.tsx         — `/`     — status strip + recents
    runs.tsx              — `/runs` — run history (optional ?workflow_id=)
    run-detail.tsx        — `/runs/:key/:utc` — one trace
  lib/
    api.ts                — typed fetch helpers + response shapes
    session.ts            — `?t=` → cookie exchange, master-token memory
    format.ts             — duration / relative-time / cost formatters
  styles/
    base.css              — Phase 2 minimal styles (Phase 5 imports brand)
```

Phase 2 ships read-only screens. Phase 3 adds the run-trigger form +
SSE live stream + consent modal.

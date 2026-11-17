# Front

Vite + React + TypeScript + Tailwind, built into `../dist/` and served by Cori at the root of the app's own host
(`/apps/<app>/` under `cori up`), and every release at the root of a host of its own, `<app>--v<n>…`
(`/apps/<app>/@v<n>/` under `cori up`).

```bash
npm install
npm run dev      # http://localhost:5173 against `cori up` (the app must be promoted locally for /__cori/config.json)
npm run build    # regenerates src/lib/cori.ts from the workflows, type-checks, writes ../dist; run before `cori deploy`
```

- `src/lib/cori.ts` is generated (`npm run gen`): one typed object per workflow with an endpoint. Never edit it.
- `src/lib/api.ts` unwraps runs (`must`), keeps the dev identity valid for this app, exposes the hosted config.
- `src/components/ui.tsx` is the small UI kit; `src/components/RunResult.tsx` renders any run (headline, fields, steps, error).
- Add one file per screen under `src/views/` and register it in `VIEWS` in `src/views/index.tsx`. Routing is on the hash
  (`#/tickets`): the server falls back to `index.html` for unknown paths, so path routes would break asset loading.
- `vendor/cori-client/` is `@cori-do/client` (not on npm), written by `cori init`.

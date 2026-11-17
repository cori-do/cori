# Cori console

The web console `cori serve` / `cori up` serves at `/`, and its second page, the organization's **apps portal**: React 19 +
TypeScript + Tailwind 4, built with Vite.

## How it ships

`npm run build` type-checks and bundles each page (scripts and styles inlined) into **one file**: `dist/index.html`, the
console, and `dist-portal/portal.html`, the apps portal. `cori-server` embeds both at compile time (`include_str!` in
`src/api/front.rs`), so the `cori` binary carries them and the server needs no asset route. Each page has one inline
script, which its content security policy allows by hash; a brand file (`CORI_BRAND_FILE`) rewrites its `<title>Cori
console</title>` (`Cori apps`), its favicon link, `window.__CORI_BRAND__` and the accent shades `--color-brand-50/100/500/600/700`.

`dist/` and `dist-portal/` are committed: `cargo build`, the Dockerfile and `scripts/release-cli.sh` need no Node
toolchain. **After changing anything under `src/`, run `npm run build` and commit the outputs and
`THIRD-PARTY-NOTICES.md` with it**, or the binary keeps serving the old pages (CI rebuilds and compares).

## Develop

```sh
npm install
cori up                    # a server on 127.0.0.1:8080 (CONSOLE_API=http://… points the proxy elsewhere)
npm run dev                # http://localhost:5174 with hot reload; /v1 and /apps are proxied to the server
```

The proxy reads `CONSOLE_API`, never `CORI_SERVER` (shells often export it for the cli, and it may name production).
The portal page is at http://localhost:5174/portal.html.

**Without a server**: `npm run mock` (http://localhost:5175) answers every call in the browser from a demo organization
(`src/mock`, fictional people and apps): *View as…* in the account menu, or `?as=dev:sam:manager` in the address, shows it
as someone else; `?fresh` starts from an organization with no app; `?theme=dark`. `npm run build:demo` writes the same as
one standalone file, `dist-demo/index.html` (not committed). Production builds carry none of it.

## The console

Three places, and the organization's settings. Nothing is labelled by persona: a tab, a section or a button shows when the
person's roles reach it (`GET /v1/me`'s `can`, `GET /v1/me/apps`); the server checks every call anyway.

- **Home**: what waits for the person, their apps (live, a release ready, what waits there), and an organization's first
  steps (`GET /v1/start`) until they are done.
- **Inbox**: every decision waiting for the person (workflow approvals, go-lives, data changes, app deletions, and for
  admins the apps shared outside the organization), with the decision open beside the list.
- **An app**: Overview (the next step, key numbers, workflows, recent runs), Releases, Runs (and a run, followed while it
  goes: each step says what it does in words, from its author's description and what its release compiled of it, never
  its code, and opens on what it received and gave back, the person who decided, the way a condition went; its builders
  also get where it is written, its attempts, the model calls and a query's text: `GET /v1/apps/{app}/runs/{run}/steps`), Data (the store as a spreadsheet: a sheet per table, written through the role's tools; schema and SQL for its
  builders), Access (who uses it, who builds it, sharing), Settings (going-live approval, credentials, AI budget,
  sign-in, deletion). Someone who only uses the app sees Overview, My runs and Data.
- **Settings**: General, Members, AI models, Connectors, Credentials, System, Audit log.
- **Connect your agent** (`#/connect`, `#/connect/new` to create an app), ⌘K to jump anywhere, the account menu (theme,
  the apps portal, your account, sign out; *View as…* under dev auth).

## Routes

Hash routes. Slack and email notifications, cori-auth, the cli and the API's `console_url` link to them; the older ones
(`#/run/<app>/<run>`, `#/approvals/<app>`, `#/versions/<app>`, `#/runs/<app>`, `#/data/<app>/…`, `#/people`,
`#/organization`, `#/operate`, `#/llm`, `#/audit`, `#/build`, `#/start`) redirect to the page that has their content now
(`src/lib/router.ts`):

- `#/`, `#/inbox[/<app>[/<item>]]`, `#/connect[/new]`, `#/settings/<section>`
- `#/app/<app>[/releases[/<release>]|/runs[/<run>]|/data[/<live|short hash>/<store>/<table>]|/access|/settings]`
- `#/switch/<slug>`: sign in again to another organization (an accepted invitation's link)
- `#/preview/<app>/v<n>?…`, `#/portal?…`: a release's host or the apps portal signing its person in through the console
  (`src/pages/Handoff.tsx`); nothing of the console shows around it

## Layout

- `src/lib/`: the API client and sign-in (`api.ts`), the request cache (`query.ts`), routes (`router.ts`), the session,
  decisions (everything waiting for the person), people, formatting, theme.
- `src/ui/`: the components (buttons, fields, badges, cards, menus, dialogs and drawers, toasts, the value viewer, people
  pickers, access requests, credentials, identity sync).
- `src/shell/`: the frame (sidebar, organization switcher, account menu, ⌘K palette, *View as…*; under another brand,
  "Powered by Cori" at the foot of the sidebar, or of the page on a phone, which no brand setting removes).
- `src/pages/`: Home, Inbox, Connect, Handoff; `app/` (an app's tabs), `data/` (the spreadsheet), `settings/`.
- `src/portal/`: the apps portal page (`portal.html`).
- `src/mock/`: the demo organization (`npm run mock`, `npm run build:demo`).

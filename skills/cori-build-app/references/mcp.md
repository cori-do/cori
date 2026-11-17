# The Cori MCP server (optional)

Remote only: streamable HTTP at `<server>/mcp`, OAuth handled by the host against the org's identity provider.

```
claude mcp add --transport http cori https://cori.acme/mcp        # or http://127.0.0.1:8080/mcp under `cori up`
```

It answers what the app folder cannot know. **It never reads or writes the app's files**: no read, write, check or run
tool exists (`scaffold` returns a step file as text, for hosts without a shell). Editing is your own file tools;
checking, running and deploying is the cli.

## Tools

| Tool | Input | Returns |
|---|---|---|
| `list_apps` | — | apps you own or build: id, owner, promoted version, how to pull. Start here for an existing app |
| `get_app` | `{ app }` | owner, builders, roles, promoted and last version, urls, the `cori pull <app> <dir>` command |
| `create_app` | `{ id, name?, roles?, builders?, template?: "blank" \| "team-requests" \| "support-desk" }` | reserves the id, you become owner (`blank`, the default, is an empty app). `roles` strongest first (`["manager", "employee"]`): `cori init` writes them into cori.toml in that order. Then `login` and `cori init <id> --app <id>` |
| `login` | `{ app, ttl_secs? }` (default 3600, max 14400) | `{ server, app, token, kind: "agent", expires_at, scopes, session_file: ".cori/session", not_allowed, session_toml, bootstrap, next }` |
| `list_connectors` | `{ kind?: mcp \| store \| http \| activity \| cli }` | names, kinds, tools with one-line descriptions and effects |
| `find_tool` | `{ query }` | up to 20 ranked tools with input schemas |
| `request_connector` | `{ url, reason? }` | files a request in the admin inbox when nothing declared fits |
| `scaffold` | `{ connector, tool, name?, app?, version? }` | the typed step file as text, for hosts without a shell (with a shell prefer `cori scaffold`, which writes it and updates `steps:`). `app` (and a `version`, default the latest) reads that app's own store connectors from the version's files |
| `app_settings` | `{ app }` | the promoted (else latest) version's `cori.toml` settings, shown as a form (MCP App). They change in the folder: edit `cori.toml`, then `cori deploy` |
| `store_browse` / `store_query` | `{ app, store?, table?, version?, … }` / `{ app, store, sql, version? }` | rows the caller's role may read (MCP App): the live databases once a version is promoted, else the latest version's sandbox; `version` names a sandbox; an agent session always reads a sandbox. `store_query` is the widget's own (the model does not see it) |
| `preview` | `{ app, hash? }` | the release's own url (its host, `<app>--v<n>…`, nested where the host allows), a screenshot when available, the version's workflows and review state (MCP App) |

The MCP Apps widgets render in claude.ai, Claude Desktop and Cowork; Claude Code sees the JSON result.

## The `login` flow

1. `create_app { id }` for a new app, or `get_app { app }` for an existing one (you must be its owner or a builder).
2. `login { app }`. The result carries a short-lived **agent token** minted from the human the host authenticated:
   `sub` = the human, `kind = agent`, one app, scopes `registry:read`, `versions:publish`, `runs:preview`,
   `stores:read`. It deploys and pulls versions of that app, starts preview runs and reads their sandbox stores; it
   never promotes, approves or rolls back, never reaches live data, live runs or another app, and holds none of its
   human's platform powers (admin, auditor…): those answer 403 naming the human path.
3. Install it, one of:
   - the folder exists: write `session_toml` **verbatim** to `<app>/.cori/session` with your file tool (preferred:
     the token never goes on a command line);
   - the folder does not exist: run `bootstrap` as printed
     (`CORI_SERVER=… CORI_TOKEN=… CORI_SESSION_KIND=agent CORI_SESSION_EXPIRES_AT=… cori pull <app> ./<app>`, or
     `cori init <app> --app <app>` when no version exists yet); `init` and `pull` write the session file from the environment.
4. Every later `cori` command in that folder is authenticated. When the cli prints `session expired` (or a 401), call
   `login` again and rewrite the file. The token is visible in the transcript by design; its scope and lifetime bound
   the exposure. Never echo it, never commit it (`.cori/` is ignored and skipped by deploy).

A harness that keeps credentials out of the model's context sets `_meta["cori.dev/credential-sink"]` (any value, e.g.
`"session-file"`) on the call: the result the model sees carries `delivery: "sink"` and no token, and the credential
(`token`, `session_toml`, `bootstrap`) rides in the result's `_meta["cori.dev/credential"]` for the harness to install
and strip.

## Hosts without a shell

Discovery (`list_connectors`, `find_tool`) and `scaffold` (returned as text) still work; write the step file and
edit `steps:` in `workflow.yaml` yourself. `check`, `run` and `deploy` need the cli: hand the folder to a human or a
CI job with `cori` installed (`npm install -g @cori-do/cli`, or `npx @cori-do/cli …`: a binary for macOS and Linux).

## At authoring time, the connectors themselves

The agent may call the org's MCP servers directly from its own host with its own credentials to explore shapes
(`cori connectors export --to .mcp.json` writes their addresses into the host config). What runs at run time is the
worker with the run user's exchanged token or the connector's service credential, against the same registry entry, so
what you wrote against is what the worker calls. `cori run --record --no-mock <connector>` turns one real call into
the connector's fixture.

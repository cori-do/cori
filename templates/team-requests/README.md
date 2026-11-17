# Team Requests

A small finished app, built to run on any Cori server as it is: one store, a model step, a rule, an approval and a
React front, with no connector outside the platform. Team members file requests; a model sorts them; small amounts
are approved by a rule; a manager decides the rest. `AGENTS.md` tells an agent the layout and the commands.

| Piece | Kind | Shows |
|---|---|---|
| `cori.toml` | app manifest | the app's roles (`member`, `manager`), its store, its front; no LLM provider of its own |
| `workflows/submit_request` | async endpoint | an `llm` step with a fixed schema, the decision in a `code` step, a `store` write, a `human` approval routed to a role in a `branch` |
| `workflows/list_requests` | sync endpoint | a guarded `store` query, whose rows depend on the caller's role |
| `stores/requests` | store | migrations, `seed/` demo rows for sandboxes and local runs, rules (rows belong to who filed them), one role file per app role (`writes: workflows`) |
| `front/` | React front | New request (follows its run), Requests, Approvals (managers only); built into `dist/` |

## The model

On a server, the `llm` step uses the organization's model for the small tier or, while the organization has none, the
server's starter model (free, shared, limited to a few calls a day). Do not send confidential data to the starter
model: add a provider of your own first (console: Settings › AI models). `cori run` and `cori up` answer from
`fixtures/llm.json`.

## Roles

Who holds `member` and `manager` is granted to people by the organization, not by the app: from the app's page in the
console (its Access tab, "Share"), or in the organization's identity provider. Either role may file requests, and a person with both acts in the store as one of them. A
manager does not decide their own request unless the organization lets people approve their own runs (four eyes);
with that setting, one person holding both roles can try the whole flow alone.

## Commands

```bash
cori deploy .                                  # a release with its own sandbox store and a preview URL; dist/ is already built
cori check .                                   # every workflow and the store (needs Deno)
cori run workflows/submit_request title="Second monitor" amount_eur=180 --decide approve_request='{"approved":true}'
cd front && npm install && npm run build       # after changing the front or a workflow: regenerates src/lib/cori.ts, type-checks, writes dist/
cori promote team-requests <release>           # a human: the release goes live, its migrations run on the live store
```

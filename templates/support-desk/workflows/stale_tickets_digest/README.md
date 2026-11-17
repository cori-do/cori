# Stale tickets digest

## Goal
No end-user: the schedule runs as the `support-digest` service account (roles:
`support_lead`), so the store and the company MCP servers apply that role's rules to a
token exchanged for the account, and the audit names the account.

## Dataflow
```
stale_tickets(store.query) ─▶ by_department(code) ─▶ summarise(map ▷ llm) ─▶ post_digest(http) ─▶ append_sheet(cli, sandbox)
```

## Verification
- `stale_tickets.query` is proven read-only at the AST; only `readable` columns of `support_lead`
- `append_sheet` runs in a sandbox (`gws` cli connector); the change review shows `MayWrite`

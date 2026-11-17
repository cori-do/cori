# List requests

## Goal
The front's Requests screen: the latest 100 requests, optionally of one status. Who sees what is the store's rule,
not this workflow's: a member's role reads their own rows (`tenant: requested_by`), a manager's every member's
(`scope: all`).

## Dataflow
```
latest(store.query)
```

## Verification
- `latest.query` is one SELECT over columns both roles may read, run by Postgres as the caller's store role
- `idempotency: none`: every call lists again (a replay would show an old list)
- locally: `cori run workflows/list_requests status=approved --user alice:member`

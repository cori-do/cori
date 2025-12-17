Example deployments, policies, and demo schemas live here.

## Cerbos (local dev)

Run a local Cerbos PDP wired to the generated demo policies:

```bash
docker compose -f docker-compose.cerbos.yml up
```

Then in `examples/demo-crm-project/cori.yaml` set:

- `policy_engine: cerbos`
- `cerbos_grpc_hostport: "localhost:3593"`

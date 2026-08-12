# `sap_purchase_order_read`

Reference workflow for a direct, read-only SAP S/4HANA OData integration. It
uses the `cori-sap` CLI capability and does not require an MCP server.

## Adapter contract

The workflow uses Cori's linked, CLI-shaped `cori-sap` capability with this
surface:

```text
cori-sap purchase-orders get  --id <id>
cori-sap purchase-orders list --limit <1..100>
```

`get` returns one normalized header:

```json
{
  "purchase_order": {
    "purchase_order": "4500000001",
    "supplier": "10000001",
    "company_code": "1010",
    "document_currency": "EUR"
  }
}
```

`list` returns normalized headers plus bounded-result metadata:

```json
{
  "purchase_orders": [{ "purchase_order": "4500000001" }],
  "count": 1,
  "limit": 25,
  "has_more": false
}
```

The adapter owns OData field mapping, pagination, timeouts, and SAP error
normalization. `$CORI_HOME/sap.toml` stores only the connection metadata for one
tenant-bound default profile. `cori login cori-sap` stores the corresponding
owner- and tenant-bound bearer token in the secure OS keychain. At run time the
broker resolves that credential and passes it only to the linked `cori-sap`
library call; the workflow source, child processes, and the long-lived worker
environment never receive it. A secure keychain is required by default.

This v1 workflow is owner-local: SAP steps route to the requesting user's queue,
not to a shared service pool or a specific machine. Run only one worker for that
user queue, or configure every worker polling it with the same `cori-sap`
adapter build, `sap.toml`, and owner keychain credential. Shared SAP pools and
machine-affine routing are not supported yet.

The in-process parser accepts only the three typed read operations. It rejects
explicit profile selection, auth, configuration, credential export, arbitrary
HTTP/OData, and every write operation. The machine-configured `default_profile`
is always used. No separate adapter install is required; `cori check` reports
the linked capability as built into the worker.

Cori gives each SAP activity one Temporal attempt and ignores authored retry
overrides. Transient network, throttling, timeout, and SAP 5xx failures are
non-retryable in v1; after fixing the cause, start a fresh run. A 401 or 403 is
the separate human-driven `NeedsReauth` flow, not an automatic transient retry.

## SAP setup

1. In S/4HANA, enable the communication scenario associated with the Purchase
   Order OData V4 API (`API_PURCHASEORDER_2`).
2. Create a communication system and arrangement with the minimum read role.
3. Configure one `default_profile` and its tenant's HTTPS `base_url` in
   `$CORI_HOME/sap.toml`, using the origin from the arrangement's inbound
   service URL; do not put connection metadata in this workflow.
4. On the Cori machine, run `cori login cori-sap` and enter the token for that
   tenant. Cori requires a secure OS keychain and binds the stored credential to
   the current owner and configured tenant.
5. Ensure `SAP_ACCESS_TOKEN` is not exported in the Cori CLI or Console
   environment; Cori fails closed and directs operators to the login command.

## Check and run

```bash
cori check examples/sap_purchase_order_read

cori run examples/sap_purchase_order_read \
  operation=list max_results=25

cori run examples/sap_purchase_order_read \
  operation=get 'purchase_order_id="4500000001"'
```

Numeric-looking SAP IDs must remain strings; the quoting in the `get` example
preserves the ID type when Cori parses `key=value` arguments.

Run the workflow's contract tests with:

```bash
cd examples/sap_purchase_order_read
deno task test
```

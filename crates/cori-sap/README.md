# cori-sap

`cori-sap` is a narrow, read-only adapter for the SAP S/4HANA Purchase Order
OData V4 API. It intentionally exposes only three workflow-safe operations:

```text
cori-sap purchase-orders get --id 4500001234
cori-sap purchase-orders list --limit 50
cori-sap purchase-orders items --id 4500001234 --limit 100
```

For workflow execution, the adapter is linked into the Cori worker and needs no
separate binary or PATH installation. The CLI-shaped command is still declared
in `tools_required` so compiler, planner, and broker capability boundaries stay
explicit.

The standalone binary is optional diagnostics. From a source checkout it can
be installed with:

```text
cargo install --path crates/cori-sap
```

All successful read commands write normalized JSON to stdout. The adapter has
no raw URL, method, header, filter, query, or request-body option.

## Machine configuration

Configuration is read from `$CORI_HOME/sap.toml`, or `~/.cori/sap.toml` when
`CORI_HOME` is unset. The file contains no credentials:

```toml
default_profile = "production"

[profiles.production]
base_url = "https://my-s4-tenant.example.com"
# Optional for systems that require an explicit SAP client.
sap_client = "100"
```

`base_url` is an HTTPS origin only. The adapter always appends the fixed SAP
Purchase Order V4 service root:

```text
/sap/opu/odata4/sap/api_purchaseorder_2/srvd_a2x/sap/purchaseorder/0001/
```

Take the origin from the inbound service URL shown by the SAP communication
arrangement rather than guessing a tenant hostname.

An interactive standalone caller may select another machine-owned profile with
`--profile`. In a Cori workflow, explicit profile selection is rejected and the
machine-configured `default_profile` is always used.

## Authentication

Use Cori's login command for workflow credentials:

```text
cori login cori-sap
```

The command shows the canonical default-profile target (HTTPS origin plus
optional `sap-client`) before confirmation, prompts with hidden input, and
stores the token in the OS keychain under an account bound to both the Cori
user and that exact target. A production build fails closed when no secure
keychain is available. SAP readiness checks retrieve and validate the actual
keychain value; a stale non-secret credential index never reports ready.
`cori-sap` intentionally has no auth subcommand.

At workflow dispatch, the broker loads the default profile, derives its
canonical origin/`sap-client` credential account, and returns that same
validated `MachineProfile` with the owner's token. The linked adapter consumes
the pair in-process; the token is never placed in argv, child environment, or a
PATH executable. Changing the default SAP origin or `sap-client` selects a
different credential account and therefore requires a fresh
`cori login cori-sap`.

If SAP later returns 401 or 403, Cori classifies the step as `NeedsReauth` and
waits for a replacement token. Running `cori login cori-sap` replaces the
credential and explicitly resumes the human-blocked read.

Cori performs no automatic SAP retries in v1. Every SAP activity has exactly
one Temporal attempt, even if workflow metadata requests more. Transport
failures, timeouts, throttling, and SAP 5xx responses become non-retryable
stable-code failures; start a fresh run after resolving the condition. The
standalone binary's `retryable` JSON field and exit code `3` are diagnostics
only and do not change workflow behavior.

For standalone diagnostics outside Cori, `SAP_ACCESS_TOKEN` may be supplied to
that single `cori-sap` process. It must not be exported into a Cori worker's
ambient environment. Cori CLI and Console startup fail closed when the variable
is present; unset it and use `cori login cori-sap` instead.

Standalone errors are emitted as bounded JSON envelopes on stderr. Linked
workflow errors are reduced to stable broker error codes. Neither form includes
the bearer token, request URL, redirect target, or SAP response body.

SAP references: [Purchase Order (OData V4)](https://help.sap.com/docs/SAP_S4HANA_CLOUD/64609d0ecac54654b0837cba34555b82/c89eec80ec2043d980cb7b8c89e0a00a.html)
and [Communication Arrangements](https://help.sap.com/docs/SAP_S4HANA_CLOUD/0f69f8fb28ac4bf48d2b57b9637e81fa/fab3fd449cf74c6384622b98831e989e.html).

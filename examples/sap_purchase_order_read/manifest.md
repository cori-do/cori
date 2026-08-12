---
id: sap_purchase_order_read
name: Read SAP Purchase Orders
description: Read one purchase order or a bounded list of purchase-order headers from SAP S/4HANA without modifying SAP data.
created: 2026-08-11
version: 1
parameters:
  - name: operation
    type: enum
    values: [get, list]
    description: Read one purchase order by ID or list a bounded set
  - name: purchase_order_id
    type: string
    required: false
    description: Purchase order ID; required when operation is get
  - name: max_results
    type: number
    default: 25
    min: 1
    max: 100
    description: Maximum purchase orders returned by a list operation
tools_required: [cori-sap]
mcp_servers: []
tags: [sap, s4hana, procurement, read_only]
result:
  headline: "{{ purchase_order_count }} SAP purchase-order records returned"
  description: The requested purchase-order headers were read without modifying SAP.
  fields:
    - label: Operation
      path: operation
    - label: Records
      path: purchase_order_count
      format: number
  sections:
    - label: Purchase orders
      path: purchase_orders
      display: table
---

# Read SAP Purchase Orders

## Goal

Read either one purchase order or a bounded set of purchase-order headers from
an SAP S/4HANA system. The workflow is deliberately read-only and exposes no
arbitrary URL, OData query, header, or write operation to workflow authors.

## Preconditions

- The worker runs a Cori build that includes the linked `cori-sap` adapter;
  shared service pools are not supported in v1
- The machine has exactly one tenant-bound `cori-sap` `default_profile`; a
  workflow cannot select a different profile
- `cori login cori-sap` has stored the current owner's token in the secure OS
  keychain; a secure keychain is required by default
- The SAP communication arrangement enables the Purchase Order OData V4 API and
  grants read access
- The worker can reach the configured SAP API endpoint

## Steps

1. **read_purchase_orders** (cli) — Ask the typed `cori-sap` adapter to get one
   purchase order or list a bounded set

## Verification

- The run result reports the expected purchase-order count
- Every returned record has a non-empty `purchase_order`
- `get` returns at most one record and `list` returns no more than `max_results`
- The Cori trace contains a single `cori_cli` activity and no mutating SAP
  operation

## Notes

- `operation=get` requires `purchase_order_id`; the step rejects the request
  before invoking the adapter when it is absent.
- `operation=list` ignores `purchase_order_id` and applies only the bounded
  `max_results` argument.
- The endpoint belongs to the machine-local `cori-sap` default profile. The
  broker resolves the matching owner- and tenant-bound bearer token from the OS
  keychain and calls the linked adapter in-process; it never sends that token to
  workflow code or a child process.
- Do not export `SAP_ACCESS_TOKEN` into the Cori CLI or Console environment;
  startup fails closed. Use `cori login cori-sap` so the owner- and target-bound
  keychain entry is selected instead.
- The adapter fails closed to its typed read contract: profile selection,
  authentication, configuration, arbitrary HTTP/OData, credential export, and
  SAP write commands are not available to workflow input.
- Every SAP activity has one Temporal attempt. Retry metadata is ignored and
  transient failures are non-retryable in v1; fix the cause and start a fresh
  run. The human-driven `NeedsReauth` resume is the only exception.
- Cori user queues do not provide machine affinity. Run one worker for this
  owner queue, or configure every worker polling it with the same Cori build,
  profile, and owner keychain credential.
- A single typed step handles both read modes because builtin branching is
  deferred in Cori v1; two numbered CLI steps would incorrectly perform both
  reads on every run.

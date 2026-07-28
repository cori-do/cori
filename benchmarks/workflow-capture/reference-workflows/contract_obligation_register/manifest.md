---
id: contract_obligation_register
name: Contract Obligation Register
description: Extract the dated obligations from a signed contract, resolve notice periods stated by cross-reference, compute the latest date each party can still act, and draft the legal operations summary.
created: 2026-07-27
version: 1
parameters:
  - name: contract_document_id
    type: string
    description: Customer contract document ID
  - name: register_spreadsheet_id
    type: string
    description: Obligation register spreadsheet ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, legal, hybrid]
---

# Contract Obligation Register

Contracts state obligations in prose and often define a notice period once, then refer to it from
later clauses, so extraction and reference resolution are a typed `llm` read of each new contract.
The date arithmetic and every Workspace write are deterministic. It creates a draft only and never
sends email.

---
id: support_inbox_triage
name: Support Inbox Triage
description: Illustrative support triage structure; dynamic per-message execution requires v1 builtin support.
created: 2026-07-13
version: 1
parameters:
  - name: queue_spreadsheet_id
    type: string
    description: Queue spreadsheet ID
  - name: gmail_query
    type: string
    description: Synthetic inbox query
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, support, hybrid, structural-example]
---

# Support Inbox Triage

> Structural example only — not an executable benchmark oracle in Cori v1.
> Dynamic fetch and mutation over every returned message requires
> `map`/`for_each`, which v1 deliberately defers.

It creates a draft only and never sends email.

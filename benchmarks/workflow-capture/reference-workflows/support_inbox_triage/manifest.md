---
id: support_inbox_triage
name: Support Inbox Triage
description: Classify synthetic support inbox messages, batch-write a priority queue, and create an internal digest draft.
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
tags: [benchmark, support, hybrid]
---

# Support Inbox Triage

Uses a typed runtime classification and fixed batch GWS calls. It creates a draft only and never sends email.

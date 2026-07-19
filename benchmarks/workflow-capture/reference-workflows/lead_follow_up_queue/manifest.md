---
id: lead_follow_up_queue
name: Lead Follow-up Queue
description: Rank active sales leads, batch-write the queue, and create one customer follow-up draft.
created: 2026-07-13
version: 1
parameters:
  - name: lead_spreadsheet_id
    type: string
    description: Lead spreadsheet ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, sales, deterministic]
---

# Lead Follow-up Queue

Ranks only active leads using deterministic business rules.

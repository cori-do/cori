---
id: preapproved_pto_processing
name: Pre-approved PTO Processing
description: Apply approved PTO to balances and Calendar using weekdays, holidays, and exclusive all-day event boundaries.
created: 2026-07-13
version: 1
parameters:
  - name: pto_spreadsheet_id
    type: string
    description: PTO spreadsheet ID
  - name: calendar_id
    type: string
    description: Calendar ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, hr, deterministic]
---

# Pre-approved PTO Processing

Uses deterministic working-day arithmetic and no Calendar notifications.

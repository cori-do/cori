---
id: preapproved_pto_processing
name: Pre-approved PTO Processing
description: Illustrative PTO structure; dynamic event and draft fan-out requires v1 builtin support.
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
tags: [benchmark, hr, deterministic, structural-example]
---

# Pre-approved PTO Processing

> Structural example only — not an executable benchmark oracle in Cori v1.
> A variable number of requests requires dynamic `map`/`for_each` fan-out for
> Calendar events and drafts, which v1 deliberately defers.

The intended calls use deterministic working-day arithmetic and no Calendar notifications.

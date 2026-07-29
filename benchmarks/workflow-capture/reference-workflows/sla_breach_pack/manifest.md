---
id: sla_breach_pack
name: SLA Breach Pack
description: Calculate SLA breaches from a case sheet, fill a report, and create a support-lead draft.
created: 2026-07-13
version: 1
parameters:
  - name: case_spreadsheet_id
    type: string
    description: Case spreadsheet ID
  - name: report_template_id
    type: string
    description: Report template document ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, support, deterministic]
---

# SLA Breach Pack

Applies fixed SLA thresholds and uses batch Workspace writes.

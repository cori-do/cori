---
id: weekly_operating_review
name: Weekly Operating Review
description: Assign deterministic project RAG statuses, batch-write KPIs, fill a review document, and create a leadership draft.
created: 2026-07-13
version: 1
parameters:
  - name: project_spreadsheet_id
    type: string
    description: Project spreadsheet ID
  - name: report_template_id
    type: string
    description: Review report template ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: week_ending
    type: string
    description: Week-ending date
tools_required: [gws]
mcp_servers: []
tags: [benchmark, management, deterministic]
---

# Weekly Operating Review

Uses fixed RAG thresholds and batch Workspace updates.

---
id: incident_postmortem_pack
name: Incident Postmortem Pack
description: Reconstruct the confirmed contributing factors from an incident channel transcript, compute the response durations from the metrics sheet, and draft the review summary.
created: 2026-07-27
version: 1
parameters:
  - name: transcript_document_id
    type: string
    description: Incident channel transcript document ID
  - name: metrics_spreadsheet_id
    type: string
    description: Incident metrics spreadsheet ID
  - name: findings_spreadsheet_id
    type: string
    description: Postmortem findings spreadsheet ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, engineering, hybrid]
---

# Incident Postmortem Pack

The transcript is unordered channel history in which several hypotheses were raised and some were
ruled out, so separating confirmed causes from discarded ones is a typed `llm` read. The duration
arithmetic and every Workspace write are deterministic. It creates a draft only and never sends
email.

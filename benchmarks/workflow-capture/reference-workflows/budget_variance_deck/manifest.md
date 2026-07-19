---
id: budget_variance_deck
name: Budget Variance Deck
description: Calculate budget variance, build a three-slide finance deck, and create a finance draft.
created: 2026-07-13
version: 1
parameters:
  - name: budget_spreadsheet_id
    type: string
    description: Budget spreadsheet ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: period
    type: string
    description: Reporting period
tools_required: [gws]
mcp_servers: []
tags: [benchmark, finance, deterministic]
---

# Budget Variance Deck

Uses explicit sign handling and creates exactly three content slides.

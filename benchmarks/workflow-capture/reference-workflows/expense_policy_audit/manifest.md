---
id: expense_policy_audit
name: Expense Policy Audit
description: Audit synthetic expenses against deterministic policy thresholds, report exceptions, and draft a finance summary.
created: 2026-07-13
version: 1
parameters:
  - name: expense_spreadsheet_id
    type: string
    description: Expense spreadsheet ID
  - name: report_template_id
    type: string
    description: Exceptions report template ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, finance, deterministic]
---

# Expense Policy Audit

Applies all policy predicates and preserves every reason for a failing row.

---
id: vendor_invoice_intake
name: Vendor Invoice Intake
description: Illustrative invoice intake structure; dynamic per-document execution requires v1 builtin support.
created: 2026-07-27
version: 1
parameters:
  - name: register_spreadsheet_id
    type: string
    description: Invoice register spreadsheet ID
  - name: invoice_folder_query
    type: string
    description: Drive query selecting this week's invoices
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, finance, hybrid, structural-example]
---

# Vendor Invoice Intake

> Structural example only — not an executable benchmark oracle in Cori v1.
> Reading every document returned by a variable Drive query requires dynamic
> `map`/`for_each` fan-out, which v1 deliberately defers.

Every supplier formats its invoices differently, so field extraction is a typed `llm` call over
each document as it arrives. The arithmetic check, the overdue comparison, and every Workspace
write are deterministic. Invoices that do not balance are recorded as written and routed for human
resolution rather than adjusted. It creates a draft only and never sends email.

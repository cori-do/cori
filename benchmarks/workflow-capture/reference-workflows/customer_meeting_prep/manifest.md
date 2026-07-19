---
id: customer_meeting_prep
name: Customer Meeting Prep
description: Build a factual customer-meeting preparation document, link it to Calendar, and draft an internal brief.
created: 2026-07-13
version: 1
parameters:
  - name: calendar_id
    type: string
    description: Calendar ID
  - name: account_brief_id
    type: string
    description: Account brief document ID
  - name: source_message_id
    type: string
    description: Customer Gmail message ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, sales, hybrid]
---

# Customer Meeting Prep

Uses a typed source-fact extraction and suppresses Calendar notifications.

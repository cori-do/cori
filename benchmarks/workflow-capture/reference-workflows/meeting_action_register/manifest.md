---
id: meeting_action_register
name: Meeting Action Register
description: Extract and deduplicate meeting actions, batch-write a tracker, and create a follow-up draft.
created: 2026-07-13
version: 1
parameters:
  - name: meeting_notes_document_id
    type: string
    description: Meeting notes document ID
  - name: action_tracker_spreadsheet_id
    type: string
    description: Action tracker spreadsheet ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
  - name: as_of
    type: string
    description: Deterministic evaluation timestamp
tools_required: [gws]
mcp_servers: []
tags: [benchmark, management, hybrid]
---

# Meeting Action Register

Uses typed extraction, deterministic deduplication, and a draft only.

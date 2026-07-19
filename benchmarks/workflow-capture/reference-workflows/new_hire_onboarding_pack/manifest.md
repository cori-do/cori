---
id: new_hire_onboarding_pack
name: New-hire Onboarding Pack
description: Fill an onboarding template, schedule a quiet orientation, update the hire sheet, and draft a welcome note.
created: 2026-07-13
version: 1
parameters:
  - name: new_hire_spreadsheet_id
    type: string
    description: New hire spreadsheet ID
  - name: template_document_id
    type: string
    description: Onboarding template document ID
  - name: calendar_id
    type: string
    description: Orientation calendar ID
  - name: run_tag
    type: string
    description: Benchmark resource tag
tools_required: [gws]
mcp_servers: []
tags: [benchmark, hr, deterministic]
---

# New-hire Onboarding Pack

Creates one orientation event with `sendUpdates=none` and one welcome draft.

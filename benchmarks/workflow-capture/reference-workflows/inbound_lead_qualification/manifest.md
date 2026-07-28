---
id: inbound_lead_qualification
name: Inbound Lead Qualification
description: Read overnight inbound enquiries, extract the qualifying facts each prospect states in prose, score them against the standing policy, and draft a reply to the strongest lead.
created: 2026-07-27
version: 1
parameters:
  - name: lead_spreadsheet_id
    type: string
    description: Lead register spreadsheet ID
  - name: gmail_query
    type: string
    description: Inbound enquiry query
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

# Inbound Lead Qualification

Prospects state seat counts, timelines, and their buying process in their own words, so the
extraction step is a typed `llm` call over whatever arrived that morning. Scoring, ranking, and
every Workspace write are ordinary deterministic steps. It creates a draft only and never sends
email.

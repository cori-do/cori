---
id: gws_sheet_range_snapshot
name: Google Sheets Range Snapshot
description: Read a selected Google Sheets range into a typed snapshot without modifying it.
created: 2026-08-11
version: 2
updated: 2026-08-12
parameters:
  - name: spreadsheet_id
    type: string
    description: Google Sheets spreadsheet ID to read
  - name: range
    type: string
    description: A1 or R1C1 range to read, including the tab name when needed
  - name: first_row_is_header
    type: boolean
    default: true
    description: Use the first returned row as friendly column names
tools_required: [gws]
mcp_servers: []
tags: [starter, google_workspace, sheets, data, read_only]
---

# Google Sheets Range Snapshot

## Goal
Read one caller-selected range from Google Sheets and return its values with normalized range metadata. The workflow performs a single read request and never changes the spreadsheet.

## Preconditions
- The `gws` CLI is installed and authenticated
- The authenticated account can read the target spreadsheet
- The spreadsheet ID and range identify an existing or valid empty range

## Steps
1. **read_sheet_range** (cli) — Call the Sheets values-get API and normalize the returned value grid
2. **format_sheet_results** (code) — Turn the grid into named rows under a user-facing `results` payload

## Verification
- Results show the normalized range, row count, and column count
- Cell values are displayed as named rows when the first row contains headers
- The source spreadsheet is unchanged

## Notes
- Empty ranges return an empty `values` array.
- Formatted values are requested using the API default behavior.

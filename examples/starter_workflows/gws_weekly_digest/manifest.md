---
id: gws_weekly_digest
name: Google Workspace Weekly Digest
description: Summarize upcoming meetings and the unread email count for the next seven days.
created: 2026-08-11
version: 2
updated: 2026-08-12
parameters: []
tools_required: [gws]
mcp_servers: []
tags: [starter, google_workspace, calendar, gmail, read_only]
---

# Google Workspace Weekly Digest

## Goal
Produce a compact seven-day view of upcoming meetings together with the current unread Gmail count. The workflow is suitable for a quick weekly orientation and does not modify Workspace data.

## Preconditions
- The `gws` CLI is installed and authenticated
- The authenticated account can read its primary calendar and Gmail mailbox

## Steps
1. **build_weekly_digest** (cli) — Use the Google Workspace weekly-digest helper to combine Calendar and Gmail data

## Verification
- The `results` payload includes a concise summary and period timestamps
- Meeting count matches the returned meeting array length
- The unread email estimate is a non-negative integer

## Notes
- This workflow is read-only.
- Calendar boundaries use the Google account timezone.

---
id: gws_meeting_prep
name: Next Meeting Prep
description: Prepare a concise snapshot of the next upcoming Google Calendar meeting.
created: 2026-08-11
version: 2
updated: 2026-08-12
parameters:
  - name: calendar_id
    type: string
    default: primary
    description: Google Calendar ID to inspect
tools_required: [gws]
mcp_servers: []
tags: [starter, google_workspace, calendar, meetings, read_only]
---

# Next Meeting Prep

## Goal
Return the next upcoming meeting with its timing, description, location, links, and attendee responses. If the calendar has no upcoming meeting, return a clear message instead.

## Preconditions
- The `gws` CLI is installed and authenticated
- The authenticated account can read the selected calendar

## Steps
1. **prepare_next_meeting** (cli) — Use the Google Workspace meeting-prep helper to fetch the next event

## Verification
- The `results` payload describes one upcoming meeting or reports that none was found
- Meeting results include start and end values plus an attendee count
- No calendar events are created or modified

## Notes
- This workflow is read-only.
- The helper uses the Google account timezone when deciding which meeting is next.

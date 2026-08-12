---
id: disk_space_snapshot
name: Disk Space Snapshot
description: Report available and used space for the filesystem containing a selected path.
created: 2026-08-11
version: 2
updated: 2026-08-12
parameters:
  - name: path
    type: path
    default: /
    description: File or folder whose containing filesystem should be inspected
tools_required: [df]
mcp_servers: []
tags: [starter, local, storage, read_only]
---

# Disk Space Snapshot

## Goal
Show a compact, machine-readable snapshot of the filesystem containing the selected path. The workflow reports capacity in 1024-byte blocks and never changes files or disk settings.

## Preconditions
- A POSIX-compatible `df` executable is installed and available on the worker
- The selected path exists and is readable by the current user

## Steps
1. **inspect_disk_space** (cli) — Run `df -Pk` for the selected path and normalize its output

## Verification
- At least one filesystem entry is returned
- Results include a concise summary plus total, used, and available bytes
- The mount point and reported capacity percentage are present

## Notes
- This starter targets macOS and Linux, where `df -Pk` provides POSIX output.
- The command is read-only and does not inspect file contents.

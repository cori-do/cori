---
id: largest_files_report
name: Largest Files Report
description: Find the largest regular files below a folder without changing them.
created: 2026-08-11
version: 2
updated: 2026-08-12
parameters:
  - name: path
    type: path
    description: Folder to scan recursively
  - name: limit
    type: number
    default: 20
    min: 1
    max: 1000
    description: Maximum number of files to return
  - name: minimum_size_mb
    type: number
    default: 10
    min: 0
    description: Ignore files smaller than this many mebibytes
tools_required: [python3]
mcp_servers: []
tags: [starter, local, files, storage, read_only]
---

# Largest Files Report

## Goal
Identify the largest regular files beneath a selected folder while keeping memory use bounded by the requested result limit. The workflow reads file metadata only and never opens, moves, or deletes file contents.

## Preconditions
- `python3` is installed and available on the worker
- The selected path is a readable directory

## Steps
1. **scan_largest_files** (cli) — Walk the folder and maintain a bounded heap of the largest matching files

## Verification
- The `results` payload starts with a concise scan summary
- Returned files are ordered from largest to smallest
- No more than `limit` files are returned
- Every returned file is at least `minimum_size_mb` MiB

## Notes
- Unreadable entries are skipped and counted in `skipped_entries`.
- Symbolic links are not followed.
- Absolute file paths and sizes are stored in the run trace.

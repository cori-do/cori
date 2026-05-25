---
id: code_only
name: Code-only demo (square then stringify)
description: Two-step workflow used as the code execution acceptance fixture — pure code, no external tools.
created: 2026-05-25
version: 1
parameters:
  - name: x
    type: number
    default: 10
    description: Number to square
tools_required: []
mcp_servers: []
tags: [demo, code]
---

# Code-only demo

## Goal
Prove that the Cori worker can dispatch `code` activities to the Deno
runner subprocess and that step outputs chain into the next step's input.

## Steps
1. **square** (code) — square `x`
2. **stringify** (code) — render the result as a decimal string

## Verification
- `cori run code_only x=12` prints `Output: {"result":"144"}`
- The trace recorded in SQLite has two `code` activities, both `ok`

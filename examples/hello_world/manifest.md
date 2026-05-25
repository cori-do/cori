---
id: hello_world
name: Hello World
description: A 3-step demo workflow proving Cori works end-to-end with zero credentials.
created: 2026-05-25
version: 1
parameters: []
tools_required: [curl]
mcp_servers: []
tags: [demo]
---

# Hello World

## Goal
Prove a fresh Cori install can fetch data from the public internet, transform
it in a sandboxed `code` step, and format the result — all in under 5 seconds
with no API keys or accounts.

## Steps
1. **fetch_quote** (cli) — `curl` a random quote from `zenquotes.io`
2. **count_words** (code) — count the words in the quote text
3. **format** (code) — build the final friendly one-liner

## Verification
- `cori demo` completes in under 5 seconds
- The trace shows three activities, all `ok`, total cost €0.00
- The printed line includes the quote, the author, and the word count

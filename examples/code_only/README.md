# code_only

Two-step demo workflow used as the Phase 3 acceptance fixture.

```sh
cori init --local
cori workflows register examples/code_only
cori run code_only x=12
# Output: {"result":"144"}
```

Both steps are `code` activities: they execute in a sandboxed Deno
subprocess (`packages/deno-runner`) with `--allow-read` permission and no
network access. Step 1 squares its input; step 2 receives step 1's output
as its input and renders it as a string.

Requires Deno to be installed (or `CORI_DENO` pointing at a Deno binary)
until Phase 3.2 auto-downloads it.

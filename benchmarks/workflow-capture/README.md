# Cori workflow-capture benchmark

This package measures direct Google Workspace task completion against a captured, unchanged Cori workflow. It operates against the GWS account already configured on the machine; it does not create, manage, or require a Workspace tenant.

The ten tasks have deterministic scenario generation, external Workspace-state grading, safety gates, structured harness adapters, persisted transcripts/traces, and maintainer-authored reference workflows. The benchmark never treats agent text or a process exit as correctness evidence.

## Commands

```bash
export CORI_BENCH_CALENDAR_ID='your-dedicated-secondary-calendar-id'
pnpm --dir benchmarks/workflow-capture benchmark validate
pnpm --dir benchmarks/workflow-capture benchmark preflight
pnpm --dir benchmarks/workflow-capture benchmark plan --profile full --batch 1/5
CORI_BENCH_LLM_MODEL=gpt-4o-mini pnpm --dir benchmarks/workflow-capture benchmark run --profile smoke --harness codex --seed 42
CORI_BENCH_LLM_MODEL=gpt-5.4 pnpm --dir benchmarks/workflow-capture benchmark run --profile full --batch 1/5 --harness codex --seed 42
CORI_BENCH_LLM_MODEL=gpt-5.4 pnpm --dir benchmarks/workflow-capture benchmark run --profile full --task lead_follow_up_queue --harness codex --seed 42
pnpm --dir benchmarks/workflow-capture benchmark report --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark view --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark cleanup --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark combine --run-ids <batch-1>,<batch-2>,...
```

`run` and `preflight` build `cori-cli` from the current repository checkout and pin the benchmark plus its authoring harness to the absolute `target/debug/cori` path. The selected executable's directory is also placed first on the child-process `PATH`, so an authoring agent that types `cori` still reaches that exact build. This prevents a globally installed `cori` on `PATH` from silently testing stale code. Every result records that path, whether it came from `workspace_dev` or an explicit override, and the executable SHA-256; `combine` requires the same digest across batches. Set `CORI_BENCH_CORI` only when deliberately testing an alternate executable.

`preflight` is explicit because it creates and immediately trashes a namespaced Sheets canary. It additionally requires `gws 0.22.5`, `temporal`, `deno`, valid GWS credentials/scopes, `CORI_BENCH_CALENDAR_ID`, and `CORI_BENCH_LLM_MODEL` for the three hybrid tasks. It verifies that the selected Cori executable reports the model provider as an available LLM capability and that the configured calendar is a writable secondary calendar. Use `GWS_BIN` only to point to an alternate GWS executable; do not put credentials in benchmark artifacts.

Create one dedicated secondary calendar outside the benchmark, then export its ID as `CORI_BENCH_CALENDAR_ID` for every batch. Calendar-backed scenarios always reuse this exact calendar; the runner never calls `calendars.insert`. Snapshots and cleanup query events by the unique scenario run tag, so direct, replay, capture-attempt, and concurrent batch evidence stays isolated. Never set this variable to `primary` or to a calendar containing real events.

The Codex adapter ignores user config/plugins and runs its shell commands without the Codex sandbox. This is required for `gws` to reach the network and the macOS keychain. Only run the benchmark against its dedicated synthetic Workspace account/resources; the task prompt still restricts the agent to the registered, run-tagged resources.

Codex authoring runs are pinned to `gpt-5.6-terra` for reproducibility. Set `CORI_BENCH_CODEX_MODEL` only when intentionally producing a separate author-model comparison; the selected author model is recorded in `result.json`, and `combine` rejects batches that used different author models.

Cleanup trashes tagged Drive and Gmail message fixtures under the supplied `gmail.modify` scope; drafts and labels are removed, and run-tagged events are deleted from the calendar ID persisted in the run's cleanup registry. The shared benchmark calendar itself is never deleted.

Profiles are fixed:

- `smoke`: first task, one trial, one held-out direct/replay pair.
- `full`: all tasks, one trial, three held-out pairs.
- `publication`: all tasks, three trials, three held-out pairs per trial.

Do not begin with one monolithic full/publication run. Use `--task <id>` for a single-task qualification or `--batch INDEX/COUNT` for deterministic contiguous task batches. For example, `--profile full --batch 1/5` runs the first two catalog tasks, while `--profile full --batch 1/10` runs only the first task with full-profile repetition counts. `--task` and `--batch` are intentionally mutually exclusive.

Every run writes `progress.json` atomically and emits the same phase changes to stderr. It identifies the current task and author/capture/check/direct/replay phase plus completed and planned lane counts. Author and qualification external-state scores are measurements: they are persisted with item-level reasons but do not stop capture or held-out work. A safety violation, invalid workflow, failed `cori check`, failed Cori trace, or replay-integrity failure remains fatal and is persisted in `result.json`; replay is never silently skipped.

Held-out trial scores are measurements, not run-success gates. Direct agents are expected to vary, and Cori replay stability is visible by comparing the direct and replay score ranges and paired rows. A completed benchmark exits successfully even when a trial scores below 100. Infrastructure failures, capture/check failures, safety violations, and replay-integrity failures remain fatal.

Design-time capture is retried up to three times on fresh, independently tagged fixtures when the agent emits an invalid workflow, fails `cori check` or workflow policy, or the disposable Cori execution fails replay-integrity checks. External-state score misses do not trigger retries. Every attempt is preserved in `result.json` and attempt-scoped artifacts. External-state safety violations and preview-gate violations are never retried. If all three attempts fail, the benchmark stops because there is no executable captured workflow to compare against.

After agent or replay execution, tag-based Gmail and Drive evidence is allowed to settle before grading. Drive discovery checks both the file name and full text, avoiding false negatives from delayed Drive full-text indexing.

The default terminal output is the same readable Markdown comparison written to `scorecard.md`, including lane averages, score ranges, 100-point counts, paired findings, timing, token totals, and USD price. Prices use $2.50 per 1M input tokens and $15.00 per 1M output tokens. Pass `--json` to `run`, `report`, or `combine` for machine-readable output. The progress counters are completed-trial counts, not passing-trial counts.

After all selected batches complete, `combine` verifies identical environments, rejects overlaps or missing tasks, verifies the exact expected direct/replay count for every task, and produces the aggregate result/scorecard without rerunning any live work. Cleanup still uses each source batch run ID.

Each run writes `result.json`, `scorecard.md`, `results.csv`, before/after Workspace snapshots, transcripts, Cori JSON traces, the captured workflow, and a cleanup registry under `benchmarks/workflow-capture/artifacts/<run-id>/`. Scenario tags are namespaced by run ID, so repeating a seed cannot select fixtures left by an earlier run. Usage fields unavailable from a vendor adapter stay `null`.

Each completed run already includes a portable `viewer.html` in that same directory. Run `benchmark view --run-id <run-id>` to regenerate it after inspecting or updating artifacts. It includes a chronological comparison table spanning authoring, direct-agent, Cori qualification, and Cori replay sessions, with per-column trend markers, paired direct-versus-Cori deltas, filters, and links back to evidence. The task-first review index also embeds direct and replay grades plus compact, normalized agent conversations; new harness sessions preserve the exact prompt for each recorded turn. Snapshots, Cori traces, checks, captured workflow files, and extracted Workspace resource URLs remain linked to their original evidence. The page makes no network requests and can be opened directly, but it must stay beside the rest of the run folder so its raw-artifact links keep working.

## Safety and publication gates

- Preview must not write a workflow; approval is required before `captured-workflow/` can exist.
- Author and qualification external-state scores are reported measurements, not run-success gates. Safety violations remain fatal.
- Direct task workspaces receive only the live-task and GWS contracts. After the direct attempt is graded, the same session receives the Cori authoring guide and `cori_save_workflow` skill, so live execution and workflow capture remain separate phases.
- Captured workflows are checked with `cori check`, import `step` from `@cori-do/sdk`, have `tools_required: [gws]`, literal `gws` argv boundaries, no shell dispatchers or v1 builtins, and no credential fields.
- Replays must emit a successful Cori trace, leave the workflow hash unchanged, create Gmail drafts only, and use tagged benchmark resources.
- The report claims reuse advantage only with no safety violations, a paired bootstrap lower confidence bound of at least -5 points, and lower cumulative cost at five repetitions.

The release workflow is intentionally manual and runs only on a serialized self-hosted benchmark runner. Normal pull-request CI remains offline.

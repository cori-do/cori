# Cori workflow-capture benchmark

This package measures direct Google Workspace task completion against a captured, unchanged Cori workflow. It operates against the GWS account already configured on the machine; it does not create, manage, or require a Workspace tenant.

The ten tasks have seed-varying scenario generation, external Workspace-state grading against per-seed ground truth, safety gates, structured harness adapters, persisted transcripts/traces, and maintainer-authored reference workflows. The benchmark never treats agent text or a process exit as correctness evidence.

## What the tasks measure

Each task is written the way an operations runbook is written: the prompt states the business policy and the exact output contract, and stops there. It never says which input maps to which answer. The expected values for a run live in the scenario the seed generated, and the grader reads only those — no expected value is written into the grader, the prompt, or the reference workflow.

Five tasks are on the **deterministic** track (`sla_breach_pack`, `expense_policy_audit`, `budget_variance_deck`, `preapproved_pto_processing`, `weekly_operating_review`). Their rules are total functions over structured input, so a workflow of `cli` and `code` steps is the correct answer and they run without a model.

Five tasks are on the **hybrid** track (`support_inbox_triage`, `inbound_lead_qualification`, `vendor_invoice_intake`, `incident_postmortem_pack`, `contract_obligation_register`). Their source data is regenerated every run: wording, volume, language, layout, and values all change. Seat counts arrive as `"two teams of thirty"`, invoice layouts and field labels differ per supplier, incident transcripts are unordered and contain hypotheses the team ruled out, and contract clauses state notice periods by reference to other clauses. Three properties make the track real rather than declared:

- `benchmark validate` runs a **regex-resistance check** over every hybrid bank. For each class label it rejects the fixture if any single token appears in every member of that class and in no member of any other. A bank a keyword matcher could score is a bug, and the check is tested against the fixture this benchmark previously shipped, which it rejects.
- `inspectWorkflowPolicy` requires a captured hybrid workflow to declare at least one `step.llm`.
- A hybrid replay whose Cori trace contains no `llm` activity is recorded as a replay-integrity failure and scores zero, even when the resulting Workspace state is correct.

`support_inbox_triage` additionally declares a **re-run contract**: part of its inbox arrives already labelled by a simulated earlier run, and re-running against an unchanged mailbox must change nothing. It is the one task exempt from the "fixtures are always fresh" prompt clause.

Seeds are held out on data, not just on tags: `assertSeedsProduceDistinctFixtures` fails the run if two seeds of a task produce identical ground truth, so three paired trials pose three different problems.

## Commands

```bash
export CORI_BENCH_CALENDAR_ID='your-dedicated-secondary-calendar-id'
pnpm --dir benchmarks/workflow-capture benchmark validate
pnpm --dir benchmarks/workflow-capture benchmark preflight
pnpm --dir benchmarks/workflow-capture benchmark plan --profile full --batch 1/5
CORI_BENCH_LLM_MODEL=gpt-4o-mini pnpm --dir benchmarks/workflow-capture benchmark run --profile smoke --harness codex --seed 42
CORI_BENCH_LLM_MODEL=gpt-5.4 pnpm --dir benchmarks/workflow-capture benchmark run --profile full --batch 1/5 --harness codex --seed 42
CORI_BENCH_LLM_MODEL=gpt-5.4 pnpm --dir benchmarks/workflow-capture benchmark run --profile full --task support_inbox_triage --harness codex --seed 42
pnpm --dir benchmarks/workflow-capture benchmark report --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark view --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark cleanup --run-id <run-id>
pnpm --dir benchmarks/workflow-capture benchmark combine --run-ids <batch-1>,<batch-2>,...
```

`run` and `preflight` build `cori-cli` from the current repository checkout and pin the benchmark plus its authoring harness to the absolute `target/debug/cori` path. The selected executable's directory is also placed first on the child-process `PATH`, so an authoring agent that types `cori` still reaches that exact build. This prevents a globally installed `cori` on `PATH` from silently testing stale code. Every result records that path, whether it came from `workspace_dev` or an explicit override, and the executable SHA-256; `combine` requires the same digest across batches. Set `CORI_BENCH_CORI` only when deliberately testing an alternate executable.

`preflight` is explicit because it creates and immediately trashes a namespaced Sheets canary. It additionally requires `gws 0.22.5`, `temporal`, `deno`, valid GWS credentials/scopes, `CORI_BENCH_CALENDAR_ID`, and `CORI_BENCH_LLM_MODEL` for the five hybrid tasks. The selected model class is named in the task prompt, so an authoring agent has a legal identifier to write into `step.llm({ model })`. It verifies that the selected Cori executable reports the model provider as an available LLM capability and that the configured calendar is a writable secondary calendar. Use `GWS_BIN` only to point to an alternate GWS executable; do not put credentials in benchmark artifacts.

Create one dedicated secondary calendar outside the benchmark, then export its ID as `CORI_BENCH_CALENDAR_ID` for every batch. Calendar-backed scenarios always reuse this exact calendar; the runner never calls `calendars.insert`. Snapshots and cleanup query events by the unique scenario run tag, so author, direct, replay, and batch evidence stays isolated. Never set this variable to `primary` or to a calendar containing real events.

The Codex adapter ignores user config/plugins and runs its shell commands without the Codex sandbox. This is required for `gws` to reach the network and the macOS keychain. Every harness process receives a benchmark-owned environment: `CORI_BENCH_CORI` is the selected absolute repository binary and its directory is first on `PATH`. Before any fixture is provisioned, the harness resolves `cori` in that exact environment and rejects path, help-surface, version, or SHA-256 mismatches, then verifies GWS OAuth with a read-only Drive `about.get`. It records a hash of the authenticated Workspace account plus the exact underlying GWS executable, excluding the audit proxy. An `invalid_rapt` failure requires an interactive `gws auth login --services drive,gmail,sheets,docs,calendar,slides` before rerunning. Only run the benchmark against its dedicated synthetic Workspace account/resources; the task prompt still restricts the agent to the registered, run-tagged resources.

The measured subject is staged outside the checkout. Full and publication profiles additionally require an audited OS boundary: `sandbox-exec` denies checkout reads on macOS, while `bwrap` masks the checkout on Linux. Smoke runs may use advisory temp-directory isolation, but their result is never inference-eligible.

Codex authoring runs are pinned to `gpt-5.6-terra` for reproducibility. Set `CORI_BENCH_CODEX_MODEL` only when intentionally producing a separate author-model comparison; the selected author model is recorded in `result.json`, and `combine` rejects batches that used different author models.

Cleanup trashes tagged Drive and Gmail message fixtures under the supplied `gmail.modify` scope; drafts and labels are removed, and run-tagged events are deleted from the calendar ID persisted in the run's cleanup registry. The shared benchmark calendar itself is never deleted.

Profiles are fixed:

- `smoke`: first or explicitly selected task, one held-out direct/replay pair.
- `full`: all selected tasks, one held-out direct/replay pair per task.
- `publication`: all selected tasks, three held-out direct/replay pairs per task.

Only a complete, combined `publication` result is inferential. It requires all ten tasks, exactly three direct/replay pairs per task, the Codex harness with a recorded model, identical instrument/account/isolation identities, and no safety or integrity violation. The three regenerated seeds are repeated measurements within a task: the confidence interval first averages each task’s paired deltas, then bootstraps the ten independent task-level differences. It does not treat the thirty task/seed rows as thirty independent samples.

Do not begin with one monolithic full/publication run. Use `--task <id>` for a single-task run or `--batch INDEX/COUNT` for deterministic contiguous task batches. For example, `--profile full --batch 1/5` runs the first two catalog tasks, while `--profile full --batch 1/10` runs only the first task. `--task` and `--batch` are intentionally mutually exclusive.

Every run writes `progress.json` atomically and emits the same phase changes to stderr. TTY output resets and clears the current terminal line before every physical log line, uses CRLF to return the cursor to column zero, and applies the same rendering to the terminal failure diagnostic. Multiline diagnostics are trimmed and indented, preventing package-manager output from leaving diagonally offset progress. Repeated harness updates within the same displayed elapsed second are suppressed. While an agent turn is running, heartbeat updates include elapsed time and its transcript artifact is refreshed with partial stdout, stderr, JSONL events, and usage. Author, capture-preview, capture-approval, direct-agent, Cori, and child-process phases have configurable bounded timeouts. An interrupted agent turn therefore leaves readable progress and partial evidence instead of looking hung.

Held-out trial scores are measurements, not run-success gates. Direct agents are expected to vary, and Cori replay quality is visible by comparing the direct and replay score ranges and paired rows. A safe replay below 100 does not stop the remaining planned pairs. Capture/check failures skip held-out work for that task, and replay safety or workflow-integrity failures stop further pairs for that task; later independent tasks still collect evidence.

Design-time capture is deliberately one-shot. The harness completes one author fixture, resumes that exact conversation with `Save this as a Cori workflow under ./captured-workflow.`, records the real skill's tree and manifest preview, verifies an unchanged task-workspace content hash and unchanged complete GWS audit window, verifies that Cori was not executed, and replies with only `yes`. It injects no `CORI_AUTHORING.md`, custom capsule, repair prompt, or retry. After approval, the harness permits only explicit read-only GWS probes needed by `cori check`, rejects mutating or unclassified GWS commands during the capture turns, separately records whether the skill attempted `cori check` and whether a completed check command reported the canonical `Result: ✓ ready`, then independently runs benchmark policy plus the selected absolute Cori binary and requires the same ready result. Generated files are preserved even when a gate fails. The first scored held-out replay is the runtime validation; there is no disposable qualification run.

After agent or replay execution, tag-based Gmail and Drive evidence is allowed to settle before grading. Drive discovery checks both the file name and full text, avoiding false negatives from delayed Drive full-text indexing.

The default terminal output is the same readable Markdown comparison written to `scorecard.md`, including lane averages, score ranges, 100-point counts, paired findings, timing, token totals, and USD price. Prices use $2.50 per 1M input tokens and $15.00 per 1M output tokens. Pass `--json` to `run`, `report`, or `combine` for machine-readable output. The progress counters are completed-trial counts, not passing-trial counts.

After all selected batches complete, `combine` verifies identical environments, rejects overlaps or missing tasks, requires exactly one direct and replay trial for every expected task/seed key (rejecting duplicates, mismatches, and extras), and produces the aggregate result/scorecard without rerunning any live work. Cleanup still uses each source batch run ID.

The benchmark-owned `gws` command is a PATH proxy that records complete JSONL command evidence, fails closed on malformed or partial logs and unknown command methods, and permits retries only for an explicit set of read-only calls. Grading requires at least one audited write, registered source targets (plus run-tagged outputs created during the trial), exact draft recipients, Calendar `sendUpdates=none`, and no Gmail send call. This is an audit boundary for normal benchmark subjects, not a syscall sandbox against a deliberately evasive program that invokes an undisclosed absolute GWS binary.

Each run writes v2 `result.json`, `scorecard.md`, `results.csv`, canonical fixture baselines, after-state Workspace snapshots, complete and partial transcripts, independent check output, policy reports, Cori JSON traces, the captured workflow and hash, and a cleanup registry under `benchmarks/workflow-capture/artifacts/<run-id>/`. The result has explicit `author`, `capture`, `check`, and `replay` outcomes and per-phase timings. Scenario tags are namespaced by run ID, so repeating a seed cannot select fixtures left by an earlier run. Usage fields unavailable from a vendor adapter stay `null`.

Each completed run already includes a portable `viewer.html` in that same directory. Run `benchmark view --run-id <run-id>` to regenerate it after inspecting or updating artifacts. It includes a chronological comparison table spanning authoring, the skill preview and approval, held-out direct agents, and Cori replays, with paired direct-versus-Cori deltas, filters, and links back to evidence. The task-first review index embeds grades plus compact, normalized agent conversations; every harness session preserves its exact prompt. The page makes no network requests and can be opened directly, but it must stay beside the rest of the run folder so its raw-artifact links keep working.

## Safety and publication gates

- Preview must leave the whole task workspace hash and the complete GWS audit window unchanged; approval is required before `captured-workflow/` can exist.
- The one-shot author fixture must score at least 90 with no safety violation before capture begins. Held-out direct/replay scores remain reported measurements rather than run-success gates.
- Direct task workspaces receive only the live-task and GWS contracts. After the author execution is graded, the same session receives the unmodified `cori-save-workflow` skill and the natural save request.
- Captured workflows must have both an author-side completed `cori check` and an independent absolute-binary check report `Result: ✓ ready`; an attempted or compound command with a masked failure is not successful. Workflows also import `step` from `@cori-do/sdk`, have `tools_required: [gws]`, literal `gws` argv boundaries, no shell dispatchers or v1 builtins, and no credential fields.
- Replays must emit a successful Cori trace, match the original post-capture workflow hash both before and after execution, create Gmail drafts only, and use tagged benchmark resources.
- Replays of a hybrid task must execute at least one `llm` activity. A workflow that solved a regenerated-input task with logic fixed at capture time scores zero regardless of the Workspace state it produced.
- The report claims reuse advantage only for an inference-eligible publication result with no safety violations, a task-level paired-bootstrap lower confidence bound of at least -5 points, and lower cumulative cost at five repetitions.

## Reference workflow scope

The six executable references (`incident_postmortem_pack`, `contract_obligation_register`, `sla_breach_pack`, `expense_policy_audit`, `budget_variance_deck`, and `weekly_operating_review`) are compiler-checked, policy-checked, and exercised against deterministic three-seed offline contracts. They are examples and validation fixtures; measured publication results still come only from workflows captured during the benchmark.

Four references (`support_inbox_triage`, `inbound_lead_qualification`, `vendor_invoice_intake`, and `preapproved_pto_processing`) are explicitly structural-only. Their real task contracts require dynamic per-item fan-out/idempotent iteration that Cori v1 cannot express because builtin `map`/`for_each` support is deferred. They must not be presented as executable replay oracles.

Credentialed publication runs are intentionally manual and serialized on a dedicated benchmark machine. Pull-request CI performs only offline compilation, type checking, policy, fixture, and contract tests; it never mutates a live Workspace account.

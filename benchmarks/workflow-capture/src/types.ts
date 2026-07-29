export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export type RuntimeTrack = "deterministic" | "hybrid";
export type WorkspaceService = "gmail" | "sheets" | "docs" | "drive" | "calendar" | "slides";
export type ScenarioLane = "author" | "direct" | "replay";
export type BenchmarkProfile = "smoke" | "full" | "publication";
export type HarnessName = "codex" | "claude" | "gemini";

export interface ParameterSpec {
  name: string;
  description: string;
}

export interface RubricItem {
  id: string;
  description: string;
  points: number;
}

export interface AllowedSideEffects {
  draftsOnly: boolean;
  calendarSendUpdates: "none";
  resourceTypes: readonly WorkspaceService[];
  requiredTag: boolean;
}

export interface ResourceBlueprint {
  /** The parameter that receives this resource's live ID. */
  parameter?: string;
  role: string;
  service: WorkspaceService;
  /** The resource is expected to exist before the agent acts. */
  source: boolean;
}

export interface TaskSpec {
  id: string;
  name: string;
  domain: "support" | "sales" | "hr" | "management" | "finance" | "legal" | "engineering";
  runtimeTrack: RuntimeTrack;
  parameters: readonly ParameterSpec[];
  requiredServices: readonly WorkspaceService[];
  resources: readonly ResourceBlueprint[];
  prompt: string;
  rubric: readonly RubricItem[];
  allowedSideEffects: AllowedSideEffects;
  /**
   * The task's source data changes shape every run and the correct output
   * cannot be derived by matching literals. `hybrid` tasks must therefore
   * execute at least one `llm` activity on replay; the runner enforces it.
   */
  requiresRuntimeModel?: boolean;
  /**
   * The fixture deliberately contains state from a simulated previous run, so
   * the workflow must be safe to execute repeatedly. Tasks that declare this
   * do not receive the "fixtures are always fresh" prompt clause.
   */
  rerunContract?: boolean;
}

/**
 * One record the grader expects to find in the agent's output, keyed by a
 * stable identifier the agent can also see. Field values are the hidden answer:
 * they are never written into the fixture surface text, so a workflow can only
 * produce them by understanding the source, not by copying it.
 */
export interface GroundTruthRecord {
  key: string;
  fields: Readonly<Record<string, string>>;
}

export interface ScenarioFixture {
  role: string;
  service: WorkspaceService;
  title: string;
  table?: string[][];
  text?: string;
  events?: Json[];
  /** Gmail fixtures provision one live message per entry, in order. */
  messages?: Json[];
  /** Docs fixtures provision one live document per entry, in order. */
  documents?: readonly { title: string; text: string }[];
}

export interface RegisteredResource {
  id: string;
  role: string;
  service: WorkspaceService;
  parentId?: string;
  createdByBenchmark: boolean;
  /** Index into `Scenario.fixtures` that produced this resource. */
  fixtureIndex?: number;
  /** Canonical Gmail label state immediately after fixture provisioning. */
  initialLabelIds?: readonly string[];
}

export interface Scenario {
  id: string;
  taskId: string;
  seed: number;
  lane: ScenarioLane;
  runTag: string;
  parameters: Record<string, string>;
  fixtures: readonly ScenarioFixture[];
  expected: {
    facts: readonly string[];
    rubric: readonly RubricItem[];
    /** Hidden per-record answers the grader checks the Workspace against. */
    groundTruth: readonly GroundTruthRecord[];
    /** Derived aggregates (counts, totals) the grader checks documents against. */
    aggregates: Readonly<Record<string, string>>;
  };
  resources: readonly RegisteredResource[];
}

export interface WorkspaceSnapshot {
  capturedAt: string;
  source?: "canonical_fixture" | "workspace";
  resources: Record<string, Json>;
  drafts: readonly Json[];
  calendarEvents: readonly Json[];
}

export interface Grade {
  score: number;
  passed: boolean;
  safetyViolations: readonly string[];
  items: readonly { id: string; earned: number; max: number; note: string }[];
}

export interface HarnessUsage {
  inputTokens: number | null;
  outputTokens: number | null;
  toolCalls: number | null;
}

export interface HarnessSession {
  sessionId: string | null;
  /** Exact prompt supplied for this recorded harness turn. */
  prompt?: string;
  transcript: readonly Json[];
  usage: HarnessUsage;
  wallTimeMs: number;
  exitCode: number;
  stdout: string;
  stderr: string;
  /** True when the benchmark terminated this turn at its phase deadline. */
  timedOut?: boolean;
}

export type BenchmarkPhase = "author" | "capture" | "check" | "replay";
export type PhaseStatus = "succeeded" | "failed" | "skipped";

export interface PhaseOutcome {
  status: PhaseStatus;
  startedAt: string;
  finishedAt: string;
  wallTimeMs: number;
  inputTokens?: number | null;
  outputTokens?: number | null;
  toolCalls?: number | null;
  plannedPairs?: number;
  completedPairs?: number;
  error?: string;
}

export interface TaskPhaseOutcomes {
  author: PhaseOutcome;
  capture: PhaseOutcome;
  check: PhaseOutcome;
  replay: PhaseOutcome;
}

export interface WorkflowPolicyReport {
  ok: boolean;
  violations: readonly string[];
  workflowHash: string;
}

/** Evidence for the workflow captured from one task's authoring session. */
export interface TaskCapture {
  taskId: string;
  authorGrade: Grade;
  outcomes: TaskPhaseOutcomes;
  /** The skill displayed a tree and complete manifest before approval. */
  previewPresented: boolean;
  previewDidNotWrite: boolean;
  /** The real skill invoked `cori check` after the natural `yes` approval. */
  skillCheckObserved: boolean;
  /** A completed author command reported the canonical `Result: ✓ ready`. */
  skillCheckSucceeded: boolean;
  /** The benchmark's independent absolute-binary check reported ready. */
  benchmarkCheckSucceeded: boolean;
  /**
   * Hybrid tasks only: static lineage from an LLM output through executable
   * callbacks to a later side-effect step. Null for deterministic tasks.
   */
  runtimeModelDataflowVerified: boolean | null;
  /** Both successful checks and static policy passed. */
  checkPassed: boolean;
  policy: WorkflowPolicyReport | null;
  /** Immutable hash recorded after capture and checked around every replay. */
  workflowHash: string | null;
  /** Absolute author-workspace path used for this run; absent when capture failed. */
  workflowPath: string | null;
  /** Actionable reason the workflow cannot enter held-out replay. */
  error?: string;
}

export interface TrialResult {
  taskId: string;
  seed: number;
  lane: "direct" | "replay";
  grade: Grade;
  harness?: HarnessSession;
  tracePath?: string;
  workflowHash?: string;
  runtime?: {
    wallTimeMs: number | null;
    inputTokens: number | null;
    outputTokens: number | null;
    costEur: number | null;
  };
}

export interface BenchmarkEnvironment {
  cori: string;
  cori_source: string;
  cori_version: string | null;
  cori_sha256: string | null;
  author_cori_path: string | null;
  author_cori_version: string | null;
  author_cori_sha256: string | null;
  harness_path: string | null;
  harness_version: string | null;
  harness_sha256: string | null;
  gws: string;
  gws_path: string | null;
  gws_version: string | null;
  gws_sha256: string | null;
  temporal_path: string | null;
  temporal_version: string | null;
  temporal_sha256: string | null;
  deno_path: string | null;
  deno_version: string | null;
  deno_sha256: string | null;
  node_path: string;
  node_version: string;
  node_sha256: string | null;
  subject_isolation: string | null;
  subject_isolation_path: string | null;
  subject_isolation_sha256: string | null;
  benchmark_source_sha256: string | null;
  capture_skill_sha256: string | null;
  workspace_account_sha256: string | null;
  calendar_id: string | null;
  author_model: string | null;
  llm_model: string | null;
  os: string;
  arch: string;
  timezone: string;
}

export interface BenchmarkResultV2 {
  version: 2;
  status: "succeeded" | "failed";
  runId: string;
  profile: BenchmarkProfile;
  harness: HarnessName;
  seed: number;
  startedAt: string;
  finishedAt: string;
  environment: BenchmarkEnvironment;
  capture: {
    previewDidNotWrite: boolean;
    checkPassed: boolean;
    /** Present for a one-task run; see tasks for full/publication evidence. */
    policy: WorkflowPolicyReport | null;
    tasks: readonly TaskCapture[];
  };
  phaseTimingsMs: Record<BenchmarkPhase, number>;
  trials: readonly TrialResult[];
  metrics: {
    directWallTimeMs: number | null;
    replayWallTimeMs: number | null;
    designTokens: number | null;
    runtimeTokens: number | null;
    runtimeCostEur: number | null;
    /** Total one-time author + capture + check time for the selected task set. */
    designWallTimeMs: number | null;
    /** Sum of per-task direct means: one complete repetition of the selected set. */
    directSuiteWallTimeMs: number | null;
    /** Sum of per-task replay means: one complete repetition of the selected set. */
    replaySuiteWallTimeMs: number | null;
    breakEvenRepetitions: number | null;
  };
  summary: {
    directScore: number | null;
    replayScore: number | null;
    /** Independent task-level pairs used for inference (seeds are averaged within task). */
    pairedSampleSize: number;
    /** True only for an aggregate produced by the validated `combine` path. */
    combinedResult: boolean;
    inferenceEligible: boolean;
    pairedDifferenceCi95: readonly [number, number] | null;
    reuseAdvantageDemonstrated: boolean;
  };
  /** Present when setup or a harness fails before the benchmark can complete. */
  error?: string;
}

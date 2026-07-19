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
  domain: "support" | "sales" | "hr" | "management" | "finance";
  runtimeTrack: RuntimeTrack;
  parameters: readonly ParameterSpec[];
  requiredServices: readonly WorkspaceService[];
  resources: readonly ResourceBlueprint[];
  prompt: string;
  rubric: readonly RubricItem[];
  allowedSideEffects: AllowedSideEffects;
}

export interface ScenarioFixture {
  role: string;
  service: WorkspaceService;
  title: string;
  table?: string[][];
  text?: string;
  events?: Json[];
  messages?: Json[];
}

export interface RegisteredResource {
  id: string;
  role: string;
  service: WorkspaceService;
  parentId?: string;
  createdByBenchmark: boolean;
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
  };
  resources: readonly RegisteredResource[];
}

export interface WorkspaceSnapshot {
  capturedAt: string;
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
  /** Every independent design-time attempt, including attempts rejected before replay. */
  attempts?: readonly {
    attempt: number;
    seed: number;
    authorGrade: Grade;
    ready: boolean;
    error?: string;
  }[];
  /** One-based attempt selected for held-out replay. */
  selectedAttempt?: number;
  previewDidNotWrite: boolean;
  checkPassed: boolean;
  /**
   * Whether the disposable Cori execution passed safety and replay-integrity
   * checks. Its external-state score remains a measurement and may be below 90.
   */
  qualificationPassed?: boolean;
  qualificationGrade?: Grade;
  qualificationTracePath?: string;
  policy: WorkflowPolicyReport | null;
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

export interface BenchmarkResultV1 {
  version: 1;
  status: "succeeded" | "failed";
  runId: string;
  profile: BenchmarkProfile;
  harness: HarnessName;
  seed: number;
  startedAt: string;
  finishedAt: string;
  environment: Record<string, string | null>;
  capture: {
    previewDidNotWrite: boolean;
    checkPassed: boolean;
    /** Present for a one-task run; see tasks for full/publication evidence. */
    policy: WorkflowPolicyReport | null;
    tasks: readonly TaskCapture[];
  };
  trials: readonly TrialResult[];
  metrics: {
    directWallTimeMs: number | null;
    replayWallTimeMs: number | null;
    designTokens: number | null;
    runtimeTokens: number | null;
    runtimeCostEur: number | null;
    breakEvenRepetitions: number | null;
  };
  summary: {
    directScore: number | null;
    replayScore: number | null;
    pairedDifferenceCi95: readonly [number, number] | null;
    reuseAdvantageDemonstrated: boolean;
  };
  /** Present when setup or a harness fails before the benchmark can complete. */
  error?: string;
}

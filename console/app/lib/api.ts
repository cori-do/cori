// Typed Tauri IPC client. The Rust core exposes commands snake-cased on
// the wire (see `console/src-tauri/src/commands.rs` — every handler is
// annotated `rename_all = "snake_case"`), so the args we send and the
// payloads we receive use snake_case throughout.

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------- Error shape -------------------------------------------------

export interface IpcError {
  code:
    | "consent_required"
    | "missing_capability"
    | "needs_login"
    | "no_temporal"
    | "not_found"
    | "workflow_missing"
    | "workflow_invalid"
    | "bad_request"
    | "internal";
  message: string;
  details: unknown;
}

export function isIpcError(e: unknown): e is IpcError {
  return (
    typeof e === "object" &&
    e !== null &&
    "code" in e &&
    typeof (e as IpcError).code === "string" &&
    "message" in e
  );
}

async function call<T>(command: string, args?: object): Promise<T> {
  return invoke<T>(command, args as Record<string, unknown> | undefined);
}

// ---------- Domain types (snake_case on the wire) -----------------------

export type WorkerIdentity =
  | { kind: "person"; user_id: string }
  | { kind: "service"; pool: string };

export interface Capability {
  id: string;
  kind: string;
  authed: boolean;
  detail?: string | null;
}

export interface CapabilityReport {
  identity: WorkerIdentity;
  task_queue: string;
  capabilities: Capability[];
}

export interface WorkerEntry {
  task_queue: string;
  kind: "user" | "shared";
  is_self: boolean;
}

export interface PinnedRemote {
  key: string;
  sha: string;
  resolved_at: string;
  trusted: boolean;
}

export interface StatusResponse {
  endpoint: string;
  reachable: boolean;
  identity: WorkerIdentity;
  task_queue: string;
  self_report: CapabilityReport;
  workers: WorkerEntry[];
  pinned_remotes: PinnedRemote[];
}

export interface ActivityTrace {
  activity_id: string;
  step_name: string;
  kind: string;
  status: string;
  started_at: string;
  ended_at: string;
  duration_ms: number;
  attempts: number;
  task_queue?: string;
  worker_identity?: WorkerIdentity;
  input_summary: unknown;
  output_summary: unknown;
  output: unknown;
  cost_eur?: number;
  tokens?: { input_tokens: number; output_tokens: number };
  error?: string | null;
  notes?: string | null;
}

export type ResultFieldFormat =
  | "auto"
  | "number"
  | "currency"
  | "percent"
  | "duration";
export type ResultFieldTone = "neutral" | "success" | "warning" | "danger";
export type ResultSectionDisplay = "auto" | "table" | "list" | "text";

export interface ResolvedResultField {
  label: string;
  value: unknown;
  format: ResultFieldFormat;
  currency?: string | null;
  tone: ResultFieldTone;
}

export interface ResolvedResultSection {
  label: string;
  value: unknown;
  display: ResultSectionDisplay;
}

export interface ResolvedResultArtifact {
  label: string;
  url: string;
}

export interface ResultIssue {
  item: string;
  type:
    | "missing_value"
    | "type_mismatch"
    | "non_scalar_template"
    | "invalid_url";
  message: string;
}

export interface ResolvedResult {
  headline: string;
  description?: string | null;
  fields?: ResolvedResultField[];
  sections?: ResolvedResultSection[];
  artifacts?: ResolvedResultArtifact[];
  issues?: ResultIssue[];
}

export interface RunTrace {
  run_id: string;
  workflow_id: string;
  status: string;
  trigger: string;
  started_at: string;
  ended_at: string;
  duration_ms: number;
  workflow_content_hash?: string;
  dry_run?: boolean;
  requesting_identity?: WorkerIdentity;
  source?: unknown;
  params: unknown;
  result?: ResolvedResult | null;
  activities: ActivityTrace[];
  cost: { total_eur: number; input_tokens: number; output_tokens: number };
  error?: string | null;
}

export interface RunListEntry {
  key: string;
  utc: string;
  run_id: string;
  workflow_id: string;
  status: string;
  trigger: string;
  started_at: string;
  ended_at: string;
  duration_ms: number;
  /** True for `--dry-run` executions — excluded from median baselines. */
  dry_run: boolean;
  /** Content hash of the workflow folder at run time, when recorded. */
  workflow_content_hash?: string | null;
  cost: { total_eur: number; input_tokens: number; output_tokens: number };
  error?: string | null;
  result_headline?: string | null;
  /** Declared result fields, for workflow-specific comparisons. */
  result_fields?: ResolvedResultField[] | null;
}

export interface StepMedianEntry {
  activity_id: string;
  median_ms: number;
  samples: number;
}

/** Per-step median durations over recent real runs of one workflow. */
export const stepMedians = (args: { history_key: string; limit?: number }) =>
  call<StepMedianEntry[]>("step_medians", args);

/** Mirrors `cori_protocol::trace::WorkflowSource` — kind-tagged. */
export type WorkflowSource =
  | { kind: "local"; path: string }
  | {
      kind: "remote";
      host: string;
      repo: string;
      subpath: string;
      ref: string;
      sha: string;
    };

export interface RecentWorkflow {
  key: string;
  /** Manifest id — routing key, not display text. Use `name` if present. */
  workflow_id: string;
  /** Manifest's human-friendly name. Absent when the manifest isn't
   *  resolvable on disk right now (file moved, remote cache evicted). */
  name?: string;
  source?: WorkflowSource | null;
  last_run_at: string;
  last_status: string;
  result_headline?: string;
  run_count: number;
}

/**
 * Reconstruct a re-runnable source string from a recorded WorkflowSource.
 * Mirrors `cori_run::remote::refspec` so the round-trip stays stable.
 *
 *   • local  → the absolute (or recorded) path
 *   • remote → `host/repo[/subpath]@ref` (or `host/repo[/subpath]` if
 *              ref is empty, which means "latest semver tag")
 */
export function sourceToCli(s: WorkflowSource | null | undefined): string | null {
  if (!s) return null;
  if (s.kind === "local") return s.path || null;
  if (s.kind === "remote") {
    const base = s.subpath ? `${s.host}/${s.repo}/${s.subpath}` : `${s.host}/${s.repo}`;
    return s.ref ? `${base}@${s.ref}` : base;
  }
  return null;
}

// ---------- Workflow preflight types -----------------------------------

export type ParameterType = "string" | "number" | "boolean" | "enum" | "path";

export interface ParameterDef {
  name: string;
  type: ParameterType;
  description: string;
  values?: unknown[] | null;
  default?: unknown;
  required: boolean;
  min?: number | null;
  max?: number | null;
}

export interface ManifestSummary {
  id: string;
  name: string;
  description: string;
  parameters: ParameterDef[];
  tools_required: string[];
  mcp_servers: string[];
  body: string;
  schedule?: string | null;
  schedule_tz?: string | null;
}

export type BuiltinKind =
  | "branch"
  | "switch"
  | "for_each"
  | "loop"
  | "wait"
  | "map"
  | "parallel";

/** One nested slot of a builtin: an inline step of some kind, or a
 *  `goto` route to a later sibling step (`goto` = resolved activity id,
 *  `goto_name` = the authored step name; `"end"` finishes the run). */
export interface BuiltinSlot {
  kind?: string;
  goto?: string;
  goto_name?: string;
}

/** Control-flow facts the compiler extracted from a builtin step. */
export interface BuiltinDetail {
  /** Nested slot → what the path does (inline step or goto route). */
  nested?: Record<string, BuiltinSlot>;
  /** `wait` only: the parsed `for` spec. */
  wait?: { timeout_ms?: number; until?: string; signal?: string };
  /** `for_each` only: compile-time item cap. */
  max_items?: number;
  /** `loop` only: compile-time iteration cap. */
  max_iterations?: number;
}

export interface StepSummary {
  activity_id: string;
  name: string;
  kind: "cli" | "mcp_tool" | "code" | "llm" | "builtin";
  description: string;
  placement:
    | { type: "anywhere" }
    | { type: "local_fs" }
    | { type: "capability"; id: string };
  /** Step source file, relative to the workflow root. */
  source_path: string;
  /** The step's TypeScript source — what actually runs. Absent when the
   *  file could not be read back from the resolved folder. */
  source?: string;
  /** `cli` steps only: the frozen binary name. */
  binary?: string;
  /** `mcp_tool` steps only: the frozen server / tool pair. */
  server?: string;
  tool?: string;
  /** `llm` steps only: the normalized low/medium/high level. */
  level?: ModelLevel;
  /** `llm` steps only: which backend will serve it, under current settings. */
  llm_resolution?: LlmResolutionInfo;
  /** `builtin` steps only: the control-flow sub-kind. */
  builtin?: BuiltinKind;
  /** `builtin` steps only: nested slots, wait spec, iteration bounds. */
  builtin_detail?: BuiltinDetail;
}

export type EffectAccess = "none" | "read" | "prompt" | "may_write";

/** Mirrors `cori_compiler::effects::StepEffect`. */
export interface StepEffect {
  activity_id: string;
  step_name: string;
  kind: string;
  target: string;
  access: EffectAccess;
  external: boolean;
}

export interface ConsentRequired {
  host: string;
  repo: string;
  subpath: string;
  ref_str: string;
  sha: string;
}

export interface WorkflowPreflight {
  manifest: ManifestSummary;
  content_hash: string;
  /** Stable run-history directory for this exact local path or remote source. */
  history_key: string;
  absolute_path: string;
  steps: StepSummary[];
  required_cli_binaries: string[];
  required_mcp_servers: string[];
  required_llm_providers: string[];
  /** The worker's full capability report (null if serialization failed);
   *  the per-capability list lives at `.capabilities`. */
  capabilities: CapabilityReport | null;
  missing_capabilities: string[];
  /** Used by a step's placement but never declared in the manifest. */
  undeclared_capabilities: string[];
  /** Compiled effect surface — derived per step, never agent-declared.
   *  Conservative: an unproven operation counts as a write. */
  effects: StepEffect[] | null;
  ready: boolean;
  /** True only for builtins the runtime still defers (`map` / `parallel`).
   *  Executable control flow (branch/switch/for_each/loop/wait) runs. */
  has_builtin_step: boolean;
}

// ---------- Run events (Channel<RunEvent>) -----------------------------

export interface PlanStep {
  activity_id: string;
  step_name: string;
  kind: string;
  task_queue: string | null;
}

export type RunEvent =
  | { type: "plan"; assignments: PlanStep[] }
  | {
      type: "step_start";
      activity_id: string;
      step_name: string;
      kind: string;
      task_queue: string | null;
    }
  | {
      type: "step_finish";
      activity_id: string;
      step_name: string;
      status: string;
      duration_ms: number;
      error: string | null;
      /** Broker notes (dry-run "would call …", lints) — per-step log lines. */
      notes?: string[];
      cost_eur?: number | null;
    }
  | { type: "completed"; trace: RunTrace }
  | { type: "failed"; error: string };

// ---------- Workers + schedules ----------------------------------------

export interface WorkerDetail {
  task_queue: string;
  identity: WorkerIdentity;
  kind: "user" | "shared";
  is_self: boolean;
  capabilities: Capability[];
}

export interface WorkersResponse {
  this_queue: string;
  workers: WorkerDetail[];
}

export interface ScheduleEntry {
  id: string;
  source: string;
  resolved_sha?: string | null;
  schedule: string;
  schedule_tz?: string | null;
  /** Workflow input passed as run params on every fire. */
  input?: Record<string, unknown> | null;
  identity: string;
  enabled: boolean;
  created_at: string;
  last_reconciled_at?: string | null;
  last_fire_at?: string | null;
  last_status?: string | null;
  last_error?: string | null;
  paused_reason?: string | null;
}

export interface ScheduleDto extends ScheduleEntry {
  next_fire_at: string | null;
  is_self_identity: boolean;
}

export interface ScheduleResponse {
  id: string;
  entry: ScheduleEntry;
  next_fire_at: string | null;
}

// ---------- Stack status (global event) --------------------------------

export type StackStatus =
  | { state: "starting" }
  | { state: "up" }
  | { state: "degraded"; reason: string }
  | { state: "down"; reason: string };

// ---------- Commands ----------------------------------------------------

export const getStatus = () => call<StatusResponse>("get_status");

export const listRuns = (
  args: {
    workflow_id?: string;
    history_key?: string;
    limit?: number;
  } = {},
) =>
  call<RunListEntry[]>("list_runs", args);

export const getRun = (args: { key: string; filename: string }) =>
  call<RunTrace>("get_run", args);

export const listRecentWorkflows = () =>
  call<RecentWorkflow[]>("list_recent_workflows");

export const getStackStatus = () => call<StackStatus>("get_stack_status");

// ---------- Capabilities (Connect buttons) ------------------------------

export interface CapabilityInfo {
  id: string;
  display_name: string;
  /** Full human-facing detail, rendered as the card's tooltip. */
  details: string;
  installed: boolean;
  path?: string;
  /** undefined == probe could not run (not installed / no adapter). */
  authed?: boolean;
  /** Connect can run end-to-end from the Console. */
  connectable: boolean;
  /** false == auth-free capability: installed means ready, Connect == install. */
  requires_auth: boolean;
}

export const listCapabilities = () =>
  call<CapabilityInfo[]>("list_capabilities");

/** Long-running: resolves when the user finishes the browser consent. */
export const connectCapability = (args: { id: string }) =>
  call<CapabilityInfo>("connect_capability", args);

// ---------- LLM provider keys (shared secret store) ---------------------

export interface LlmProviderInfo {
  id: string;
  display_name: string;
  /** A key is stored for this provider (non-secret index; the value never reaches the UI). */
  configured: boolean;
  /** An env var overrides the stored key at run time. */
  env_override: boolean;
  /** Secrets go to the OS keychain (false → 0600 file fallback). */
  keychain: boolean;
}

export const listLlmProviders = () =>
  call<LlmProviderInfo[]>("list_llm_providers");

/** Verifies the key against the provider's API, then stores it in the OS keychain. */
export const setLlmProviderKey = (args: { provider: string; api_key: string }) =>
  call<LlmProviderInfo>("set_llm_provider_key", args);

export const removeLlmProviderKey = (args: { provider: string }) =>
  call<LlmProviderInfo>("remove_llm_provider_key", args);

// ---------- LLM backends (one explicit active provider) ----------------

/** `ready` is usable now; the rest each have a `remedy`. */
export type LlmBackendStatus =
  | "ready"
  | "signed_out"
  | "not_installed"
  | "no_key";

export type LlmBackendKind = "subscription" | "api";

export type ModelLevel = "low" | "medium" | "high";

export const MODEL_LEVELS: ModelLevel[] = ["low", "medium", "high"];

export interface LlmBackendModel {
  level: ModelLevel;
  /** The model that will actually be sent. */
  model: string;
  /** Cori's built-in choice, used as placeholder and reset target. */
  default_model: string;
  /** The user picked this, rather than inheriting the default. */
  overridden: boolean;
}

export interface LlmBackendInfo {
  /** `claude` | `codex` | `cursor` | `gemini-cli` | `openai` | `anthropic` | `gemini`. */
  id: string;
  display_name: string;
  kind: LlmBackendKind;
  active: boolean;
  status: LlmBackendStatus;
  remedy?: string;
  /** Subscription only: the plan that pays for it. */
  subscription_name?: string;
  /** Subscription only: the executable Cori looks for. */
  binary?: string;
  /** Subscription only: exact terminal sign-in command. */
  login_command?: string;
  /** API only: a key is stored. */
  key_configured?: boolean;
  /** API only: an env var overrides the stored key. */
  key_env_override?: boolean;
  models: LlmBackendModel[];
  model_suggestions: string[];
}

/** Which backend serves a workflow level. */
export interface LlmResolutionInfo {
  backend_id: string;
  display_name: string;
  kind: LlmBackendKind;
  level: ModelLevel;
  model: string;
}

export interface LlmSettings {
  backends: LlmBackendInfo[];
  /** Explicit selection, retained if it later becomes unavailable. */
  selected_backend?: string;
  /** What a medium step runs on right now. */
  active?: LlmResolutionInfo;
  /** Why nothing is ready, when `active` is absent. */
  blocked_reason?: string;
  /** Shared workers can't use personal subscriptions. */
  subscriptions_gated_off: boolean;
}

export const getLlmSettings = () => call<LlmSettings>("get_llm_settings");

/** Re-probe every subscription, bypassing the 30s cache. */
export const refreshLlmSettings = () =>
  call<LlmSettings>("refresh_llm_settings");

export const setLlmActiveBackend = (args: { backend?: string }) =>
  call<LlmSettings>("set_llm_active_backend", args);

/** An empty `model` clears the override and restores the default. */
export const setLlmLevelModel = (args: {
  backend: string;
  level: ModelLevel;
  model: string;
}) => call<LlmSettings>("set_llm_level_model", args);

export const previewLlmResolution = (args: { level?: ModelLevel }) =>
  call<LlmResolutionInfo | null>("preview_llm_resolution", args);

export const resolveWorkflow = (args: { source: string; update?: boolean }) =>
  call<WorkflowPreflight>("resolve_workflow", args);

export interface StartRunArgs {
  source: string;
  params: Record<string, unknown>;
  dry_run: boolean;
  update?: boolean;
  on_event: Channel<RunEvent>;
}

export const startRun = (args: StartRunArgs) =>
  call<{ run_id: string }>("start_run", args);

export const subscribeRun = (args: { run_id: string; on_event: Channel<RunEvent> }) =>
  call<Record<string, never>>("subscribe_run", args);

export const recordTrust = (args: {
  host: string;
  repo: string;
  subpath: string;
  ref_str: string;
  sha: string;
}) => call<Record<string, never>>("record_trust", args);

// ---------- Search bar ------------------------------------------------

export type PeekKind = "filter" | "local" | "remote";

export interface PeekResult {
  kind: PeekKind;
  /** Tilde-expanded path (local) or host-prefixed ref (remote shorthand). */
  normalized: string;
  /** Only meaningful when kind === "local". */
  local_exists: boolean;
  /** True when the local path is a directory containing manifest.md
   *  — i.e. the path names a workflow folder, not just any directory.
   *  Only meaningful when kind === "local"; absent otherwise. */
  is_workflow_dir?: boolean;
  /** Set to "github.com" for bare owner/repo shorthand. */
  default_host?: string;
}

export const peekSource = (input: string) =>
  call<PeekResult>("peek_source", { input });

export type DirEntryKind = "dir" | "workflow" | "file";

export interface DirEntry {
  name: string;
  kind: DirEntryKind;
  path: string;
  /** Present only when true; symlinks are never followed. */
  symlink?: boolean;
}

export interface DirListing {
  path: string;
  parent: string | null;
  entries: DirEntry[];
}

export const listDir = (path: string) =>
  call<DirListing>("list_dir", { path });

export const getLastLocalDir = () => call<string>("get_last_local_dir");

export const nearestExistingDirectory = (path: string) =>
  call<string>("nearest_existing_directory", { path });

// ---------- Remote repo browsing (Phase 4) ----------------------------

export interface RemoteWorkflowEntry {
  /** In-repo path (forward-slashes), relative to repo root. */
  subpath: string;
  name: string;
  description: string;
}

export interface RemoteListing {
  host: string;
  /** owner/repo. */
  repo: string;
  /** The subpath the user originally targeted (may be empty). */
  spec_subpath: string;
  /** What the user typed after `@` (may be empty — latest semver). */
  ref_str: string;
  /** Resolved sha. Use `sha.slice(0, 8)` for the breadcrumb pin. */
  sha: string;
  workflows: RemoteWorkflowEntry[];
}

export const listRemoteWorkflows = (refStr: string, update = false) =>
  call<RemoteListing>("list_remote_workflows", {
    ref_str: refStr,
    update,
  });

// ---------- Workers + schedules ----------------------------------------

export const listWorkers = () => call<WorkersResponse>("list_workers");

export const listSchedules = () => call<ScheduleDto[]>("list_schedules");

export const enableSchedule = (args: {
  source: string;
  schedule?: string;
  schedule_tz?: string;
  input?: Record<string, unknown>;
}) => call<ScheduleResponse>("enable_schedule", args);

export const setScheduleEnabled = (args: { id: string; enabled: boolean }) =>
  call<ScheduleResponse>("set_schedule_enabled", args);

export const updateSchedule = (args: {
  id: string;
  schedule: string;
  schedule_tz?: string;
  /** Omit to keep the stored input; pass `{}` to clear it. */
  input?: Record<string, unknown>;
}) => call<ScheduleResponse>("update_schedule", args);

export const deleteSchedule = (args: { id: string }) =>
  call<Record<string, never>>("delete_schedule", args);

// ---------- CLI install (Install `cori` command in PATH) ----------------

export interface CliInstallStatus {
  /** This app bundle ships the CLI sidecar (false in dev builds). */
  bundled: boolean;
  /** Where `cori` currently resolves on PATH, if anywhere. */
  installed_path: string | null;
  /** The installed `cori` is the one this app manages. */
  managed: boolean;
}

export interface InstallCliResult {
  path: string;
  created: boolean;
  /** When false, the install dir isn't on PATH — tell the user. */
  on_path: boolean;
}

export const getCliInstallStatus = () =>
  call<CliInstallStatus>("cli_install_status");

export const installCli = () => call<InstallCliResult>("install_cli");

// ---------- Approvals (local human-in-the-loop inbox) ------------------
// The Rust core watches ~/.cori/approvals/pending and emits
// `approvals:changed`; deciding writes the decision file and retires the
// pending item. The Console is the only writer of decisions.

export type ApprovalKind =
  | "run_confirm"
  | "trust_consent"
  | "schedule_reconsent"
  | "step_gate"
  | "reauth_required"
  | "agent_input"
  | "agent_approval";

export interface ApprovalRequest {
  nonce: string;
  kind: ApprovalKind;
  created_at: string;
  expires_at: string;
  requested_by: string;
  message: string;
  payload: Record<string, unknown>;
}

export interface ApprovalDecisionEntry {
  nonce: string;
  decision: "approved" | "declined";
  decided_at: string;
  via: string;
}

export const listApprovals = (): Promise<ApprovalRequest[]> =>
  call<ApprovalRequest[]>("list_approvals");

export const listDecidedApprovals = (): Promise<ApprovalDecisionEntry[]> =>
  call<ApprovalDecisionEntry[]>("list_decided_approvals");

export const decideApproval = (
  nonce: string,
  approved: boolean,
  // Structured payload returned to the blocked requester: an
  // `agent_input` answer ({ answer }) or a denial note ({ note }).
  response?: Record<string, unknown>,
): Promise<void> => call<void>("decide_approval", { nonce, approved, response });

export const onApprovalsChanged = (
  cb: (pending: ApprovalRequest[]) => void,
): Promise<UnlistenFn> =>
  listen<{ pending: ApprovalRequest[] }>("approvals:changed", (ev) =>
    cb(ev.payload.pending),
  );

// ---------- Authoring sessions (MCP agents editing workflows) -----------
// One row per journalled editing session in ~/.cori/sessions/. The
// Console reads; the only write is the human's Stop.

export interface AuthoringSessionEvent {
  seq: number;
  kind: string;
  rel_path: string | null;
  to_rel_path: string | null;
  note: string | null;
  ts: string;
}

/** Net effect of the session on one file, replayed from the journal. */
export type ProposalChange =
  | "added"
  | "modified"
  | "renamed"
  | "deleted"
  | "unchanged";

/** One workflow step as the reviewer sees it — compiled identity,
 * proven effect, and what the session did to its source file. */
export interface ProposalStep {
  activity_id: string;
  index: number;
  name: string;
  description: string;
  kind: string;
  target: string;
  access: "none" | "read" | "prompt" | "may_write";
  external: boolean;
  source_path: string;
  source_sha256?: string | null;
  change: ProposalChange;
  renamed_from?: string | null;
}

export interface ProposalFileChange {
  rel_path: string;
  change: ProposalChange;
  renamed_from?: string | null;
}

export interface ProposalResolution {
  decision: "accepted" | "rejected";
  by: string;
  at: string;
  published_version?: number | null;
  reason?: string | null;
  discarded_changes: boolean;
}

/** The frozen review card an agent submitted via the `propose` tool. */
export interface SessionProposal {
  session_id: string;
  agent: string;
  workflow_dir: string;
  workflow_name: string;
  description: string;
  summary: string;
  base_version: number | null;
  manifest_version: number;
  proposed_at: string;
  steps: ProposalStep[];
  files: ProposalFileChange[];
  rollup: {
    pure: number;
    reads: number;
    prompts: number;
    may_writes: number;
    external: number;
  };
  warnings: string[];
  resolution?: ProposalResolution | null;
}

export interface AuthoringSession {
  session_id: string;
  agent: string;
  workflow_dir: string;
  folder_name: string;
  state: "writing" | "proposed" | "stopped";
  stop_reason: string | null;
  created_at: string;
  updated_at: string;
  current_seq: number;
  last_event: AuthoringSessionEvent | null;
  proposal: SessionProposal | null;
}

export const listAuthoringSessions = (): Promise<AuthoringSession[]> =>
  call<AuthoringSession[]>("list_authoring_sessions");

/** Full journal of one session, oldest-first — live diff + ledger data. */
export const sessionJournal = (
  sessionId: string,
): Promise<AuthoringSessionEvent[]> =>
  call<AuthoringSessionEvent[]>("session_journal", { session_id: sessionId });

export const stopAuthoringSession = (
  sessionId: string,
  reason?: string,
): Promise<void> =>
  call<void>("stop_authoring_session", { session_id: sessionId, reason });

/** Rewind the folder to the state after journal entry `seq`. */
export const rewindAuthoringSession = (
  sessionId: string,
  seq: number,
): Promise<void> =>
  call<void>("rewind_authoring_session", { session_id: sessionId, seq });

/** Accept a pending proposal: publishes the next version, stops the
 * session. Returns { version, previous_version, schedules }. */
export const acceptAuthoringProposal = (
  sessionId: string,
): Promise<{ version: number; previous_version: number; schedules: string[] }> =>
  call("accept_authoring_proposal", { session_id: sessionId });

/** Reject a pending proposal: stops the session with the reason,
 * optionally rewinding the folder to its pre-session state. */
export const rejectAuthoringProposal = (
  sessionId: string,
  reason?: string,
  discardChanges = false,
): Promise<void> =>
  call<void>("reject_authoring_proposal", {
    session_id: sessionId,
    reason,
    discard_changes: discardChanges,
  });

/** Pushed by the Rust watcher whenever any session's journal moves. */
export const onSessionsChanged = (
  cb: (sessions: AuthoringSession[]) => void,
): Promise<UnlistenFn> =>
  listen<{ sessions: AuthoringSession[] }>("sessions:changed", (ev) =>
    cb(ev.payload.sessions),
  );

// ---------- Self-update -------------------------------------------------

export const installUpdate = (): Promise<void> => call<void>("install_update");

export const onUpdaterAvailable = (
  cb: (version: string) => void,
): Promise<UnlistenFn> =>
  listen<{ version: string }>("updater:available", (ev) => cb(ev.payload.version));

// ---------- Global event subscriptions ---------------------------------

export const onStackStatus = (
  cb: (status: StackStatus) => void,
): Promise<UnlistenFn> => listen<StackStatus>("stack:status", (ev) => cb(ev.payload));

export const onScheduleFired = (
  cb: (payload: { id: string; run_id: string; fired_at: string }) => void,
): Promise<UnlistenFn> =>
  listen<{ id: string; run_id: string; fired_at: string }>("schedule:fired", (ev) =>
    cb(ev.payload),
  );

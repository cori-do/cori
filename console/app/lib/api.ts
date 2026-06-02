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
  activities: ActivityTrace[];
  cost: { total_eur: number; input_tokens: number; output_tokens: number };
  error?: string | null;
}

export interface RunListEntry extends RunTrace {
  key: string;
  utc: string;
}

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

export interface StepSummary {
  activity_id: string;
  name: string;
  kind: "cli" | "mcp_tool" | "code" | "llm" | "builtin";
  description: string;
  placement:
    | { type: "anywhere" }
    | { type: "local_fs" }
    | { type: "capability"; id: string };
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
  absolute_path: string;
  steps: StepSummary[];
  required_cli_binaries: string[];
  required_mcp_servers: string[];
  required_llm_providers: string[];
  capabilities: Capability[];
  missing_capabilities: string[];
  ready: boolean;
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
  identity: string;
  enabled: boolean;
  created_at: string;
  last_reconciled_at?: string | null;
  last_fire_at?: string | null;
  last_status?: string | null;
  last_error?: string | null;
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

export const listRuns = (args: { workflow_id?: string; limit?: number } = {}) =>
  call<RunListEntry[]>("list_runs", args);

export const getRun = (args: { key: string; filename: string }) =>
  call<RunTrace>("get_run", args);

export const listRecentWorkflows = () =>
  call<RecentWorkflow[]>("list_recent_workflows");

export const getStackStatus = () => call<StackStatus>("get_stack_status");

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
}) => call<ScheduleResponse>("enable_schedule", args);

export const setScheduleEnabled = (args: { id: string; enabled: boolean }) =>
  call<ScheduleResponse>("set_schedule_enabled", args);

export const deleteSchedule = (args: { id: string }) =>
  call<Record<string, never>>("delete_schedule", args);

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

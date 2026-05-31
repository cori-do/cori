// Typed fetch helpers. Everything goes through `apiGet`/`apiPost` so
// the session cookie + bearer token rules stay centralised.

import { getMasterToken } from "./session";

export class ApiError extends Error {
  status: number;
  body: unknown;
  constructor(status: number, body: unknown, message: string) {
    super(message);
    this.status = status;
    this.body = body;
  }
}

async function jsonOrText(res: Response): Promise<unknown> {
  const ct = res.headers.get("content-type") ?? "";
  if (ct.includes("application/json")) {
    return res.json();
  }
  return res.text();
}

export async function apiGet<T>(path: string): Promise<T> {
  const res = await fetch(path, {
    method: "GET",
    credentials: "include",
    headers: { accept: "application/json" },
  });
  if (!res.ok) {
    const body = await jsonOrText(res).catch(() => null);
    if (res.status === 401) {
      throw new Response("unauthorized", { status: 401 });
    }
    throw new ApiError(res.status, body, `${res.status} ${res.statusText}`);
  }
  return (await res.json()) as T;
}

export async function apiPost<T>(path: string, body: unknown): Promise<T> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    accept: "application/json",
  };
  const bearer = getMasterToken();
  if (bearer) headers.authorization = `Bearer ${bearer}`;

  const res = await fetch(path, {
    method: "POST",
    credentials: "include",
    headers,
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const body = await jsonOrText(res).catch(() => null);
    throw new ApiError(res.status, body, `${res.status} ${res.statusText}`);
  }
  return (await res.json()) as T;
}

// -- Response shapes ----------------------------------------------------

export interface WorkerIdentity {
  Person?: { user_id: string };
  Service?: { pool: string };
}

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

export interface RunSummary {
  run_id: string;
  workflow_id: string;
  status: string;
  trigger: string;
  started_at: string;
  ended_at: string;
  duration_ms: number;
  cost?: { total_eur: number; input_tokens: number; output_tokens: number };
  error?: string | null;
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

export interface RunTrace extends RunSummary {
  workflow_content_hash?: string;
  dry_run?: boolean;
  requesting_identity?: WorkerIdentity;
  source?: unknown;
  params: unknown;
  activities: ActivityTrace[];
  cost: { total_eur: number; input_tokens: number; output_tokens: number };
}

/// One row from `GET /api/runs`. Extends `RunTrace` with the on-disk
/// coordinates so we can deep-link to `/runs/:key/:utc`.
export interface RunListEntry extends RunTrace {
  key: string;
  utc: string;
}

export interface RecentWorkflow {
  key: string;
  workflow_id: string;
  source?: unknown;
  last_run_at: string;
  last_status: string;
  run_count: number;
}

// -- Workflow preflight (GET /api/workflow) ------------------------------

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
  ref: string;
  sha: string;
  url: string;
  declared_capabilities: string[];
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
  consent_required: ConsentRequired | null;
}

// -- Trigger (POST /api/runs) --------------------------------------------

export interface TriggerBody {
  source: string;
  params: Record<string, unknown>;
  dry_run: boolean;
  update?: boolean;
}

export interface TriggerResponse {
  run_id: string;
  stream_url: string;
}

export interface TriggerConflict {
  consent_required: ConsentRequired;
}

// -- SSE event envelope --------------------------------------------------

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

export interface PlanStep {
  activity_id: string;
  step_name: string;
  task_queue: string;
}

// -- Workers (GET /api/workers) ------------------------------------------

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

// -- Schedules (GET / POST / PATCH / DELETE /api/schedules[/:id]) --------

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

export interface CreateScheduleBody {
  source: string;
  schedule?: string;
  schedule_tz?: string;
}

export interface ScheduleResponse {
  id: string;
  entry: ScheduleEntry;
  next_fire_at: string | null;
}

export async function apiPatch<T>(path: string, body: unknown): Promise<T> {
  return apiMutate<T>(path, body, "PATCH");
}

export async function apiDelete<T>(path: string): Promise<T> {
  return apiMutate<T>(path, null, "DELETE");
}

async function apiMutate<T>(
  path: string,
  body: unknown,
  method: "PATCH" | "DELETE",
): Promise<T> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    accept: "application/json",
  };
  const bearer = getMasterToken();
  if (bearer) headers.authorization = `Bearer ${bearer}`;
  const res = await fetch(path, {
    method,
    credentials: "include",
    headers,
    body: body == null ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    const errBody = await res.json().catch(() => null);
    throw new ApiError(res.status, errBody, `${res.status} ${res.statusText}`);
  }
  return (await res.json()) as T;
}

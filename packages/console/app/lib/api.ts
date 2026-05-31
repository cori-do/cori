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

export interface RecentWorkflow {
  key: string;
  workflow_id: string;
  source?: unknown;
  last_run_at: string;
  last_status: string;
  run_count: number;
}

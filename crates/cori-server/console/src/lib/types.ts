// The shapes of the server's /v1 responses the console reads (see cori-server src/api.rs); fields it ignores are left out.

/** What the console shows the caller: `admin` the platform too, `builder` the apps they build, `user` the apps their roles open. */
export type Persona = "admin" | "builder" | "user";
/** The organization a request acts in, on a deployment of several (`/v1/me`): its id, the slug people know it by, its name. */
export type Organization = { id: string; slug?: string | null; name?: string | null };
export type Me = {
  sub: string; roles?: string[]; email?: string | null; persona?: Persona; organization?: Organization;
  /** someone outside the organization an app was shared with: they reach those apps only */
  guest?: boolean;
  /** they reach their apps and nothing else of the organization: a guest, or a person with no power who builds no app */
  apps_only?: boolean;
  /** admins: the apps shared outside the organization waiting for their decision */
  access_waiting?: number;
  can?: { admin?: boolean; create_apps?: boolean; build?: boolean; operate?: boolean; audit?: boolean; usage?: boolean; approve?: boolean; store_admin?: boolean; people?: boolean };
  /** where the organization's people find their apps (its apps portal), when it has one */
  portal_url?: string | null;
  /** the console's own address */
  console_url?: string;
  /** the providers a person connects their own accounts to here (Google): the console's Connected accounts */
  providers?: { name: string; label: string }[];
};
/** `GET /v1/me/apps`: an app the caller builds (`build`) or holds roles in (`roles`). */
export type MyApp = {
  id: string; name?: string; build: boolean; owner?: string | null; roles: string[]; promoted?: string | null; promoted_release?: string | null;
  protected?: boolean; front_url?: string | null; stores: string[]; waiting_for_me: number; my_runs: number;
  /** the live release's `[app] description`, its first line */
  description?: string | null;
  /** the live release's favicon, on the apps portal's host (`/__cori/icons/<app>`) */
  icon?: string | null;
};
/** `GET /v1/auth/config`: how to sign in, the MCP endpoint, and the deployment's plugin for coding agents. */
export type AuthConfig = { mode?: "builtin" | "dev" | "off"; issuer?: string; web_client_id?: string; resource?: string; mcp?: string; agent_plugin?: string; /** where people manage their own account */ account_url?: string;
  /** scopes a sign-in asks for besides its own: the deployment's API, for an issuer that picks the token's audience by scope */
  scopes?: string[];
  /** the public client MCP hosts sign in with where the issuer registers none for them, and its callback's fixed port */
  mcp_client_id?: string; mcp_callback_port?: number };

/** `GET /v1/start`: an organization's first steps, each done when the server saw it done. */
export type StartClient = { done: boolean; detail?: string; by?: string; first_at?: string; last_at?: string };
export type StartState = {
  /** where the organization's people find their apps, when it has an apps portal */
  portal_url?: string | null;
  agent: StartClient;
  cli: StartClient;
  llm: { done: boolean; own?: boolean; starter?: LlmStarter | null; can_add: boolean };
  app: { done: boolean; id?: string | null };
  release: { done: boolean; app?: string; release?: string | null; preview_url?: string };
};

/** `GET /v1/apps`: the apps the caller builds, with how each ships and runs. */
export type AppSummary = {
  id: string; name?: string | null; promoted?: string | null; promoted_release?: string | null; workflows: number; protected?: boolean; runs_24h: number; pending_approvals: number;
  owner?: string | null; builders?: string[]; versions?: number; created_at?: string;
  /** the newest release deployed (live or not) */
  last_version?: { hash: string; status: string; created_at: string; created_by: string } | null;
  front_url?: string | null;
};

export type Param = { name: string; type?: string; description?: string; default?: unknown; required?: boolean;
  /** `file` / `files`: the types it takes, the largest file, how many files at most. */
  accept?: string[]; max_size?: string | number; max_files?: number };
export type Endpoint = { mode: string; auth: string; allow?: { roles?: string[] }; rate_limit?: string };
export type PromotedWorkflow = { id: string; endpoint?: Endpoint | null; schedule?: string | null; parameters?: Param[] };
export type AppDetail = {
  id: string; name?: string; promoted?: string | null; promoted_release?: string | null; owner?: string | null; builders?: string[];
  /** a user's own roles in the app; for its builders, `app_roles` are every role the app knows */
  roles?: string[]; app_roles?: string[];
  /** builders only: whether the server has a store database (`CORI_STORE_DATABASE_URL`) to create the app's stores in */
  stores_configured?: boolean;
  protected?: boolean; front_url?: string | null; build?: boolean; promoted_workflows?: PromotedWorkflow[] | unknown;
};

export type Meters = { runs?: Record<string, number>; llm: { calls: number; tokens_in: number; tokens_out: number; cost_eur: number | string; month?: LlmMonth }; storage?: AppStorage };

/** What an app's stores, releases and files take (measured every few minutes), against its limit: `own`, set by an
 *  admin, within the organization's for each app (`per_app`); `limit` is the one that applies. */
export type AppStorage = { bytes: number; limit: number | null; own: number | null; per_app: number | null; organization: { bytes: number; limit: number | null } };

/** An app's LLM spend this calendar month (UTC) against its budget; `blocked` once the budget is spent. */
export type LlmMonth = { app?: string; month: string; calls: number; tokens_in: number; tokens_out: number; spent_eur: number; budget_eur: number | null; remaining_eur: number | null; blocked: boolean };
export type LlmModel = { id: string; input_eur_per_mtok: number; output_eur_per_mtok: number };
/** `managed`: declared by the deployment's providers file, applied at every start: read only here. */
export type LlmProvider = { name: string; kind: string; base_url?: string | null; models: LlmModel[]; enabled: boolean; has_key: boolean; updated_at: string; updated_by: string; managed?: boolean };
export type LlmTier = { provider: string; model: string; managed?: boolean };
/** The deployment's starter model: one model at no cost to the org, a few calls a minute and a day (0: no limit), for the
 * tiers the org set no default model for (`tiers`, on the LLM page). */
export type LlmStarter = { model: string; calls_per_minute: number; calls_per_day: number; calls_today: number; note?: string | null; tiers?: string[] };
/** `GET /v1/llm`: the org's providers, default model per tier, every app's month, and the starter model when there is one. */
export type LlmOverview = { providers: LlmProvider[]; tiers: Record<string, LlmTier>; tier_names: string[]; kinds: string[]; apps: LlmMonth[]; starter?: LlmStarter | null; can_edit: boolean };

export type RunSummary = { run_id: string; workflow: string; status: string; user: string; started_at?: string; ended_at?: string | null; parent_run?: string | null };
/** A run's step: from its trace once the run ended (`attempts`, how many its activity took; `path`, the way a branch, a
 * switch or a try went), else from the event each attempt sends (`attempt`, which one it was). */
export type Step = { name: string; kind: string; status: string; attempts?: number; attempt?: number; ms?: number; notes?: string[]; error?: string; cost_eur?: number; path?: string };

/** A step as the run's release defines it (`GET /v1/apps/{app}/runs/{run}/steps`): its author's description, what it works
 * with (from what its file writes out, never its code) and the data it uses (`uses`: `params.x`, `steps.<step>.y`,
 * `user.email`, `item.z`…). `file`, `line` and `query_text` for the app's builders. */
export type StepPlan = {
  name: string; kind: string; description?: string; nested_in?: string; slot?: string | null;
  access: "none" | "read" | "prompt" | "may_write"; external: boolean; uses?: string[];
  connector?: string; tool?: string; method?: string; workflow?: string; activity?: string; program?: string;
  tier?: string; fallback?: string; query?: boolean; query_text?: string;
  /** a person's step: an `approval` or an `input`, asked of a role or of the person who started the run */
  ask?: "approval" | "input"; to?: { role?: string; user?: string }; timeout?: string; on_timeout?: string;
  wait_ms?: number; max_iterations?: number; goto?: { slot: string; to: string }[];
  file?: string; line?: number;
};
/** An attempt of a step the worker ran, as its event says: what the step received and gave back (absent on runs from before
 * they were kept, `too_large` when left out). */
export type StepAttempt = { at: string; attempt?: number; status: string; ms?: number; input?: unknown; output?: unknown; error?: string; too_large?: { input?: number; output?: number } };
/** A person a step asked: who (`to`: a role, or `user:initiator`), what they were shown, who decided and when. */
export type StepDecision = { step: string; to: string; status: string; asked_at: string; decided_by?: string | null; decided_at?: string | null; shown?: unknown };
/** A model call, as the server priced it (for the app's builders): `prompt`, the instructions it was given. */
export type ModelCall = { step: string; tier: string; model: string; provider?: string | null; mode: string; valid: boolean; ms: number; tokens_in: number; tokens_out: number; cost_eur: number; prompt?: string | null; at: string };
export type RunSteps = {
  workflow: { id: string; name?: string; description?: string; release?: string | null };
  /** empty when the run's release is gone */
  steps: StepPlan[];
  attempts: Record<string, StepAttempt[]>;
  decisions: StepDecision[];
  children: { step: string; run_id: string; workflow: string; at: string; input?: unknown }[];
  model_calls?: ModelCall[];
};
export type RunDetail = {
  workflow: string; status: string; started_at?: string; ended_at?: string; error?: string; params?: unknown; outputs?: unknown; steps?: Step[];
  result?: { headline: string; fields?: { label: string; value: unknown }[] };
};
export type RunEvent = { kind: string; payload: Step };

export type Approval = {
  id: string; kind?: string; status?: string; workflow?: string; step: string; to: string; requested_by: string; run_id?: string; created_at?: string;
  /** who asked, as the server saw them: the person deciding may not read the organization's people */
  requester?: { name?: string | null; email?: string | null };
  payload: { short?: string; reason?: string; delta?: unknown; store?: string; target?: string; payload?: unknown; fields?: unknown } & Record<string, unknown>;
};

/** What stands between a release and going live, as the server sees it for the caller. */
export type Gate = { state: "live" | "ready" | "needs_approval" | "requested" | "approved"; reason?: string; by?: string; against?: string | null };
export type Version = {
  /** `v9` (or a short hash for a version deployed before releases were numbered) */
  release: string; seq?: number | null; short: string; message?: string | null;
  section: "live" | "newer" | "older"; live: boolean; was_live: boolean; live_periods: { from: string; until?: string | null }[];
  gate: Gate;
  /** what Go live does for the caller: now, approve (and go live now), request, pending, or null for the live release */
  go_live: "now" | "approve" | "request" | "pending" | null;
  pending_migrations: string[]; schema_ahead: string[];
  delta?: { new_or_changed_writes?: string[]; added_workflows?: string[]; removed_workflows?: string[] } | null;
  preview: { url: string; stores: string[] };
  git_branch?: string | null;
  hash: string; status: string; created_by: string; created_at: string; preview_url: string; sandbox?: string[];
  git_commit?: string; git_dirty?: boolean; approved_by?: string; promotion_pending?: boolean; needs_approval?: boolean; adds_writes?: boolean;
  /** approved against a version that is no longer the promoted one */
  approval_lapsed?: boolean;
  /** what promoting it does for the caller now; null for the promoted version */
  action?: "promote" | "approve_and_promote" | "request" | "pending" | null;
};
export type ReleaseEvent = { kind: string; by: string; at: string; details?: Record<string, unknown> | null };
export type VersionDetail = { release: string; events: ReleaseEvent[]; review: { needs_approval?: boolean; delta?: unknown; workflows?: { id: string; steps: { name: string; external?: boolean; target?: string; access?: string }[] }[] } };
export type Deployment = { at: string; kind: string; hash: string; previous?: string; by: string; migrations?: string[] };

/** A store database: its builders get where it lives (`database`) and every role it defines; an app's user the live stores
 *  where they hold a role, with their roles there. */
export type StoreInstance = { app: string; name: string; target: string; database?: string; roles?: string[]; seeded?: boolean; updated_at?: string };

export type Connector = {
  name: string; kind: string; description?: string; tools?: { name: string }[]; roles?: string[];
  transport: { url?: string; command?: string[] }; declared_by?: string; declared_at?: string;
  /** its calls carry a connected account's token: each person's own (or the app's), at this provider */
  oauth?: { provider: string; label: string; scopes: string[] } | null;
  /** the server's deployment declares it for every organization (read-only), or the organization did */
  provided?: "deployment" | "organization";
};
/** A provider the deployment connects accounts to, and one owner's connection to it (a person's own, or an app's). */
export type ProviderAccount = {
  provider: string; label: string; connectors: string[]; scopes: string[];
  connection: { account?: string | null; scopes: string[]; missing: string[]; connected_at: string; since: string; used_at?: string | null } | null;
};
export type ConnectorRequest = { at: string; url: string; requested_by: string; reason?: string };
export type Pool = { name: string; last_seen?: string; capabilities?: unknown };
/** A secret's name, never its value: the deployment's (`app` null) or an app's; `key_by` is what wraps its data key
 * (`master`, or a key service's key such as `vault-transit:<key>:<version>`). */
export type Secret = { name: string; app?: string | null; kind: string; version: number; key_by?: string; updated_at?: string };
export type Doctor = { checks: { ok: boolean; name: string; detail: string }[] };
export type AuditRow = { at: string; actor: string; action: string; object: string; details: unknown };

// `cori/describe` on a store: the caller's store role and, per table, what that role may do.
export type Column = {
  name: string; type: string; nullable?: boolean; pk?: boolean; default?: unknown; fk?: { table: string; column: string };
  read?: boolean; create?: boolean; update?: boolean; approval?: boolean;
};
/** Which rows the role reaches: its own (tenant column = caller), all, a shared table, a table without tenancy, or
 * none at all (not named in rules.yaml: the database hides every row). */
export type RowScope = "own" | "all" | "global" | "unscoped" | "hidden";
export type Table = {
  name: string; read?: boolean; columns: Column[]; primary_key: string[]; tenant?: string; delete?: boolean; max_per_page?: number;
  tools: { create?: string; update?: string; delete?: string } & Record<string, string | undefined>;
  // enforced by Postgres (grants and row level security compiled from the role files)
  scope?: RowScope; rls?: boolean; soft_delete?: string | null; delete_is_soft?: boolean;
  /** columns the role files give the role but the database refuses it: the enforced policy is not the files' */
  drift?: string[];
};
export type Describe = {
  role?: string | null; tables: Table[]; roles: { name: string; description?: string }[]; you: { sub: string; roles?: string[] };
  /** the caller reads as the read-only inspector (the app's owner, builders, admins): no business role of the store */
  inspector?: boolean;
  policy?: { hash: string; enforced_by: string; warnings?: string[] };
};

// What the client hook told the identity provider (`crate::client_hook`): each app's sign-in client, its roles, the events.
export type IdentitySettings = { hook: boolean; role_events: boolean; roles_at: "deploy" | "promote" };
/** A role of an app as the identity provider was told of it: `pending` until its IAM confirms it (200), `deleting` once no
 * kept version declares it (`role.deleted` follows after a grace period). */
export type IdentityRole = { role: string; description?: string | null; note?: string | null; state: "pending" | "confirmed" | "deleting"; declared_at: string; undeclared_since?: string | null; confirmed_at?: string | null };
/** An app's sign-in client: `ready` (bound), `pending` (the organization is creating it), `none`. */
export type IdentityApp = { app: string; name?: string | null; owner?: string | null; client_id?: string | null; client_status: "ready" | "pending" | "none"; client_note?: string | null; origin?: string; roles: IdentityRole[] };
export type IdentityEvent = {
  id: string; seq: number; app: string; event: string; body: unknown; status: "queued" | "done" | "dead" | "cancelled"; attempts: number;
  next_at: string; occurred_at: string; delivered_at?: string | null; last_status?: number | null; last_answer?: string | null;
};
/** `GET /v1/identity/catalog`. */
export type IdentityCatalog = IdentitySettings & { apps: IdentityApp[] };
/** `GET /v1/apps/{app}/identity`: the app's entry in the catalog and its last events; `may_sync`: the caller may ask the hook again. */
export type AppIdentity = IdentitySettings & Partial<IdentityApp> & { events: IdentityEvent[]; may_sync: boolean; /** where the identity provider grants the app's roles */ admin_url?: string | null };

// People and access (`GET /v1/people`, `/v1/invitations`, `/v1/apps/{app}/access`): asked of the identity provider
// through the identity hook, which decides; what the caller may change follows what its receiver handles.
/** What the caller may change, by what the receiver handles and the powers they hold. */
export type PeopleCan = { invite?: boolean; set_roles?: boolean; remove?: boolean; cancel_invitations?: boolean; resend_invitations?: boolean; grant?: boolean; revoke?: boolean; rename?: boolean; leave?: boolean };
/** A member role: the identity provider's (`offered`), or one seen on its people; `powers`, what it gives in Cori. */
export type MemberRole = { name: string; description?: string | null; offered: boolean; powers: string[]; /** a guest's role (`CORI_GUEST_ROLES`) */ guest?: boolean };
/** A person of the organization, with their member roles, their apps and their roles in them. */
export type PersonRow = {
  id: string; email?: string | null; name?: string | null; roles?: string[]; guest?: boolean; joined_at?: string | null; first_seen?: string; last_seen?: string;
  /** a guest's: the apps shared with them, who shared each and who approved */
  shares?: { app: string; roles: string[]; by: string; approved_by?: string | null; until?: string | null; status: string }[];
  apps: { owns: string[]; builds: string[]; uses: { app: string; roles: string[] }[] };
};
/** A change asked through Cori, and where it stands with the identity provider. */
export type AccessRequest = {
  id: string; kind: string; event: string; person?: string | null; email?: string | null; app?: string | null; roles?: string[] | null; details?: Record<string, unknown> | null;
  guest: boolean; until?: string | null; note?: string | null; asked_by: string; asked_at: string; approved_by?: string | null; decided_at?: string | null;
  status: "awaiting_approval" | "sent" | "pending" | "done" | "refused" | "given_up" | "cancelled" | "ended"; answer?: string | null; updated_at: string;
};
export type PeopleList = { people: PersonRow[]; source: "directory" | "seen"; can: PeopleCan; member_roles: MemberRole[]; requests: AccessRequest[]; admin_url?: string | null; problem?: string };
/** An invitation not accepted yet, as the identity provider lists it. */
export type Invitation = { id: string; email: string; roles?: string[]; inviter?: string | null; created_at?: string | null; expires_at?: string | null; grants?: { app?: string | null; client_id?: string; roles: string[] }[]; guest?: boolean };
export type Invitations = { invitations: Invitation[] | null; requests: AccessRequest[] };
/** An app's access: who builds it, who holds each role it declares, the requests about it. */
export type AppAccessInfo = {
  app: string; client_id?: string | null; owner?: string | null; builders: string[]; source: "directory" | "none";
  roles: { role: string; description?: string | null; declared: boolean; holders: { id: string; email?: string | null; name?: string | null; guest?: boolean }[] }[];
  requests: AccessRequest[]; can: { grant: boolean; revoke: boolean; invite: boolean }; admin_url?: string | null;
  /** its owner left the organization: an admin hands it to another */
  owner_left?: boolean;
  /** its roles are given through Cori (by its owner and admins), not in the identity provider */
  grants_here?: boolean;
  /** what the deployment says of giving roles (`CORI_IDENTITY_ACCESS_MESSAGE`): how long a change takes, whom to ask */
  message?: string | null;
};
/** `GET /v1/people/{id}/leaving`: what someone leaves behind if removed. */
export type Leaving = { owns: string[]; builds: string[]; agent_sessions: number; waiting: { app: string; run: string; step: string }[] };
/** `GET /v1/me/organizations`: the organizations the caller belongs to on this server (`current`: this one). */
export type MyOrganization = { id: string; slug?: string | null; name?: string | null; current?: boolean };
/** …and whether they may start an organization of their own, as the identity provider says: where (`url`), or why not
 *  (`reason`, shown as it is); absent when it offers none. */
export type MyOrganizations = { organizations: MyOrganization[]; start?: { url?: string; reason?: string } };
/** `GET /v1/organization`: the organization a request acts in, as its identity provider names it, and what the caller may do. */
export type OrganizationInfo = {
  id?: string; slug?: string | null; name?: string | null; roles?: MemberRole[];
  can: PeopleCan & { delete?: boolean; export?: boolean }; admin_url?: string | null;
};

/** `GET /v1/agent-sessions`: a coding agent's session (the MCP server's), one per app it signed in for. */
export type AgentSession = { jti: string; sub: string; app: string; mcp_session?: string | null; issued_at: string; expires_at: string; revoked_at?: string | null };
/** `GET /v1/apps/{app}/schedules`. */
export type Schedule = { id: string; workflow: string; cron: string; tz: string; mode: string; updated_at: string; runs_as?: { sub: string; roles?: string[] } | null; problem?: string | null };

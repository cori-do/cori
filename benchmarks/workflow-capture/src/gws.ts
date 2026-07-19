import { spawn } from "node:child_process";

import { taskById } from "./tasks.js";
import type { Json, RegisteredResource, Scenario, ScenarioFixture, WorkspaceSnapshot } from "./types.js";

export interface ProcessResult {
  code: number;
  stdout: string;
  stderr: string;
}

export type ProcessRunner = (file: string, args: readonly string[], cwd?: string) => Promise<ProcessResult>;

export const benchmarkCalendarEnv = "CORI_BENCH_CALENDAR_ID";

export function configuredBenchmarkCalendarId(): string | undefined {
  const calendarId = process.env[benchmarkCalendarEnv]?.trim();
  return calendarId || undefined;
}

export function requireBenchmarkCalendarId(): string {
  const calendarId = configuredBenchmarkCalendarId();
  if (!calendarId) {
    throw new Error(
      `${benchmarkCalendarEnv} is required for Calendar-backed benchmark tasks; create one dedicated secondary calendar and export its ID`,
    );
  }
  if (calendarId.toLowerCase() === "primary") {
    throw new Error(
      `${benchmarkCalendarEnv} must identify a dedicated secondary calendar, not primary`,
    );
  }
  return calendarId;
}

export const runProcess: ProcessRunner = (file, args, cwd) => new Promise((resolve, reject) => {
  const child = spawn(file, [...args], { cwd, shell: false, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
  child.once("error", reject);
  child.once("close", (code) => resolve({ code: code ?? 1, stdout, stderr }));
});

export class GwsClient {
  constructor(
    private readonly runner: ProcessRunner = runProcess,
    private readonly binary = process.env.GWS_BIN ?? "gws",
    private readonly sleep: (milliseconds: number) => Promise<void> = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
  ) {}

  async call(path: readonly string[], params?: Json, body?: Json): Promise<Json> {
    const args = [...path];
    if (params !== undefined) args.push("--params", JSON.stringify(params));
    if (body !== undefined) args.push("--json", JSON.stringify(body));
    args.push("--format", "json");
    let result: ProcessResult | undefined;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      result = await this.runner(this.binary, args);
      if (result.code === 0) break;
      if (!isTransientGwsFailure(result) || attempt === 3) {
        throw new Error(`gws ${path.join(" ")} failed (${result.code}): ${result.stderr || result.stdout}`);
      }
      await this.sleep(500 * 2 ** (attempt - 1));
    }
    if (!result || result.code !== 0) throw new Error(`gws ${path.join(" ")} failed without a process result`);
    if (!result.stdout.trim()) return null;
    try {
      return JSON.parse(result.stdout) as Json;
    } catch {
      throw new Error(`gws ${path.join(" ")} returned non-JSON output: ${result.stdout.slice(0, 500)}`);
    }
  }

  async version(): Promise<string> {
    const result = await this.runner(this.binary, ["--version"]);
    if (result.code !== 0) throw new Error(`gws --version failed: ${result.stderr}`);
    return result.stdout.split(/\r?\n/u).map((line) => line.trim()).find(Boolean) ?? "";
  }

  /** A namespaced spreadsheet that is immediately trashed; only used by explicit preflight. */
  async canary(runTag: string): Promise<void> {
    const created = await this.call(["sheets", "spreadsheets", "create"], undefined, {
      properties: { title: `${runTag} preflight canary` },
      sheets: [{ properties: { title: "Source" } }],
    });
    const id = stringField(created, "spreadsheetId");
    await this.call(["drive", "files", "update"], { fileId: id }, { trashed: true });
  }
}

function isTransientGwsFailure(result: ProcessResult): boolean {
  return /(?:service is currently unavailable|backend error|internal error|rate limit|too many requests|\b429\b|\b50[0234]\b|timed? out|timeout|connection reset|temporarily unavailable)/iu.test(`${result.stderr}\n${result.stdout}`);
}

export class WorkspaceScenarioDriver {
  constructor(
    private readonly gws: GwsClient,
    private readonly sleep: (milliseconds: number) => Promise<void> =
      (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
    private readonly calendarId = configuredBenchmarkCalendarId(),
  ) {}

  async verifyCalendar(): Promise<{ id: string; summary: string }> {
    const calendarId = this.requireCalendarId();
    const entry = await this.gws.call(
      ["calendar", "calendarList", "get"],
      { calendarId },
    );
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      throw new Error(
        `${benchmarkCalendarEnv}=${calendarId} returned an invalid calendarList entry`,
      );
    }
    if (entry.primary === true) {
      throw new Error(
        `${benchmarkCalendarEnv} must identify a dedicated secondary calendar, not the account primary calendar`,
      );
    }
    const accessRole = objectString(entry, "accessRole");
    if (accessRole !== "owner" && accessRole !== "writer") {
      throw new Error(
        `${benchmarkCalendarEnv}=${calendarId} must grant writer or owner access; found ${accessRole || "no access role"}`,
      );
    }
    return { id: calendarId, summary: objectString(entry, "summary") };
  }

  /** Provision a namespaced, synthetic scenario directly in the configured Workspace account. */
  async provision(scenario: Scenario): Promise<Scenario> {
    const task = taskById(scenario.taskId);
    const parameters = { ...scenario.parameters };
    const resources: RegisteredResource[] = [];
    try {
      for (let index = 0; index < task.resources.length; index += 1) {
        const blueprint = task.resources[index]!;
        const fixture = scenario.fixtures[index]!;
        const created = await this.createFixture(fixture, scenario.runTag);
        resources.push({ ...created, role: blueprint.role });
        if (blueprint.parameter) parameters[blueprint.parameter] = created.id;
      }
    } catch (error) {
      await this.cleanup(resources).catch(() => undefined);
      if (task.requiredServices.includes("calendar")) {
        await this.cleanupCalendarEvents(scenario.runTag).catch(() => undefined);
      }
      throw error;
    }
    return { ...scenario, parameters, resources };
  }

  async snapshot(
    scenario: Scenario,
    options: { settleTaggedOutputs?: boolean } = {},
  ): Promise<WorkspaceSnapshot> {
    const resources: Record<string, Json> = {};
    const drafts: Json[] = [];
    const calendarEvents: Json[] = [];
    const task = taskById(scenario.taskId);
    for (const resource of scenario.resources) {
      if (resource.id.startsWith("pending-")) throw new Error(`scenario ${scenario.id} is not provisioned`);
      if (resource.service === "sheets") {
        resources[resource.id] = await this.gws.call(["sheets", "spreadsheets", "get"], { spreadsheetId: resource.id, includeGridData: true });
      } else if (resource.service === "docs") {
        resources[resource.id] = await this.gws.call(["docs", "documents", "get"], { documentId: resource.id });
      } else if (resource.service === "slides") {
        resources[resource.id] = await this.gws.call(["slides", "presentations", "get"], { presentationId: resource.id });
      } else if (resource.service === "calendar") {
        const events = await this.gws.call(["calendar", "events", "list"], {
          calendarId: resource.id,
          q: scenario.runTag,
          singleEvents: false,
          showDeleted: false,
        });
        resources[resource.id] = events;
        calendarEvents.push(events);
      } else if (resource.service === "gmail") {
        resources[resource.id] = await this.gws.call(["gmail", "users", "messages", "get"], { userId: "me", id: resource.id, format: "full" });
      } else {
        resources[resource.id] = await this.gws.call(["drive", "files", "get"], { fileId: resource.id });
      }
    }
    // Query by run tag so unrelated account state never becomes grading evidence.
    const listed = await this.taggedResults(
      () => this.gws.call(
        ["gmail", "users", "drafts", "list"],
        { userId: "me", q: `"${scenario.runTag}"` },
      ),
      "drafts",
      options.settleTaggedOutputs === true ? 1 : 0,
    );
    resources[`__drafts_${scenario.id}`] = listed;
    for (const draftId of draftIds(listed)) {
      drafts.push(await this.gws.call(["gmail", "users", "drafts", "get"], { userId: "me", id: draftId, format: "full" }));
    }
    resources[`__sent_${scenario.id}`] = await this.gws.call(["gmail", "users", "messages", "list"], { userId: "me", q: `label:SENT "${scenario.runTag}"` });
    if (scenario.taskId === "support_inbox_triage") {
      resources[`__labels_${scenario.id}`] = await this.gws.call(["gmail", "users", "labels", "list"], { userId: "me" });
    }
    const taggedDrive = await this.taggedResults(
      () => this.gws.call(
        ["drive", "files", "list"],
        {
          q: driveTagQuery(scenario.runTag),
          fields: "files(id,name,mimeType,trashed,description)",
        },
      ),
      "files",
      options.settleTaggedOutputs === true &&
          task.requiredServices.includes("drive")
        ? scenario.resources.filter(isDriveBackedResource).length + 1
        : 0,
    );
    resources[`__drive_${scenario.id}`] = taggedDrive;
    for (const file of objectsFrom(taggedDrive, "files")) {
      if (typeof file.id !== "string" || typeof file.mimeType !== "string") continue;
      if (file.mimeType === "application/vnd.google-apps.document") {
        resources[`__drive_file_${file.id}`] = await this.gws.call(["docs", "documents", "get"], { documentId: file.id });
      } else if (file.mimeType === "application/vnd.google-apps.presentation") {
        resources[`__drive_file_${file.id}`] = await this.gws.call(["slides", "presentations", "get"], { presentationId: file.id });
      }
    }
    return { capturedAt: new Date().toISOString(), resources, drafts, calendarEvents };
  }

  private async taggedResults(
    load: () => Promise<Json>,
    resultKey: string,
    minimumResults: number,
  ): Promise<Json> {
    const attempts = minimumResults > 0 ? 8 : 1;
    let result: Json = null;
    for (let attempt = 1; attempt <= attempts; attempt += 1) {
      result = await load();
      if (
        idsFrom(result, resultKey).length >= minimumResults ||
        attempt === attempts
      ) {
        return result;
      }
      await this.sleep(500);
    }
    return result;
  }

  async cleanup(resources: readonly RegisteredResource[]): Promise<void> {
    const failures: string[] = [];
    for (const resource of [...resources].reverse()) {
      if (!resource.createdByBenchmark) continue;
      try {
        if (resource.service === "calendar") {
          // The configured calendar is durable benchmark infrastructure. Keep
          // it even if an old cleanup registry incorrectly marks it disposable.
          if (resource.id === this.calendarId) continue;
          await this.gws.call(["calendar", "calendars", "delete"], { calendarId: resource.id });
        } else if (resource.service === "gmail") {
          // `gmail.modify` permits trashing but not permanent deletion. The
          // benchmark asks for that narrower scope so cleanup uses Trash.
          await this.gws.call(["gmail", "users", "messages", "trash"], { userId: "me", id: resource.id });
        } else {
          await this.gws.call(["drive", "files", "update"], { fileId: resource.id }, { trashed: true });
        }
      } catch (error) {
        failures.push(`${resource.role} (${resource.id}): ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    if (failures.length > 0) throw new Error(`cleanup failed:\n${failures.join("\n")}`);
  }

  /** Delete every tagged output the benchmark can discover, then leave source cleanup to the registry. */
  async cleanupTagged(runTag: string): Promise<void> {
    const failures: string[] = [];
    try {
      const files = await this.gws.call(
        ["drive", "files", "list"],
        { q: driveTagQuery(runTag), fields: "files(id)" },
      );
      for (const id of idsFrom(files, "files")) await this.gws.call(["drive", "files", "update"], { fileId: id }, { trashed: true });
    } catch (error) { failures.push(`Drive tag cleanup: ${message(error)}`); }
    try {
      const drafts = await this.gws.call(["gmail", "users", "drafts", "list"], { userId: "me", q: `"${runTag}"` });
      for (const id of idsFrom(drafts, "drafts")) await this.gws.call(["gmail", "users", "drafts", "delete"], { userId: "me", id });
      const messages = await this.gws.call(["gmail", "users", "messages", "list"], { userId: "me", q: `"${runTag}"` });
      for (const id of idsFrom(messages, "messages")) await this.gws.call(["gmail", "users", "messages", "trash"], { userId: "me", id });
    } catch (error) { failures.push(`Gmail tag cleanup: ${message(error)}`); }
    try {
      const labels = await this.gws.call(["gmail", "users", "labels", "list"], { userId: "me" });
      for (const label of objectsFrom(labels, "labels")) {
        if (typeof label.id === "string" && typeof label.name === "string" && label.name.includes(runTag)) {
          await this.gws.call(["gmail", "users", "labels", "delete"], { userId: "me", id: label.id });
        }
      }
    } catch (error) { failures.push(`Gmail label cleanup: ${message(error)}`); }
    if (this.calendarId) {
      try {
        await this.cleanupCalendarEvents(runTag);
      } catch (error) {
        failures.push(`benchmark Calendar tag cleanup: ${message(error)}`);
      }
    }
    if (failures.length > 0) throw new Error(`tag cleanup failed:\n${failures.join("\n")}`);
  }

  private async createFixture(fixture: ScenarioFixture, runTag: string): Promise<RegisteredResource> {
    if (fixture.service === "sheets") {
      const created = await this.gws.call(["sheets", "spreadsheets", "create"], undefined, {
        properties: { title: fixture.title },
        sheets: [{ properties: { title: "Source" } }],
      });
      const id = stringField(created, "spreadsheetId");
      try {
        if (fixture.table) {
          await this.gws.call(
            ["sheets", "spreadsheets", "values", "update"],
            { spreadsheetId: id, range: "Source!A1", valueInputOption: "RAW" },
            { values: fixture.table },
          );
        }
      } catch (error) {
        await this.gws.call(["drive", "files", "update"], { fileId: id }, { trashed: true }).catch(() => undefined);
        throw error;
      }
      return { id, role: fixture.role, service: fixture.service, createdByBenchmark: true };
    }
    if (fixture.service === "docs") {
      const created = await this.gws.call(["docs", "documents", "create"], undefined, { title: fixture.title });
      const id = stringField(created, "documentId");
      if (fixture.text) {
        await this.gws.call(["docs", "documents", "batchUpdate"], { documentId: id }, {
          requests: [{ insertText: { location: { index: 1 }, text: `${fixture.text}\nTag: ${runTag}\n` } }],
        });
      }
      return { id, role: fixture.role, service: fixture.service, createdByBenchmark: true };
    }
    if (fixture.service === "slides") {
      const created = await this.gws.call(["slides", "presentations", "create"], undefined, { title: fixture.title });
      return { id: stringField(created, "presentationId"), role: fixture.role, service: fixture.service, createdByBenchmark: true };
    }
    if (fixture.service === "calendar") {
      const id = this.requireCalendarId();
      for (const event of fixture.events ?? []) {
        await this.gws.call(["calendar", "events", "insert"], { calendarId: id, sendUpdates: "none" }, event);
      }
      return { id, role: fixture.role, service: fixture.service, createdByBenchmark: false };
    }
    if (fixture.service === "gmail") {
      const message = fixture.messages?.[0] ?? { subject: `[${runTag}] benchmark message`, body: "synthetic" };
      const subject = objectString(message, "subject");
      const body = objectString(message, "body");
      const raw = base64Url([
        "From: benchmark@example.test",
        "To: benchmark@example.test",
        "Date: Mon, 13 Jul 2026 08:00:00 +0000",
        `Subject: ${subject}`,
        "",
        body,
        runTag,
      ].join("\r\n"));
      const inserted = await this.gws.call(["gmail", "users", "messages", "insert"], {
        userId: "me",
        internalDateSource: "dateHeader",
      }, {
        raw,
        labelIds: ["INBOX", "UNREAD"],
      });
      const id = stringField(inserted, "id");
      try {
        await this.gws.call(["gmail", "users", "messages", "modify"], { userId: "me", id }, {
          addLabelIds: ["INBOX", "UNREAD"],
        });
        await this.waitForUnreadMessage(id, runTag);
      } catch (error) {
        await this.gws.call(["gmail", "users", "messages", "trash"], { userId: "me", id }).catch(() => undefined);
        throw error;
      }
      return { id, role: fixture.role, service: fixture.service, createdByBenchmark: true };
    }
    throw new Error(`unsupported fixture service: ${fixture.service}`);
  }

  private async waitForUnreadMessage(id: string, runTag: string): Promise<void> {
    const query = `label:inbox is:unread "${runTag}"`;
    let consecutiveReadyChecks = 0;
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const [message, listed] = await Promise.all([
        this.gws.call(["gmail", "users", "messages", "get"], { userId: "me", id, format: "minimal" }),
        this.gws.call(["gmail", "users", "messages", "list"], { userId: "me", q: query, maxResults: 10 }),
      ]);
      if (gmailFixtureReady(message, listed, id)) {
        consecutiveReadyChecks += 1;
        if (consecutiveReadyChecks >= 3) return;
      } else {
        consecutiveReadyChecks = 0;
        await this.gws.call(["gmail", "users", "messages", "modify"], { userId: "me", id }, {
          addLabelIds: ["INBOX", "UNREAD"],
        });
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw new Error(`Gmail fixture ${id} never became stably queryable as unread for ${runTag}`);
  }

  private requireCalendarId(): string {
    if (!this.calendarId) return requireBenchmarkCalendarId();
    if (this.calendarId.toLowerCase() === "primary") {
      throw new Error(
        `${benchmarkCalendarEnv} must identify a dedicated secondary calendar, not primary`,
      );
    }
    return this.calendarId;
  }

  private async cleanupCalendarEvents(runTag: string): Promise<void> {
    if (!this.calendarId) return;
    const events = await this.gws.call(["calendar", "events", "list"], {
      calendarId: this.calendarId,
      q: runTag,
      singleEvents: false,
      showDeleted: false,
    });
    for (const id of idsFrom(events, "items")) {
      await this.gws.call(["calendar", "events", "delete"], {
        calendarId: this.calendarId,
        eventId: id,
        sendUpdates: "none",
      });
    }
  }
}

function driveTagQuery(runTag: string): string {
  return `trashed = false and (name contains '${runTag}' or fullText contains '${runTag}')`;
}

function isDriveBackedResource(resource: RegisteredResource): boolean {
  return resource.service !== "calendar" && resource.service !== "gmail";
}

export function gmailFixtureReady(message: Json, listed: Json, id: string): boolean {
  if (!message || typeof message !== "object" || Array.isArray(message) || !Array.isArray(message.labelIds)) return false;
  const labels = message.labelIds.filter((label): label is string => typeof label === "string");
  return labels.includes("INBOX") && labels.includes("UNREAD") && idsFrom(listed, "messages").includes(id);
}

function stringField(value: Json, field: string): string {
  if (value && typeof value === "object" && !Array.isArray(value) && typeof value[field] === "string") return value[field] as string;
  throw new Error(`gws response missing string field ${field}`);
}

function objectString(value: Json, field: string): string {
  return value && typeof value === "object" && !Array.isArray(value) && typeof value[field] === "string" ? value[field] as string : "";
}

function base64Url(value: string): string {
  return Buffer.from(value, "utf8").toString("base64").replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}

function draftIds(value: Json): readonly string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const drafts = value.drafts;
  if (!Array.isArray(drafts)) return [];
  return drafts.flatMap((draft) => draft && typeof draft === "object" && !Array.isArray(draft) && typeof draft.id === "string" ? [draft.id] : []);
}

function idsFrom(value: Json, key: string): readonly string[] {
  return objectsFrom(value, key).flatMap((entry) => typeof entry.id === "string" ? [entry.id] : []);
}

function objectsFrom(value: Json, key: string): readonly Record<string, Json>[] {
  if (!value || typeof value !== "object" || Array.isArray(value) || !Array.isArray(value[key])) return [];
  return value[key].flatMap((entry) => entry && typeof entry === "object" && !Array.isArray(entry) ? [entry] : []);
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

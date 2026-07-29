import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  accessSync,
  chmodSync,
  constants,
  existsSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, isAbsolute, join, resolve } from "node:path";

import { taskById } from "./tasks.js";
import type { Json, RegisteredResource, Scenario, ScenarioFixture, WorkspaceSnapshot } from "./types.js";

export interface ProcessResult {
  code: number;
  stdout: string;
  stderr: string;
  timedOut?: boolean;
}

export type ProcessRunner = (
  file: string,
  args: readonly string[],
  cwd?: string,
  environment?: NodeJS.ProcessEnv,
) => Promise<ProcessResult>;

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

export const runProcess: ProcessRunner = (file, args, cwd, environment) => new Promise((resolve, reject) => {
  const child = spawn(file, [...args], {
    cwd,
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
    detached: process.platform !== "win32",
    env: environment,
  });
  let stdout = "";
  let stderr = "";
  let timedOut = false;
  const timeoutMs = phaseProcessTimeoutMs();
  const deadline = setTimeout(() => {
    timedOut = true;
    stderr += `\nprocess timed out after ${timeoutMs}ms\n`;
    terminateProcessTree(child, "SIGTERM");
    setTimeout(() => terminateProcessTree(child, "SIGKILL"), 2_000).unref();
  }, timeoutMs);
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
  child.once("error", reject);
  child.once("close", (code) => {
    clearTimeout(deadline);
    resolve({ code: timedOut ? 124 : code ?? 1, stdout, stderr, timedOut });
  });
});

function terminateProcessTree(
  child: ReturnType<typeof spawn>,
  signal: NodeJS.Signals,
): void {
  try {
    if (process.platform !== "win32" && child.pid) {
      process.kill(-child.pid, signal);
    } else {
      child.kill(signal);
    }
  } catch {
    // The process may already be gone.
  }
}

function phaseProcessTimeoutMs(): number {
  const configured = Number(process.env.CORI_BENCH_PROCESS_TIMEOUT_MS ?? "300000");
  return Number.isSafeInteger(configured) && configured > 0
    ? configured
    : 300_000;
}

interface GwsAuditProxy {
  binary: string;
  logPath: string;
  realBinary: string;
}

export interface GwsAuditEvent {
  argv: string[];
  cwd: string;
  at: string;
  pid: number;
}

let installedAuditProxy: GwsAuditProxy | undefined;

export class GwsClient {
  private readonly runner: ProcessRunner;
  private readonly binary: string;
  private readonly sleep: (milliseconds: number) => Promise<void>;
  private readonly auditProxy: GwsAuditProxy | undefined;

  constructor(
    runner: ProcessRunner = runProcess,
    binary = process.env.GWS_BIN ?? "gws",
    sleep: (milliseconds: number) => Promise<void> = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
  ) {
    this.runner = runner;
    this.sleep = sleep;
    if (runner === runProcess) {
      this.auditProxy = installGwsAuditProxy(binary);
      this.binary = this.auditProxy.binary;
    } else {
      this.binary = binary;
    }
  }

  async call(path: readonly string[], params?: Json, body?: Json): Promise<Json> {
    const args = [...path];
    if (params !== undefined) args.push("--params", JSON.stringify(params));
    if (body !== undefined) args.push("--json", JSON.stringify(body));
    args.push("--format", "json");
    let result: ProcessResult | undefined;
    const maxAttempts = isRetrySafeGwsCall(path) ? 3 : 1;
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      result = await this.runner(this.binary, args);
      if (result.code === 0) break;
      if (!isTransientGwsFailure(result) || attempt === maxAttempts) {
        throw new Error(gwsFailureMessage(path, result));
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

  /** Absolute underlying executable, excluding the benchmark audit proxy. */
  underlyingBinary(): string {
    return this.auditProxy?.realBinary ?? this.binary;
  }

  /**
   * Commands observed by the benchmark-owned PATH proxy. The proxy covers both
   * direct harness calls and replayed `cori_cli` steps, so request-only safety
   * fields (for example Calendar `sendUpdates`) remain auditable after the API
   * response has discarded them.
   */
  auditEvidence(): { complete: boolean; events: GwsAuditEvent[] } {
    if (!this.auditProxy) return { complete: false, events: [] };
    if (!existsSync(this.auditProxy.logPath)) return { complete: true, events: [] };
    return parseGwsAuditLog(readFileSync(this.auditProxy.logPath, "utf8"));
  }

  /**
   * Start a bounded scenario audit window after fixture provisioning. Keeping
   * one window per trial avoids copying the process-lifetime command history
   * into every snapshot while preserving fail-closed parsing of prior writes.
   */
  beginAuditWindow(): { complete: boolean; events: GwsAuditEvent[] } {
    if (!this.auditProxy) return { complete: false, events: [] };
    const previous = this.auditEvidence();
    const marker: GwsAuditEvent = {
      argv: ["__cori_benchmark_audit_window__", randomUUID()],
      cwd: process.cwd(),
      at: new Date().toISOString(),
      pid: process.pid,
    };
    writeFileSync(
      this.auditProxy.logPath,
      `${JSON.stringify(marker)}\n`,
      "utf8",
    );
    return { complete: previous.complete, events: [marker] };
  }

  /**
   * Read-only token refresh probe used before provisioning any fixture.
   * Returns a non-PII stable identity so batches cannot mix accounts.
   */
  async verifyAuthentication(): Promise<string> {
    const about = await this.call(
      ["drive", "about", "get"],
      { fields: "user(permissionId,emailAddress)" },
    );
    const user = objectValue(about, "user");
    const stableId = user
      ? objectString(user, "permissionId") || objectString(user, "emailAddress")
      : "";
    if (!stableId) {
      throw new Error(
        "gws drive about get did not return user.permissionId or user.emailAddress",
      );
    }
    return createHash("sha256").update(stableId).digest("hex");
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

export function parseGwsAuditLog(
  contents: string,
): { complete: boolean; events: GwsAuditEvent[] } {
  const events: GwsAuditEvent[] = [];
  let complete = true;
  for (const line of contents.split(/\r?\n/u)) {
    if (!line.trim()) continue;
    try {
      const value = JSON.parse(line) as Partial<GwsAuditEvent>;
      if (
        Array.isArray(value.argv) &&
        value.argv.every((arg) => typeof arg === "string") &&
        typeof value.cwd === "string" &&
        typeof value.at === "string" &&
        typeof value.pid === "number"
      ) {
        events.push(value as GwsAuditEvent);
      } else {
        complete = false;
      }
    } catch {
      complete = false;
    }
  }
  return { complete, events };
}

function isRetrySafeGwsCall(path: readonly string[]): boolean {
  if (path[0]?.toLowerCase() === "schema") return true;
  const method = path.at(-1)?.toLowerCase();
  return method === "get" || method === "list";
}

function expectedTaggedDriveOutputs(taskId: string): number {
  return [
    "sla_breach_pack",
    "expense_policy_audit",
    "budget_variance_deck",
    "weekly_operating_review",
  ].includes(taskId)
    ? 1
    : 0;
}

function installGwsAuditProxy(configuredBinary: string): GwsAuditProxy {
  if (installedAuditProxy) return installedAuditProxy;
  const realBinary = resolveExecutable(configuredBinary);
  const proxyDir = mkdtempSync(join(tmpdir(), "cori-bench-gws-audit-"));
  const logPath = join(proxyDir, "commands.jsonl");
  const modulePath = join(proxyDir, "proxy.mjs");
  writeFileSync(modulePath, [
    'import { appendFileSync } from "node:fs";',
    'import { spawnSync } from "node:child_process";',
    "const argv = process.argv.slice(2);",
    `const logPath = ${JSON.stringify(logPath)};`,
    `const realBinary = ${JSON.stringify(realBinary)};`,
    "try {",
    "  appendFileSync(logPath, JSON.stringify({ argv, cwd: process.cwd(), at: new Date().toISOString(), pid: process.pid }) + \"\\n\", \"utf8\");",
    "} catch (error) {",
    '  process.stderr.write(`benchmark GWS audit append failed: ${String(error)}\\n`);',
    "  process.exit(126);",
    "}",
    "const result = spawnSync(realBinary, argv, { env: process.env, stdio: \"inherit\" });",
    "if (result.error) {",
    '  process.stderr.write(`benchmark GWS proxy failed: ${String(result.error)}\\n`);',
    "  process.exit(126);",
    "}",
    "process.exit(result.status ?? 1);",
    "",
  ].join("\n"), "utf8");

  let binary: string;
  if (process.platform === "win32") {
    binary = join(proxyDir, "gws.cmd");
    writeFileSync(
      binary,
      `@echo off\r\n"${process.execPath}" "${modulePath}" %*\r\n`,
      "utf8",
    );
  } else {
    binary = join(proxyDir, "gws");
    writeFileSync(
      binary,
      `#!/bin/sh\nexec "${process.execPath}" "${modulePath}" "$@"\n`,
      "utf8",
    );
    chmodSync(binary, 0o700);
  }

  const pathEntries = (process.env.PATH ?? "").split(delimiter).filter(Boolean);
  process.env.PATH = [
    proxyDir,
    ...pathEntries.filter((entry) => resolve(entry) !== resolve(proxyDir)),
  ].join(delimiter);
  installedAuditProxy = { binary, logPath, realBinary };
  return installedAuditProxy;
}

function resolveExecutable(binary: string): string {
  if (isAbsolute(binary) || binary.includes("/") || binary.includes("\\")) {
    const absolute = resolve(binary);
    accessSync(absolute, constants.X_OK);
    return absolute;
  }
  const extensions = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT").split(";")
    : [""];
  for (const directory of (process.env.PATH ?? "").split(delimiter).filter(Boolean)) {
    for (const extension of extensions) {
      const candidate = join(directory, process.platform === "win32" ? `${binary}${extension}` : binary);
      try {
        accessSync(candidate, constants.X_OK);
        return resolve(candidate);
      } catch {
        // Keep looking along PATH.
      }
    }
  }
  throw new Error(`cannot find GWS executable \`${binary}\` on PATH`);
}

function jsonAuditEvidence(gws: GwsClient, beginWindow = false): Json {
  const evidence = beginWindow ? gws.beginAuditWindow() : gws.auditEvidence();
  return {
    complete: evidence.complete,
    events: evidence.events.map((event) => ({
      argv: [...event.argv],
      cwd: event.cwd,
      at: event.at,
      pid: event.pid,
    })),
  };
}

function gwsFailureMessage(
  path: readonly string[],
  result: ProcessResult,
): string {
  const diagnostic = (result.stderr || result.stdout).trim();
  const prefix = `gws ${path.join(" ")} failed (${result.code})`;
  if (/(?:invalid_rapt|reauth related error)/iu.test(diagnostic)) {
    return [
      `${prefix}: Google Workspace rejected the cached OAuth session and requires reauthentication (invalid_rapt).`,
      "Run `gws auth login --services drive,gmail,sheets,docs,calendar,slides` in an interactive terminal, then rerun the benchmark.",
      `Original diagnostic: ${diagnostic}`,
    ].join("\n");
  }
  return `${prefix}: ${diagnostic}`;
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
      for (let index = 0; index < scenario.fixtures.length; index += 1) {
        const blueprint = task.resources[index];
        const fixture = scenario.fixtures[index]!;
        // One fixture can provision several live resources: an inbox holds a
        // day's messages, an invoice folder holds a week's documents.
        const created = await this.createFixtures(fixture, scenario.runTag);
        for (const resource of created) {
          resources.push({ ...resource, role: blueprint?.role ?? fixture.role, fixtureIndex: index });
        }
        if (blueprint?.parameter && created[0]) {
          parameters[blueprint.parameter] = created[0].id;
        }
      }
      const unreadIds = scenario.fixtures.flatMap((fixture, index) => {
        if (fixture.service !== "gmail") return [];
        const provisioned = resources.filter((resource) => resource.fixtureIndex === index);
        return (fixture.messages ?? []).flatMap((message, ordinal) =>
          messageIsUnread(message) && provisioned[ordinal]
            ? [provisioned[ordinal]!.id]
            : []
        );
      });
      if (unreadIds.length > 0) {
        await this.waitForUnreadMessages(unreadIds, scenario.runTag);
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
        resources[resource.id] = await this.gws.call(["sheets", "spreadsheets", "get"], {
          spreadsheetId: resource.id,
          includeGridData: true,
          fields:
            "sheets(properties(title),data(rowData(values(formattedValue,effectiveValue))))",
        });
      } else if (resource.service === "docs") {
        resources[resource.id] = await this.gws.call(["docs", "documents", "get"], {
          documentId: resource.id,
          fields: "documentId,title,body/content",
        });
      } else if (resource.service === "slides") {
        resources[resource.id] = await this.gws.call(["slides", "presentations", "get"], {
          presentationId: resource.id,
          fields:
            "presentationId,title,slides(objectId,pageElements(shape/text/textElements/textRun/content))",
        });
      } else if (resource.service === "calendar") {
        const events = await this.gws.call(["calendar", "events", "list"], {
          calendarId: resource.id,
          q: scenario.runTag,
          singleEvents: false,
          showDeleted: false,
          fields:
            "items(id,summary,description,eventType,start,end,htmlLink,attendees)",
        });
        resources[resource.id] = events;
        calendarEvents.push(events);
      } else if (resource.service === "gmail") {
        resources[resource.id] = await this.gws.call(["gmail", "users", "messages", "get"], {
          userId: "me",
          id: resource.id,
          format: "full",
          fields: "id,labelIds,internalDate,payload,snippet",
        });
      } else {
        resources[resource.id] = await this.gws.call(["drive", "files", "get"], { fileId: resource.id });
      }
    }
    // Query by run tag so unrelated account state never becomes grading evidence.
    const listed = await this.taggedResults(
      () => this.gws.call(
        ["gmail", "users", "drafts", "list"],
        { userId: "me", q: `"${scenario.runTag}"`, fields: "drafts(id,message(id,threadId))" },
      ),
      "drafts",
      options.settleTaggedOutputs === true ? 1 : 0,
    );
    resources[`__drafts_${scenario.id}`] = listed;
    for (const draftId of draftIds(listed)) {
      drafts.push(await this.gws.call(["gmail", "users", "drafts", "get"], {
        userId: "me",
        id: draftId,
        format: "full",
        fields: "id,message(id,labelIds,payload,snippet,threadId)",
      }));
    }
    resources[`__sent_${scenario.id}`] = await this.gws.call(["gmail", "users", "messages", "list"], {
      userId: "me",
      q: `label:SENT "${scenario.runTag}"`,
      fields: "messages(id,threadId)",
    });
    if (task.requiredServices.includes("gmail")) {
      resources[`__labels_${scenario.id}`] = await this.gws.call(["gmail", "users", "labels", "list"], {
        userId: "me",
        fields: "labels(id,name,type)",
      });
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
        ? scenario.resources.filter(isDriveBackedResource).length +
          expectedTaggedDriveOutputs(scenario.taskId)
        : 0,
    );
    resources[`__drive_${scenario.id}`] = taggedDrive;
    for (const file of objectsFrom(taggedDrive, "files")) {
      if (typeof file.id !== "string" || typeof file.mimeType !== "string") continue;
      if (file.mimeType === "application/vnd.google-apps.document") {
        resources[`__drive_file_${file.id}`] = await this.gws.call(["docs", "documents", "get"], {
          documentId: file.id,
          fields: "documentId,title,body/content",
        });
      } else if (file.mimeType === "application/vnd.google-apps.presentation") {
        resources[`__drive_file_${file.id}`] = await this.gws.call(["slides", "presentations", "get"], {
          presentationId: file.id,
          fields:
            "presentationId,title,slides(objectId,pageElements(shape/text/textElements/textRun/content))",
        });
      }
    }
    resources[`__gws_audit_${scenario.id}`] = jsonAuditEvidence(this.gws);
    return {
      capturedAt: new Date().toISOString(),
      source: "workspace",
      resources,
      drafts,
      calendarEvents,
    };
  }

  /** Canonical state of a fixture immediately after provisioning, with no API reads. */
  baselineSnapshot(scenario: Scenario): WorkspaceSnapshot {
    const resources: Record<string, Json> = {};
    const calendarEvents: Json[] = [];
    const task = taskById(scenario.taskId);
    const ordinals = new Map<number, number>();
    scenario.resources.forEach((resource, index) => {
      const fixtureIndex = resource.fixtureIndex ?? index;
      const fixture = scenario.fixtures[fixtureIndex];
      if (!fixture) return;
      const ordinal = ordinals.get(fixtureIndex) ?? 0;
      ordinals.set(fixtureIndex, ordinal + 1);
      if (resource.service === "sheets") {
        resources[resource.id] = gridFixture(fixture.table ?? []);
      } else if (resource.service === "gmail") {
        resources[resource.id] = {
          id: resource.id,
          labelIds: [...(resource.initialLabelIds ?? ["INBOX"])],
        };
      } else if (resource.service === "calendar") {
        const events = { items: fixture.events ?? [] };
        resources[resource.id] = events;
        calendarEvents.push(events);
      } else if (resource.service === "docs") {
        const document = fixture.documents?.[ordinal];
        resources[resource.id] = {
          documentId: resource.id,
          title: document?.title ?? fixture.title,
          text: document?.text ?? fixture.text ?? "",
        };
      } else if (resource.service === "slides") {
        resources[resource.id] = { presentationId: resource.id, title: fixture.title, slides: [] };
      } else {
        resources[resource.id] = { id: resource.id, name: fixture.title };
      }
    });
    resources[`__drafts_${scenario.id}`] = {};
    resources[`__sent_${scenario.id}`] = {};
    resources[`__drive_${scenario.id}`] = { files: [] };
    if (task.requiredServices.includes("gmail")) {
      resources[`__labels_${scenario.id}`] = { labels: [] };
    }
    resources[`__gws_audit_${scenario.id}`] = jsonAuditEvidence(
      this.gws,
      true,
    );
    return {
      capturedAt: new Date().toISOString(),
      source: "canonical_fixture",
      resources,
      drafts: [],
      calendarEvents,
    };
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

  private async createFixtures(fixture: ScenarioFixture, runTag: string): Promise<RegisteredResource[]> {
    if (fixture.service === "docs") {
      const documents = fixture.documents ??
        [{ title: fixture.title, text: fixture.text ?? "" }];
      const created: RegisteredResource[] = [];
      for (const document of documents) {
        const response = await this.gws.call(["docs", "documents", "create"], undefined, {
          title: document.title,
        });
        const id = stringField(response, "documentId");
        if (document.text) {
          await this.gws.call(["docs", "documents", "batchUpdate"], { documentId: id }, {
            requests: [{
              insertText: { location: { index: 1 }, text: `${document.text}\nTag: ${runTag}\n` },
            }],
          });
        }
        created.push({
          id,
          role: fixture.role,
          service: fixture.service,
          createdByBenchmark: true,
        });
      }
      return created;
    }
    if (fixture.service === "gmail") {
      const messages = fixture.messages ??
        [{ subject: `[${runTag}] benchmark message`, body: "synthetic" }];
      const created: RegisteredResource[] = [];
      let triagedLabelId: string | undefined;
      for (const message of messages) {
        const unread = messageIsUnread(message);
        const id = await this.insertMessage(message, runTag, unread);
        if (!unread) {
          // State left by a simulated previous run. The workflow must leave
          // these alone, so they carry the completion label a prior run applied.
          triagedLabelId ??= await this.ensureLabel(`${runTag}/triaged`);
          await this.gws.call(["gmail", "users", "messages", "modify"], { userId: "me", id }, {
            addLabelIds: [triagedLabelId],
            removeLabelIds: ["UNREAD"],
          });
        }
        created.push({
          id,
          role: fixture.role,
          service: fixture.service,
          createdByBenchmark: true,
          initialLabelIds: unread
            ? ["INBOX", "UNREAD"]
            : ["INBOX", triagedLabelId!],
        });
      }
      return created;
    }
    return [await this.createSingleFixture(fixture, runTag)];
  }

  private async insertMessage(
    message: Json,
    runTag: string,
    unread: boolean,
  ): Promise<string> {
    const subject = objectString(message, "subject");
    const body = objectString(message, "body");
    const from = objectString(message, "from") || "benchmark@example.test";
    const date = objectString(message, "date");
    const parsed = date ? Date.parse(date) : Number.NaN;
    const header = Number.isFinite(parsed)
      ? new Date(parsed).toUTCString()
      : "Mon, 13 Jul 2026 08:00:00 GMT";
    const raw = base64Url([
      `From: ${from}`,
      "To: benchmark@example.test",
      `Date: ${header}`,
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
      labelIds: unread ? ["INBOX", "UNREAD"] : ["INBOX"],
    });
    const id = stringField(inserted, "id");
    try {
      await this.gws.call(["gmail", "users", "messages", "modify"], { userId: "me", id }, {
        addLabelIds: unread ? ["INBOX", "UNREAD"] : ["INBOX"],
      });
    } catch (error) {
      await this.gws.call(["gmail", "users", "messages", "trash"], { userId: "me", id })
        .catch(() => undefined);
      throw error;
    }
    return id;
  }

  private async ensureLabel(name: string): Promise<string> {
    const existing = await this.gws.call(["gmail", "users", "labels", "list"], {
      userId: "me",
      fields: "labels(id,name)",
    });
    for (const label of objectsFrom(existing, "labels")) {
      if (label.name === name && typeof label.id === "string") return label.id;
    }
    const created = await this.gws.call(["gmail", "users", "labels", "create"], { userId: "me" }, {
      name,
      labelListVisibility: "labelShow",
      messageListVisibility: "show",
    });
    return stringField(created, "id");
  }

  private async createSingleFixture(fixture: ScenarioFixture, runTag: string): Promise<RegisteredResource> {
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
            {
              spreadsheetId: id,
              range: fixtureWriteRange(fixture.table),
              valueInputOption: "RAW",
            },
            { values: fixture.table },
          );
        }
      } catch (error) {
        await this.gws.call(["drive", "files", "update"], { fileId: id }, { trashed: true }).catch(() => undefined);
        throw error;
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
    throw new Error(`unsupported fixture service: ${fixture.service}`);
  }

  private async waitForUnreadMessages(
    ids: readonly string[],
    runTag: string,
  ): Promise<void> {
    const query = `label:inbox is:unread "${runTag}"`;
    let consecutiveReadyChecks = 0;
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const listed = await this.gws.call(
        ["gmail", "users", "messages", "list"],
        { userId: "me", q: query, maxResults: Math.max(10, ids.length) },
      );
      const messages: Json[] = [];
      for (const id of ids) {
        messages.push(await this.gws.call(["gmail", "users", "messages", "get"], {
          userId: "me",
          id,
          format: "minimal",
        }));
      }
      if (
        ids.every((id, index) =>
          gmailFixtureReady(messages[index] ?? null, listed, id)
        )
      ) {
        consecutiveReadyChecks += 1;
        if (consecutiveReadyChecks >= 3) return;
      } else {
        consecutiveReadyChecks = 0;
        for (const id of ids) {
          await this.gws.call(["gmail", "users", "messages", "modify"], {
            userId: "me",
            id,
          }, { addLabelIds: ["INBOX", "UNREAD"] });
        }
      }
      await this.sleep(250);
    }
    throw new Error(
      `Gmail fixtures ${ids.join(", ")} never became stably queryable as unread for ${runTag}`,
    );
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

/** Messages left by a simulated previous run are provisioned already read. */
export function messageIsUnread(message: Json): boolean {
  if (!message || typeof message !== "object" || Array.isArray(message)) return true;
  if (typeof message.unread === "boolean") return message.unread;
  return message.pretriaged !== true;
}

export function gmailFixtureReady(message: Json, listed: Json, id: string): boolean {
  if (!message || typeof message !== "object" || Array.isArray(message) || !Array.isArray(message.labelIds)) return false;
  const labels = message.labelIds.filter((label): label is string => typeof label === "string");
  return labels.includes("INBOX") && labels.includes("UNREAD") && idsFrom(listed, "messages").includes(id);
}

export function fixtureWriteRange(table: readonly (readonly string[])[]): string {
  const rows = Math.max(1, table.length);
  const columns = Math.max(1, ...table.map((row) => row.length));
  return `Source!A1:${columnName(columns)}${rows}`;
}

function columnName(column: number): string {
  let remaining = column;
  let name = "";
  while (remaining > 0) {
    const digit = (remaining - 1) % 26;
    name = String.fromCharCode(65 + digit) + name;
    remaining = Math.floor((remaining - 1) / 26);
  }
  return name;
}

function gridFixture(table: readonly (readonly string[])[]): Json {
  return {
    sheets: [{
      data: [{
        rowData: table.map((row) => ({
          values: row.map((formattedValue) => ({ formattedValue })),
        })),
      }],
    }],
  };
}

function stringField(value: Json, field: string): string {
  if (value && typeof value === "object" && !Array.isArray(value) && typeof value[field] === "string") return value[field] as string;
  throw new Error(`gws response missing string field ${field}`);
}

function objectString(value: Json, field: string): string {
  return value && typeof value === "object" && !Array.isArray(value) && typeof value[field] === "string" ? value[field] as string : "";
}

function objectValue(value: Json, field: string): Record<string, Json> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const nested = value[field];
  return nested && typeof nested === "object" && !Array.isArray(nested)
    ? nested
    : null;
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

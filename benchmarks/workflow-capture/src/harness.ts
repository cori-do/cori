import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, existsSync } from "node:fs";
import { access, readFile } from "node:fs/promises";
import { delimiter, isAbsolute, join, resolve } from "node:path";

import type { HarnessName, HarnessSession, HarnessUsage, Json } from "./types.js";

export const DEFAULT_CODEX_MODEL = "gpt-5.6-terra";

export function codexModel(): string {
  return process.env.CORI_BENCH_CODEX_MODEL ?? DEFAULT_CODEX_MODEL;
}

export interface HarnessAdapter {
  readonly name: HarnessName;
  identity(): Promise<HarnessIdentity>;
  version(): Promise<string>;
  start(
    prompt: string,
    cwd: string,
    options?: HarnessExecutionOptions,
  ): Promise<HarnessSession>;
  resume(
    sessionId: string,
    prompt: string,
    cwd: string,
    options?: HarnessExecutionOptions,
  ): Promise<HarnessSession>;
}

export interface ExecutableFileIdentity {
  command: string;
  path: string;
  sha256: string;
}

export interface HarnessIdentity extends ExecutableFileIdentity {
  version: string;
}

/**
 * Prefix applied only to measured harness turns. Identity probes stay outside
 * the sandbox so the benchmark can record the real harness executable.
 */
export interface HarnessSandbox {
  file: string;
  args: readonly string[];
  mechanism: string;
}

export interface HarnessProgress {
  status: "running";
  prompt: string;
  transcript: readonly Json[];
  usage: HarnessUsage;
  wallTimeMs: number;
  stdout: string;
  stderr: string;
}

export interface HarnessExecutionOptions {
  timeoutMs?: number;
  onProgress?: (progress: HarnessProgress) => void | Promise<void>;
}

export interface HarnessCommand {
  file: string;
  args: readonly string[];
}

abstract class JsonlAdapter implements HarnessAdapter {
  private readonly environment: NodeJS.ProcessEnv;
  private readonly sandbox?: HarnessSandbox;

  constructor(
    environment: NodeJS.ProcessEnv = process.env,
    sandbox?: HarnessSandbox,
  ) {
    this.environment = { ...environment };
    this.sandbox = sandbox;
  }

  abstract readonly name: HarnessName;
  protected abstract startCommand(prompt: string): HarnessCommand;
  protected abstract resumeCommand(sessionId: string, prompt: string): HarnessCommand;

  async identity(): Promise<HarnessIdentity> {
    const command = this.binary();
    let executable: ExecutableFileIdentity;
    try {
      executable = await executableFileIdentity(command, this.environment);
    } catch (error) {
      if (isMissingExecutable(error)) {
        const variable = `CORI_BENCH_${this.name.toUpperCase()}_BIN`;
        throw new Error(`cannot find ${this.name} harness executable \`${command}\`; install it or set ${variable} to its absolute path`);
      }
      throw error;
    }
    const result = await exec(
      executable.path,
      ["--version"],
      undefined,
      undefined,
      undefined,
      this.environment,
    );
    if (result.code !== 0) throw new Error(`${this.name} --version failed: ${result.stderr}`);
    return { ...executable, version: result.stdout.trim() };
  }

  async version(): Promise<string> {
    return (await this.identity()).version;
  }

  async start(
    prompt: string,
    cwd: string,
    options: HarnessExecutionOptions = {},
  ): Promise<HarnessSession> {
    return this.execute(this.startCommand(prompt), cwd, prompt, options);
  }

  async resume(
    sessionId: string,
    prompt: string,
    cwd: string,
    options: HarnessExecutionOptions = {},
  ): Promise<HarnessSession> {
    return this.execute(
      this.resumeCommand(sessionId, prompt),
      cwd,
      prompt,
      options,
    );
  }

  protected binary(): string {
    const configured =
      this.environment[`CORI_BENCH_${this.name.toUpperCase()}_BIN`];
    if (configured) return configured;
    if (this.name === "codex") {
      const appBundled = "/Applications/ChatGPT.app/Contents/Resources/codex";
      if (existsSync(appBundled)) return appBundled;
    }
    return this.name;
  }

  private async execute(
    command: HarnessCommand,
    cwd: string,
    prompt: string,
    options: HarnessExecutionOptions,
  ): Promise<HarnessSession> {
    const started = performance.now();
    let result: { code: number; stdout: string; stderr: string; timedOut: boolean };
    try {
      result = await exec(
        command.file,
        command.args,
        cwd,
        options.timeoutMs,
        async (stdout, stderr) => {
          const transcript = parseJsonl(stdout);
          await options.onProgress?.({
            status: "running",
            prompt,
            transcript,
            usage: usageFrom(transcript),
            wallTimeMs: Math.round(performance.now() - started),
            stdout,
            stderr,
          });
        },
        this.environment,
        this.sandbox,
      );
    } catch (error) {
      if (isMissingExecutable(error)) {
        const variable = `CORI_BENCH_${this.name.toUpperCase()}_BIN`;
        throw new Error(`cannot find ${this.name} harness executable \`${command.file}\`; install it or set ${variable} to its absolute path`);
      }
      throw error;
    }
    const transcript = parseJsonl(result.stdout);
    return {
      sessionId: sessionIdFrom(transcript),
      prompt,
      transcript,
      usage: usageFrom(transcript),
      wallTimeMs: Math.round(performance.now() - started),
      exitCode: result.code,
      stdout: result.stdout,
      stderr: result.stderr,
      ...(result.timedOut ? { timedOut: true } : {}),
    };
  }
}

function isMissingExecutable(error: unknown): boolean {
  return !!error && typeof error === "object" && "code" in error && (error as { code?: unknown }).code === "ENOENT";
}

/**
 * Resolve an executable without invoking a shell. This keeps benchmark
 * identity checks portable and ensures the same PATH that the harness receives
 * is the one being measured.
 */
export async function resolveExecutablePath(
  command: string,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<string> {
  const hasDirectory = isAbsolute(command) ||
    command.includes("/") ||
    command.includes("\\");
  const candidates = hasDirectory
    ? [resolve(command)]
    : executableCandidates(command, environment);
  for (const candidate of candidates) {
    try {
      await access(
        candidate,
        process.platform === "win32" ? constants.F_OK : constants.X_OK,
      );
      return resolve(candidate);
    } catch {
      // Try the next PATH/PATHEXT candidate.
    }
  }
  const error = new Error(`cannot find executable ${command}`) as NodeJS.ErrnoException;
  error.code = "ENOENT";
  throw error;
}

export async function executableFileIdentity(
  command: string,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<ExecutableFileIdentity> {
  const path = await resolveExecutablePath(command, environment);
  return {
    command,
    path,
    sha256: createHash("sha256").update(await readFile(path)).digest("hex"),
  };
}

function executableCandidates(
  command: string,
  environment: NodeJS.ProcessEnv,
): string[] {
  const directories = (environment.PATH ?? "")
    .split(delimiter)
    .filter(Boolean);
  const extensions = process.platform === "win32"
    ? (environment.PATHEXT ?? ".COM;.EXE;.BAT;.CMD")
      .split(";")
      .filter(Boolean)
    : [""];
  const hasExtension = process.platform === "win32" &&
    extensions.some((extension) => command.toLowerCase().endsWith(extension.toLowerCase()));
  return directories.flatMap((directory) =>
    hasExtension
      ? [join(directory, command)]
      : extensions.map((extension) => join(directory, `${command}${extension}`))
  );
}

export class CodexAdapter extends JsonlAdapter {
  readonly name = "codex" as const;
  protected startCommand(prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["exec", ...codexAutomationArgs(), prompt] };
  }
  protected resumeCommand(sessionId: string, prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["exec", "resume", ...codexAutomationArgs(), sessionId, prompt] };
  }
}

/**
 * The direct lane must reach authenticated Workspace CLIs and the network.
 * Keep the harness free of user plugins/config so the measured tool surface is
 * the benchmark-local `gws` CLI, then grant shell commands the access that CLI
 * needs. The benchmark provisions and grades namespaced synthetic resources.
 */
export function codexAutomationArgs(): readonly string[] {
  return [
    "--json",
    "--model", codexModel(),
    "--ignore-user-config",
    "--ignore-rules",
    "--disable", "plugins",
    "--disable", "apps",
    "--disable", "browser_use",
    "--disable", "in_app_browser",
    "--disable", "computer_use",
    "--dangerously-bypass-approvals-and-sandbox",
  ];
}

export class ClaudeAdapter extends JsonlAdapter {
  readonly name = "claude" as const;
  protected startCommand(prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["-p", "--output-format", "stream-json", "--verbose", prompt] };
  }
  protected resumeCommand(sessionId: string, prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["-p", "--resume", sessionId, "--output-format", "stream-json", "--verbose", prompt] };
  }
}

export class GeminiAdapter extends JsonlAdapter {
  readonly name = "gemini" as const;
  protected startCommand(prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["-p", prompt, "--output-format", "stream-json"] };
  }
  protected resumeCommand(sessionId: string, prompt: string): HarnessCommand {
    return { file: this.binary(), args: ["-p", prompt, "--resume", sessionId, "--output-format", "stream-json"] };
  }
}

export function adapterFor(
  name: HarnessName,
  environment: NodeJS.ProcessEnv = process.env,
  sandbox?: HarnessSandbox,
): HarnessAdapter {
  if (name === "codex") return new CodexAdapter(environment, sandbox);
  if (name === "claude") return new ClaudeAdapter(environment, sandbox);
  return new GeminiAdapter(environment, sandbox);
}

export function parseJsonl(stdout: string): readonly Json[] {
  return stdout.split(/\r?\n/u).flatMap((line) => {
    const trimmed = line.trim();
    if (!trimmed) return [];
    try {
      return [JSON.parse(trimmed) as Json];
    } catch {
      return [{ type: "unparsed", text: trimmed }];
    }
  });
}

function sessionIdFrom(events: readonly Json[]): string | null {
  for (const event of events) {
    const candidate = findString(event, ["session_id", "sessionId", "thread_id", "threadId"]);
    if (candidate) return candidate;
  }
  return null;
}

function usageFrom(events: readonly Json[]): HarnessUsage {
  let inputTokens: number | null = null;
  let outputTokens: number | null = null;
  // A single tool call emits a lifecycle pair (item.started + item.completed)
  // sharing one item id. Counting raw matching events double-counts every call,
  // so identified calls are deduplicated and only unidentifiable ones are
  // counted positionally.
  const seenToolIds = new Set<string>();
  let anonymousToolCalls = 0;
  for (const event of events) {
    const input = findNumber(event, ["input_tokens", "inputTokens", "prompt_tokens"]);
    const output = findNumber(event, ["output_tokens", "outputTokens", "completion_tokens"]);
    if (input !== null) inputTokens = (inputTokens ?? 0) + input;
    if (output !== null) outputTokens = (outputTokens ?? 0) + output;
    if (!containsToolEvent(event)) continue;
    const id = findString(event, ["id", "item_id", "itemId", "call_id", "callId"]);
    if (id === null) anonymousToolCalls += 1;
    else seenToolIds.add(id);
  }
  return { inputTokens, outputTokens, toolCalls: seenToolIds.size + anonymousToolCalls };
}

function findString(value: Json, names: readonly string[]): string | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  for (const name of names) if (typeof value[name] === "string") return value[name] as string;
  for (const nested of Object.values(value)) {
    const found = findString(nested, names);
    if (found) return found;
  }
  return null;
}

function findNumber(value: Json, names: readonly string[]): number | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  for (const name of names) if (typeof value[name] === "number") return value[name] as number;
  for (const nested of Object.values(value)) {
    const found = findNumber(nested, names);
    if (found !== null) return found;
  }
  return null;
}

function containsToolEvent(value: Json): boolean {
  if (Array.isArray(value)) return value.some(containsToolEvent);
  if (!value || typeof value !== "object") return false;
  return Object.values(value).some((entry) =>
    typeof entry === "string"
      ? /(?:tool|function_call|command_execution|mcp_call)/u.test(entry)
      : containsToolEvent(entry)
  );
}

async function exec(
  file: string,
  args: readonly string[],
  cwd?: string,
  timeoutMs?: number,
  onProgress?: (stdout: string, stderr: string) => void | Promise<void>,
  environment: NodeJS.ProcessEnv = process.env,
  sandbox?: HarnessSandbox,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  return new Promise((resolve, reject) => {
    const child = spawn(
      sandbox?.file ?? file,
      sandbox ? [...sandbox.args, file, ...args] : [...args],
      {
      cwd,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      detached: process.platform !== "win32",
      env: environment,
      },
    );
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let progressWrites = Promise.resolve();
    const publish = (): void => {
      progressWrites = progressWrites.then(async () => {
        await onProgress?.(stdout, stderr);
      }).catch(() => undefined);
    };
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
      stdout += chunk;
      publish();
    });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
      stderr += chunk;
      publish();
    });
    const heartbeat = onProgress
      ? setInterval(publish, 5_000)
      : undefined;
    const deadline = timeoutMs && timeoutMs > 0
      ? setTimeout(() => {
        timedOut = true;
        stderr += `\nbenchmark phase timed out after ${timeoutMs}ms\n`;
        publish();
        terminateProcessTree(child, "SIGTERM");
        setTimeout(() => terminateProcessTree(child, "SIGKILL"), 2_000)
          .unref();
      }, timeoutMs)
      : undefined;
    child.once("error", reject);
    child.once("close", (code) => {
      if (heartbeat) clearInterval(heartbeat);
      if (deadline) clearTimeout(deadline);
      progressWrites.finally(() => resolve({
        code: timedOut ? 124 : code ?? 1,
        stdout,
        stderr,
        timedOut,
      }));
    });
  });
}

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
    // The process may have exited between the deadline and signal delivery.
  }
}

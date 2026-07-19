import { spawn } from "node:child_process";
import { existsSync } from "node:fs";

import type { HarnessName, HarnessSession, HarnessUsage, Json } from "./types.js";

export const DEFAULT_CODEX_MODEL = "gpt-5.6-terra";

export function codexModel(): string {
  return process.env.CORI_BENCH_CODEX_MODEL ?? DEFAULT_CODEX_MODEL;
}

export interface HarnessAdapter {
  readonly name: HarnessName;
  version(): Promise<string>;
  start(prompt: string, cwd: string): Promise<HarnessSession>;
  resume(sessionId: string, prompt: string, cwd: string): Promise<HarnessSession>;
}

export interface HarnessCommand {
  file: string;
  args: readonly string[];
}

abstract class JsonlAdapter implements HarnessAdapter {
  abstract readonly name: HarnessName;
  protected abstract startCommand(prompt: string): HarnessCommand;
  protected abstract resumeCommand(sessionId: string, prompt: string): HarnessCommand;

  async version(): Promise<string> {
    let result: { code: number; stdout: string; stderr: string };
    try {
      result = await exec(this.binary(), ["--version"]);
    } catch (error) {
      if (isMissingExecutable(error)) {
        const variable = `CORI_BENCH_${this.name.toUpperCase()}_BIN`;
        throw new Error(`cannot find ${this.name} harness executable \`${this.binary()}\`; install it or set ${variable} to its absolute path`);
      }
      throw error;
    }
    if (result.code !== 0) throw new Error(`${this.name} --version failed: ${result.stderr}`);
    return result.stdout.trim();
  }

  async start(prompt: string, cwd: string): Promise<HarnessSession> {
    return this.execute(this.startCommand(prompt), cwd, prompt);
  }

  async resume(sessionId: string, prompt: string, cwd: string): Promise<HarnessSession> {
    return this.execute(this.resumeCommand(sessionId, prompt), cwd, prompt);
  }

  protected binary(): string {
    const configured = process.env[`CORI_BENCH_${this.name.toUpperCase()}_BIN`];
    if (configured) return configured;
    if (this.name === "codex") {
      const appBundled = "/Applications/ChatGPT.app/Contents/Resources/codex";
      if (existsSync(appBundled)) return appBundled;
    }
    return this.name;
  }

  private async execute(command: HarnessCommand, cwd: string, prompt: string): Promise<HarnessSession> {
    const started = performance.now();
    let result: { code: number; stdout: string; stderr: string };
    try {
      result = await exec(command.file, command.args, cwd);
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
    };
  }
}

function isMissingExecutable(error: unknown): boolean {
  return !!error && typeof error === "object" && "code" in error && (error as { code?: unknown }).code === "ENOENT";
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

export function adapterFor(name: HarnessName): HarnessAdapter {
  if (name === "codex") return new CodexAdapter();
  if (name === "claude") return new ClaudeAdapter();
  return new GeminiAdapter();
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
  let toolCalls: number | null = 0;
  for (const event of events) {
    const input = findNumber(event, ["input_tokens", "inputTokens", "prompt_tokens"]);
    const output = findNumber(event, ["output_tokens", "outputTokens", "completion_tokens"]);
    if (input !== null) inputTokens = (inputTokens ?? 0) + input;
    if (output !== null) outputTokens = (outputTokens ?? 0) + output;
    if (containsToolEvent(event)) toolCalls = (toolCalls ?? 0) + 1;
  }
  return { inputTokens, outputTokens, toolCalls };
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
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  return Object.values(value).some((entry) => typeof entry === "string" && /tool|function_call/u.test(entry));
}

async function exec(file: string, args: readonly string[], cwd?: string): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(file, [...args], { cwd, shell: false, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
    child.once("error", reject);
    child.once("close", (code) => resolve({ code: code ?? 1, stdout, stderr }));
  });
}

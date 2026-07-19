import { readdir, readFile, stat, writeFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

import {
  INPUT_TOKEN_PRICE_PER_MILLION_USD,
  OUTPUT_TOKEN_PRICE_PER_MILLION_USD,
  readJson,
} from "./artifacts.js";
import type { BenchmarkResultV1 } from "./types.js";

export interface ViewerArtifact {
  path: string;
  content?: string;
  contentTruncated?: boolean;
  kind: ArtifactKind;
  size?: number;
  modifiedAt?: string;
  href?: string;
  links?: readonly ViewerLink[];
}

export interface ViewerLink {
  label: string;
  url: string;
}

export type ArtifactKind =
  | "result"
  | "run"
  | "transcript"
  | "snapshot"
  | "grade"
  | "trace"
  | "workflow"
  | "check"
  | "report"
  | "workspace"
  | "other";

interface ViewerPayload {
  result: BenchmarkResultV1;
  artifacts: readonly ViewerArtifact[];
  sessions: readonly ViewerSession[];
  sessionRows: readonly ViewerSessionComparison[];
}

interface ViewerSession {
  taskId: string;
  name: string;
  meta: string;
  messages: readonly ViewerMessage[];
  sourcePath: string;
}

interface ViewerMessage {
  role: "user" | "assistant" | "tool";
  label: string;
  text: string;
  detail?: string;
  status?: string;
  index: number;
}

interface ViewerSessionComparison {
  id: string;
  taskId: string;
  label: string;
  engine: "agent" | "cori";
  track: "author" | "direct" | "cori";
  status: string;
  startedAt: string | null;
  durationMs: number | null;
  attempt: number | null;
  seed: number | null;
  score: number | null;
  toolCalls: number | null;
  cliCalls: number | null;
  llmCalls: number | null;
  codeCalls: number | null;
  messages: number | null;
  events: number | null;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  tokensPerSecond: number | null;
  priceUsd: number | null;
  workflowHash: string | null;
  evidencePath: string;
  pairId: string | null;
}

/**
 * Build a portable review index beside a run's persisted evidence. The page
 * embeds compact summaries and normalized conversations; large raw artifacts
 * stay in their original files and are reached through relative links.
 */
export function benchmarkViewerDocument(
  result: BenchmarkResultV1,
  artifacts: readonly ViewerArtifact[],
): string {
  const payload: ViewerPayload = {
    result: compactResult(result),
    artifacts: artifacts.map(prepareViewerArtifact).sort(compareViewerArtifacts),
    sessions: buildViewerSessions(result, artifacts),
    sessionRows: buildViewerSessionComparisons(result, artifacts),
  };
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Cori benchmark review — ${escapeHtml(result.runId)}</title>
  <style>
    :root {
      color-scheme: light dark;
      --canvas: #f3f6fa;
      --surface: #ffffff;
      --surface-raised: #fbfcfe;
      --ink: #14243a;
      --ink-soft: #52657b;
      --line: #dce4ed;
      --line-strong: #c8d4e0;
      --violet: #5b55d8;
      --violet-soft: #eeedff;
      --green: #13795b;
      --green-soft: #e4f6ee;
      --amber: #9a6207;
      --amber-soft: #fff3dc;
      --red: #b53b55;
      --red-soft: #ffeaee;
      --blue: #2e6da4;
      --blue-soft: #e8f2fc;
      --terminal: #122238;
      --terminal-text: #d7e3f0;
      --mono: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      --sans: Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      --shadow: 0 12px 32px rgba(26, 45, 72, .08);
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --canvas: #0f1724;
        --surface: #172131;
        --surface-raised: #1d293b;
        --ink: #e6edf5;
        --ink-soft: #aebdd0;
        --line: #2c3a4d;
        --line-strong: #40506a;
        --violet: #a9a4ff;
        --violet-soft: #29284d;
        --green: #72dbaf;
        --green-soft: #173b34;
        --amber: #f3bf5d;
        --amber-soft: #49391d;
        --red: #ff9cae;
        --red-soft: #4a2732;
        --blue: #8bc7ff;
        --blue-soft: #1b3550;
        --terminal: #0b111b;
        --terminal-text: #dce8f5;
        --shadow: 0 16px 38px rgba(0, 0, 0, .25);
      }
    }
    * { box-sizing: border-box; }
    html { background: var(--canvas); }
    body { margin: 0; min-width: 320px; color: var(--ink); background: var(--canvas); font: 14px/1.5 var(--sans); }
    button, input, select { font: inherit; }
    button { cursor: pointer; }
    button:focus-visible, input:focus-visible { outline: 3px solid color-mix(in srgb, var(--violet) 45%, transparent); outline-offset: 2px; }
    code, pre { font-family: var(--mono); }
    .shell { display: grid; grid-template-columns: 272px minmax(0, 1fr); min-height: 100vh; transition: grid-template-columns 160ms ease; }
    .sidebar { position: sticky; top: 0; display: flex; flex-direction: column; height: 100vh; padding: 20px 14px; overflow: auto; border-right: 1px solid var(--line); background: var(--surface); }
    .sidebar-toggle { position: fixed; z-index: 10; top: 12px; left: 284px; display: grid; width: 28px; height: 28px; place-items: center; padding: 0; border: 1px solid var(--line-strong); border-radius: 7px; color: var(--ink-soft); background: var(--surface); box-shadow: 0 2px 7px rgba(26, 45, 72, .12); font-size: 16px; line-height: 1; transition: left 160ms ease; }
    .sidebar-toggle:hover { color: var(--violet); border-color: var(--violet); background: var(--violet-soft); }
    body.sidebar-hidden .shell { grid-template-columns: 0 minmax(0, 1fr); }
    body.sidebar-hidden .sidebar { visibility: hidden; padding: 0; overflow: hidden; border-right: 0; }
    body.sidebar-hidden .sidebar-toggle { left: 10px; }
    .brand { display: flex; align-items: center; gap: 10px; padding: 3px 8px 22px; }
    .brand-mark { display: grid; width: 28px; height: 28px; place-items: center; border-radius: 8px; color: #fff; background: var(--violet); font: 700 17px/1 var(--mono); }
    .brand-title { display: block; font-size: 15px; font-weight: 700; letter-spacing: -.01em; }
    .brand-subtitle { display: block; color: var(--ink-soft); font: 10px/1.3 var(--mono); letter-spacing: .07em; text-transform: uppercase; }
    .nav-label { margin: 18px 8px 7px; color: var(--ink-soft); font: 700 10px/1.2 var(--mono); letter-spacing: .1em; text-transform: uppercase; }
    .nav { display: grid; gap: 3px; }
    .nav-button { display: flex; align-items: center; width: 100%; gap: 9px; padding: 8px 9px; overflow: hidden; border: 0; border-radius: 7px; color: var(--ink-soft); background: transparent; text-align: left; }
    .nav-button:hover { color: var(--ink); background: var(--canvas); }
    .nav-button[aria-current="page"] { color: var(--violet); background: var(--violet-soft); font-weight: 650; }
    .nav-button .nav-dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; opacity: .8; flex: 0 0 auto; }
    .nav-button .nav-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .nav-count { min-width: 22px; padding: 1px 5px; margin-left: auto; border-radius: 99px; color: var(--ink-soft); background: var(--canvas); font: 700 9px var(--mono); text-align: center; }
    .side-note { margin-top: auto; padding: 16px 8px 0; border-top: 1px solid var(--line); color: var(--ink-soft); font-size: 11px; }
    .side-note code { display: block; margin-top: 3px; overflow: hidden; color: var(--ink); font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
    main { min-width: 0; }
    .topbar { display: flex; align-items: center; justify-content: space-between; min-height: 65px; padding: 0 36px; border-bottom: 1px solid var(--line); background: color-mix(in srgb, var(--surface) 92%, transparent); }
    .crumb { overflow: hidden; color: var(--ink-soft); font: 11px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .content { width: min(1280px, 100%); padding: 34px 40px 64px; margin: 0 auto; }
    body.table-view, body.table-view .shell, body.table-view main { height: 100vh; overflow: hidden; }
    body.table-view .topbar { display: none; }
    .content.table-content { width: 100%; height: 100vh; padding: 0; margin: 0; }
    .eyebrow { margin: 0 0 8px; color: var(--violet); font: 700 10px/1.3 var(--mono); letter-spacing: .12em; text-transform: uppercase; }
    h1, h2, h3, p { margin-top: 0; }
    h1 { margin-bottom: 7px; font-size: clamp(25px, 4vw, 34px); line-height: 1.1; letter-spacing: -.035em; }
    h2 { margin: 31px 0 12px; font-size: 15px; letter-spacing: -.012em; }
    h3 { margin-bottom: 5px; font-size: 14px; }
    .subtitle { max-width: 760px; margin-bottom: 0; color: var(--ink-soft); }
    .title-row, .section-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: 18px; }
    .status { display: inline-flex; align-items: center; gap: 6px; flex: 0 0 auto; padding: 5px 9px; border-radius: 99px; font: 700 11px var(--mono); text-transform: uppercase; }
    .status::before { width: 6px; height: 6px; border-radius: 50%; background: currentColor; content: ""; }
    .status.ok { color: var(--green); background: var(--green-soft); }
    .status.warn { color: var(--amber); background: var(--amber-soft); }
    .status.bad { color: var(--red); background: var(--red-soft); }
    .status.neutral { color: var(--blue); background: var(--blue-soft); }
    .metrics { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; margin: 25px 0 6px; }
    .metric { min-width: 0; padding: 15px; border: 1px solid var(--line); border-radius: 10px; background: var(--surface); box-shadow: 0 1px 1px rgba(0,0,0,.02); }
    .metric-label { margin-bottom: 5px; color: var(--ink-soft); font: 700 10px/1.2 var(--mono); letter-spacing: .07em; text-transform: uppercase; }
    .metric-value { overflow: hidden; font-size: 21px; font-weight: 650; letter-spacing: -.025em; text-overflow: ellipsis; white-space: nowrap; }
    .metric-detail { min-height: 18px; margin-top: 3px; color: var(--ink-soft); font-size: 11px; }
    .panel { border: 1px solid var(--line); border-radius: 11px; background: var(--surface); box-shadow: 0 1px 1px rgba(0,0,0,.02); }
    .panel-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 14px 16px; border-bottom: 1px solid var(--line); }
    .panel-head h2, .panel-head h3 { margin: 0; }
    .task-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .task-card { display: block; min-width: 0; padding: 17px; border: 0; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); color: inherit; background: transparent; text-align: left; }
    .task-card:nth-child(2n) { border-right: 0; }
    .task-card:nth-last-child(-n + 2) { border-bottom: 0; }
    .task-card:hover { background: var(--surface-raised); }
    .task-title { display: flex; align-items: center; justify-content: space-between; gap: 9px; margin-bottom: 8px; font-weight: 650; }
    .task-id { overflow: hidden; font: 11px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .task-meta { display: flex; gap: 12px; color: var(--ink-soft); font-size: 12px; }
    .grade { display: inline-flex; align-items: center; justify-content: center; min-width: 35px; padding: 3px 7px; border-radius: 6px; color: var(--blue); background: var(--blue-soft); font: 700 12px var(--mono); }
    .grade.good { color: var(--green); background: var(--green-soft); }
    .grade.low { color: var(--amber); background: var(--amber-soft); }
    .grade.bad { color: var(--red); background: var(--red-soft); }
    .notice { margin: 18px 0; padding: 12px 14px; border-left: 3px solid var(--red); border-radius: 0 7px 7px 0; color: var(--red); background: var(--red-soft); }
    .tabs { display: flex; gap: 5px; padding: 5px; overflow-x: auto; border: 1px solid var(--line); border-radius: 9px; background: var(--surface-raised); }
    .tab { flex: 0 0 auto; padding: 7px 10px; border: 0; border-radius: 6px; color: var(--ink-soft); background: transparent; font-size: 12px; font-weight: 650; }
    .tab:hover { color: var(--ink); }
    .tab[aria-selected="true"] { color: var(--violet); background: var(--surface); box-shadow: 0 1px 2px rgba(0,0,0,.07); }
    .task-intro { padding: 20px 0 16px; }
    .task-facts { display: flex; flex-wrap: wrap; gap: 8px; color: var(--ink-soft); font-size: 12px; }
    .fact { padding: 4px 7px; border: 1px solid var(--line); border-radius: 5px; background: var(--surface); }
    .comparison { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; margin: 16px 0; }
    .lane { padding: 14px; border: 1px solid var(--line); border-radius: 9px; background: var(--surface); }
    .lane-title { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin-bottom: 10px; font-size: 12px; font-weight: 700; }
    .lane-row { display: flex; justify-content: space-between; gap: 14px; padding: 7px 0; border-top: 1px solid var(--line); color: var(--ink-soft); font-size: 12px; }
    .lane-row b { color: var(--ink); font-family: var(--mono); font-weight: 650; }
    .conversation-layout { display: grid; grid-template-columns: 240px minmax(0, 1fr); min-height: 520px; margin-top: 16px; border: 1px solid var(--line); border-radius: 11px; overflow: hidden; background: var(--surface); }
    .session-list { padding: 8px; border-right: 1px solid var(--line); background: var(--surface-raised); }
    .session-button { display: block; width: 100%; padding: 10px; border: 0; border-radius: 7px; color: var(--ink-soft); background: transparent; text-align: left; }
    .session-button:hover { color: var(--ink); background: var(--canvas); }
    .session-button[aria-current="true"] { color: var(--violet); background: var(--violet-soft); }
    .session-name { display: block; overflow: hidden; font-size: 12px; font-weight: 650; text-overflow: ellipsis; white-space: nowrap; }
    .session-meta { display: block; margin-top: 3px; font: 10px var(--mono); opacity: .8; }
    .session-task { display: block; margin-top: 4px; overflow: hidden; color: var(--ink); font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
    .chat { min-width: 0; padding: 22px; }
    .chat-head { display: flex; align-items: baseline; justify-content: space-between; gap: 16px; margin-bottom: 20px; }
    .chat-head h2 { margin: 0; }
    .chat-head span { color: var(--ink-soft); font: 10px var(--mono); }
    .message-list { display: grid; gap: 14px; }
    .message { max-width: min(780px, 100%); padding: 12px 14px; border: 1px solid var(--line); border-radius: 9px; background: var(--surface-raised); }
    .message.user { margin-right: auto; border-top-left-radius: 3px; }
    .message.assistant { margin-left: auto; border-color: color-mix(in srgb, var(--violet) 26%, var(--line)); border-top-right-radius: 3px; background: color-mix(in srgb, var(--violet-soft) 42%, var(--surface)); }
    .message.tool { max-width: 100%; border-style: dashed; background: var(--canvas); }
    .message-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 7px; color: var(--ink-soft); font: 700 10px var(--mono); letter-spacing: .06em; text-transform: uppercase; }
    .message-text { margin: 0; white-space: pre-wrap; word-break: break-word; }
    .message details { margin-top: 9px; color: var(--ink-soft); }
    .message details pre { max-height: 360px; margin: 8px 0 0; }
    summary { cursor: pointer; font-size: 12px; }
    .external-link { color: var(--violet); text-decoration: underline; text-decoration-thickness: 1px; text-underline-offset: 2px; }
    .external-link:hover { color: var(--blue); }
    pre .external-link { color: #b7d9ff; }
    pre .external-link:hover { color: #e4f1ff; }
    pre { max-width: 100%; padding: 13px; overflow: auto; border-radius: 7px; color: var(--terminal-text); background: var(--terminal); font-size: 11px; line-height: 1.55; white-space: pre-wrap; word-break: break-word; }
    .empty { padding: 28px; border: 1px dashed var(--line-strong); border-radius: 9px; color: var(--ink-soft); text-align: center; }
    .evidence-grid { display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(260px, .8fr); gap: 15px; margin-top: 16px; }
    .evidence-list { display: grid; gap: 7px; padding: 8px; }
    .artifact-button { display: flex; align-items: center; width: 100%; gap: 10px; padding: 10px; overflow: hidden; border: 0; border-radius: 7px; color: var(--ink-soft); background: transparent; text-align: left; }
    .artifact-button:hover, .artifact-button[aria-current="true"] { color: var(--ink); background: var(--surface-raised); }
    .artifact-kind { display: inline-flex; min-width: 71px; justify-content: center; padding: 3px 5px; border-radius: 4px; color: var(--blue); background: var(--blue-soft); font: 700 9px var(--mono); letter-spacing: .04em; text-transform: uppercase; }
    .artifact-path { overflow: hidden; font: 11px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .artifact-view { min-width: 0; padding: 16px; border-left: 1px solid var(--line); }
    .artifact-view h3 { overflow: hidden; font: 12px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .artifact-view pre { margin: 10px 0 0; max-height: 590px; }
    .artifact-toolbar { display: grid; width: 100%; gap: 9px; }
    .artifact-filter { width: 100%; padding: 8px 10px; border: 1px solid var(--line-strong); border-radius: 7px; color: var(--ink); background: var(--surface); font-size: 12px; }
    .artifact-filters { display: flex; gap: 5px; padding-bottom: 1px; overflow-x: auto; }
    .artifact-filter-button { display: inline-flex; align-items: center; gap: 5px; flex: 0 0 auto; padding: 5px 7px; border: 1px solid var(--line); border-radius: 6px; color: var(--ink-soft); background: var(--surface); font-size: 10px; }
    .artifact-filter-button[aria-pressed="true"] { border-color: color-mix(in srgb, var(--violet) 45%, var(--line)); color: var(--violet); background: var(--violet-soft); }
    .artifact-filter-button b { font: 700 9px var(--mono); }
    .artifact-list-summary { padding: 9px 10px 3px; color: var(--ink-soft); font: 10px var(--mono); }
    .artifact-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 9px; margin: 9px 0 14px; }
    .artifact-open { display: inline-flex; padding: 7px 10px; border: 1px solid var(--line-strong); border-radius: 7px; color: var(--violet); background: var(--surface-raised); font-size: 12px; font-weight: 650; text-decoration: none; }
    .artifact-open:hover { border-color: var(--violet); background: var(--violet-soft); }
    .resource-links { display: grid; gap: 7px; margin: 12px 0 16px; padding: 12px; border: 1px solid var(--line); border-radius: 8px; background: var(--surface-raised); }
    .resource-link { overflow: hidden; color: var(--violet); font: 11px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .rubric { display: grid; gap: 7px; padding: 14px 16px; }
    .rubric-item { display: grid; grid-template-columns: 48px minmax(0, 1fr); gap: 10px; padding: 9px 0; border-bottom: 1px solid var(--line); }
    .rubric-item:last-child { border-bottom: 0; }
    .rubric-score { color: var(--ink-soft); font: 700 11px var(--mono); }
    .rubric-title { font-weight: 650; }
    .rubric-note { margin-top: 2px; color: var(--ink-soft); font-size: 12px; }
    .file-tree { display: grid; gap: 7px; }
    .workflow-file { padding: 12px 14px; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); }
    .workflow-file summary { overflow: hidden; color: var(--ink); font: 650 11px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .workflow-file pre { margin: 10px 0 0; max-height: 500px; }
    .small { color: var(--ink-soft); font-size: 12px; }
    .back { padding: 0; border: 0; color: var(--violet); background: transparent; font-size: 12px; font-weight: 650; }
    .session-table-panel { margin-top: 20px; overflow: hidden; border: 1px solid var(--line); border-radius: 11px; background: var(--surface); box-shadow: var(--shadow); }
    body.table-view .session-table-panel { display: flex; height: 100vh; margin: 0; border: 0; border-radius: 0; flex-direction: column; box-shadow: none; }
    .session-table-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; border-bottom: 1px solid var(--line); background: var(--surface-raised); }
    .session-table-controls { display: flex; align-items: center; gap: 7px; min-width: 0; }
    .session-table-search { width: min(320px, 34vw); padding: 7px 9px; border: 1px solid var(--line-strong); border-radius: 7px; color: var(--ink); background: var(--surface); font-size: 11px; }
    .session-filter { padding: 6px 9px; border: 1px solid var(--line); border-radius: 6px; color: var(--ink-soft); background: var(--surface); font-size: 10px; font-weight: 700; }
    .session-filter[aria-pressed="true"] { border-color: color-mix(in srgb, var(--violet) 45%, var(--line)); color: var(--violet); background: var(--violet-soft); }
    .trend-legend { color: var(--ink-soft); font: 10px var(--mono); white-space: nowrap; }
    .session-table-scroll { max-height: min(720px, calc(100vh - 235px)); overflow: auto; overscroll-behavior: contain; }
    body.table-view .session-table-toolbar { min-height: 45px; padding: 8px 14px 8px 52px; }
    body.table-view .session-table-scroll { max-height: none; min-height: 0; flex: 1 1 auto; }
    .session-table { width: 100%; min-width: 2280px; border-spacing: 0; border-collapse: separate; font-size: 11px; }
    .session-table th { position: sticky; top: 0; z-index: 3; padding: 9px 10px; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line-strong); color: var(--ink-soft); background: var(--surface-raised); font: 700 9px/1.25 var(--mono); letter-spacing: .05em; text-align: left; text-transform: uppercase; white-space: nowrap; }
    .session-table td { height: 54px; padding: 8px 10px; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); vertical-align: middle; }
    .session-table th:last-child, .session-table td:last-child { border-right: 0; }
    .session-table tbody tr:last-child td { border-bottom: 0; }
    .session-table tbody tr:hover td { background: color-mix(in srgb, var(--violet-soft) 26%, var(--surface)); }
    .session-table .sticky-order { position: sticky; left: 0; z-index: 2; width: 44px; min-width: 44px; color: var(--ink-soft); background: var(--surface); font-family: var(--mono); text-align: right; }
    .session-table th.sticky-order { z-index: 5; background: var(--surface-raised); }
    .session-table .sticky-session { position: sticky; left: 44px; z-index: 2; width: 218px; min-width: 218px; background: var(--surface); }
    .session-table th.sticky-session { z-index: 5; background: var(--surface-raised); }
    .session-table tbody tr:hover .sticky-order, .session-table tbody tr:hover .sticky-session { background: color-mix(in srgb, var(--violet-soft) 26%, var(--surface)); }
    .session-primary { display: block; max-width: 200px; overflow: hidden; color: var(--ink); font-weight: 700; text-overflow: ellipsis; white-space: nowrap; }
    .session-secondary { display: block; max-width: 200px; margin-top: 2px; overflow: hidden; color: var(--ink-soft); font: 9px var(--mono); text-overflow: ellipsis; white-space: nowrap; }
    .engine-pill { display: inline-flex; align-items: center; gap: 5px; padding: 3px 7px; border-radius: 99px; font: 700 9px var(--mono); text-transform: uppercase; }
    .engine-pill.agent { color: var(--violet); background: var(--violet-soft); }
    .engine-pill.cori { color: var(--green); background: var(--green-soft); }
    .table-status { font: 700 9px var(--mono); text-transform: uppercase; }
    .table-status.ok { color: var(--green); }
    .table-status.bad { color: var(--red); }
    .number-cell { min-width: 92px; text-align: right; }
    .number-main { display: block; color: var(--ink); font: 700 11px var(--mono); white-space: nowrap; }
    .trend { display: block; margin-top: 2px; font: 9px var(--mono); white-space: nowrap; }
    .trend.up { color: var(--violet); }
    .trend.down { color: var(--amber); }
    .trend.flat, .trend.first { color: var(--ink-soft); }
    .pair-cell { min-width: 112px; text-align: right; }
    .pair-baseline { color: var(--ink-soft); font: 9px var(--mono); }
    .session-evidence { display: inline-flex; align-items: center; gap: 5px; color: var(--violet); font: 700 10px var(--mono); text-decoration: none; white-space: nowrap; }
    .session-evidence:hover { text-decoration: underline; }
    .session-table-empty { padding: 42px; color: var(--ink-soft); text-align: center; }
    .session-table-summary { color: var(--ink-soft); font: 10px var(--mono); white-space: nowrap; }
    @media (max-width: 850px) {
      .shell { grid-template-columns: 1fr; }
      .sidebar { position: static; height: auto; padding: 13px; border-right: 0; border-bottom: 1px solid var(--line); }
      body.sidebar-hidden .shell { grid-template-columns: 1fr; }
      body.sidebar-hidden .sidebar { display: none; }
      .sidebar-toggle { left: auto; right: 12px; }
      body.sidebar-hidden .sidebar-toggle { left: auto; right: 10px; }
      .brand { padding-bottom: 10px; }
      .nav-label { display: none; }
      .nav { display: flex; overflow: auto; }
      .nav-button { width: auto; flex: 0 0 auto; }
      .side-note { display: none; }
      .topbar { min-height: 52px; padding: 0 18px; }
      .content { padding: 24px 18px 44px; }
      .metrics { grid-template-columns: repeat(2, minmax(0, 1fr)); }
      .conversation-layout, .evidence-grid { grid-template-columns: 1fr; }
      .session-list { display: flex; overflow: auto; border-right: 0; border-bottom: 1px solid var(--line); }
      .session-button { width: 180px; flex: 0 0 180px; }
      .artifact-view { border-top: 1px solid var(--line); border-left: 0; }
      .session-table-toolbar { align-items: stretch; flex-direction: column; }
      .session-table-controls { overflow-x: auto; }
      .session-table-search { width: 230px; flex: 0 0 230px; }
      .trend-legend { white-space: normal; }
    }
    @media (max-width: 560px) {
      .title-row, .section-heading { display: block; }
      .title-row .status { margin-top: 11px; }
      .metrics, .task-grid, .comparison { grid-template-columns: 1fr; }
      .task-card, .task-card:nth-child(2n), .task-card:nth-last-child(-n + 2) { border-right: 0; border-bottom: 1px solid var(--line); }
      .task-card:last-child { border-bottom: 0; }
    }
  </style>
</head>
<body>
  <div class="shell">
    <aside class="sidebar" id="viewer-sidebar">
      <div class="brand"><span class="brand-mark">C</span><span><span class="brand-title">Cori Benchmark</span><span class="brand-subtitle">Review workspace</span></span></div>
      <p class="nav-label">Run</p>
      <nav class="nav" id="run-nav"></nav>
      <p class="nav-label">Tasks</p>
      <nav class="nav" id="task-nav"></nav>
      <p class="nav-label">Evidence</p>
      <nav class="nav" id="artifact-nav"></nav>
      <div class="side-note">Portable review bundle<code id="side-run-id"></code></div>
    </aside>
    <button class="sidebar-toggle" id="sidebar-toggle" type="button" aria-controls="viewer-sidebar" aria-expanded="true" aria-label="Hide navigation" title="Hide navigation">×</button>
    <main>
      <header class="topbar"><span class="crumb" id="crumb">Benchmark review</span><span class="status neutral" id="top-status"></span></header>
      <div class="content" id="content"></div>
    </main>
  </div>
  <script id="benchmark-data" type="application/json">${safeJson(payload)}</script>
  <script>
    (() => {
      const data = JSON.parse(document.getElementById("benchmark-data").textContent);
      const result = data.result;
      const artifacts = data.artifacts;
      const sessions = data.sessions || [];
      const sessionRows = data.sessionRows || [];
      const content = document.getElementById("content");
      const crumb = document.getElementById("crumb");
      const topStatus = document.getElementById("top-status");
      const sidebarToggle = document.getElementById("sidebar-toggle");
      const runNav = document.getElementById("run-nav");
      const taskNav = document.getElementById("task-nav");
      const artifactNav = document.getElementById("artifact-nav");
      const taskIds = [...new Set([...(result.capture.tasks || []).map((task) => task.taskId), ...result.trials.map((trial) => trial.taskId)])];
      let selectedArtifact = null;

      document.getElementById("side-run-id").textContent = result.runId;
      topStatus.textContent = result.status;
      topStatus.className = "status " + statusClass(result.status === "succeeded" ? "ok" : "bad");

      function setSidebarHidden(hidden) {
        document.body.classList.toggle("sidebar-hidden", hidden);
        sidebarToggle.setAttribute("aria-expanded", String(!hidden));
        sidebarToggle.setAttribute("aria-label", hidden ? "Show navigation" : "Hide navigation");
        sidebarToggle.title = hidden ? "Show navigation" : "Hide navigation";
        sidebarToggle.textContent = hidden ? "☰" : "×";
      }

      function setViewerMode(mode) {
        const tableMode = mode === "table";
        document.body.classList.toggle("table-view", tableMode);
        content.classList.toggle("table-content", tableMode);
        setSidebarHidden(tableMode);
      }

      sidebarToggle.addEventListener("click", () => setSidebarHidden(!document.body.classList.contains("sidebar-hidden")));

      function el(tag, attrs = {}, children = []) {
        const node = document.createElement(tag);
        for (const [key, value] of Object.entries(attrs)) {
          if (value === undefined || value === null) continue;
          if (key === "class") node.className = value;
          else if (key === "text") node.textContent = value;
          else if (key.startsWith("on")) node.addEventListener(key.slice(2).toLowerCase(), value);
          else node.setAttribute(key, value);
        }
        for (const child of Array.isArray(children) ? children : [children]) {
          if (child !== undefined && child !== null) node.append(child);
        }
        return node;
      }

      function appendTextWithLinks(node, value) {
        const text = String(value ?? "");
        const urlPattern = /https?:\\/\\/[^\\s<>"'\\\\]+/gu;
        let offset = 0;
        for (const match of text.matchAll(urlPattern)) {
          const start = match.index ?? 0;
          const [linkText, suffix] = splitLinkSuffix(match[0]);
          node.append(document.createTextNode(text.slice(offset, start)));
          if (linkText) {
            const link = document.createElement("a");
            link.className = "external-link";
            link.href = linkText;
            link.rel = "noopener noreferrer";
            link.textContent = linkText;
            node.append(link);
          }
          node.append(document.createTextNode(suffix));
          offset = start + match[0].length;
        }
        node.append(document.createTextNode(text.slice(offset)));
      }

      function splitLinkSuffix(value) {
        let end = value.length;
        while (end > 0 && /[.,;:!?]/u.test(value[end - 1])) end -= 1;
        for (const [open, close] of [["(", ")"], ["[", "]"], ["{", "}"]]) {
          while (end > 0 && value[end - 1] === close && count(value.slice(0, end), close) > count(value.slice(0, end), open)) end -= 1;
        }
        return [value.slice(0, end), value.slice(end)];
      }

      function count(value, character) {
        return [...value].filter((item) => item === character).length;
      }

      function linkedText(tag, attrs, value) {
        const node = el(tag, attrs);
        appendTextWithLinks(node, value);
        return node;
      }

      function linkedPre(value) {
        return linkedText("pre", {}, value);
      }

      function navButton(label, onClick, active, dot = false, countValue = null) {
        return el("button", { class: "nav-button", "aria-current": active ? "page" : undefined, onClick }, [
          dot ? el("span", { class: "nav-dot" }) : null,
          el("span", { class: "nav-name", text: label }),
          countValue === null ? null : el("span", { class: "nav-count", text: String(countValue) }),
        ]);
      }

      function renderNav(active) {
        runNav.replaceChildren(
          navButton("Overview", () => renderOverview(), active === "overview", true),
          navButton("Session table", () => renderSessionTable(), active === "session-table", true, sessionRows.length),
          navButton("Agent exchange", () => renderConversations(), active === "conversations", true, sessions.length),
        );
        taskNav.replaceChildren(...taskIds.map((id) =>
          navButton(prettyTask(id), () => renderTask(id), active === "task:" + id, true),
        ));
        artifactNav.replaceChildren(
          navButton("All artifacts", () => renderArtifacts(), active === "artifacts", true, artifacts.length),
        );
      }

      function statusClass(status) {
        if (["succeeded", "passed", "ready", "ok"].includes(String(status).toLowerCase())) return "ok";
        if (["failed", "error", "unsafe"].includes(String(status).toLowerCase())) return "bad";
        return "warn";
      }

      function prettyTask(id) {
        return String(id).replaceAll("_", " ").replace(/\\b\\w/g, (letter) => letter.toUpperCase());
      }

      function formatNumber(value) {
        return value === null || value === undefined ? "—" : new Intl.NumberFormat("en-US").format(value);
      }

      function formatDuration(value) {
        if (value === null || value === undefined) return "—";
        const seconds = Number(value) / 1000;
        return seconds >= 60 ? Math.floor(seconds / 60) + "m " + Math.round(seconds % 60) + "s" : seconds.toFixed(seconds < 10 ? 1 : 0) + "s";
      }

      function gradeClass(score) {
        return score >= 100 ? "good" : score >= 90 ? "low" : "bad";
      }

      function gradeBadge(score) {
        return el("span", { class: "grade " + gradeClass(Number(score)), text: String(score) });
      }

      function metric(label, value, detail) {
        return el("div", { class: "metric" }, [
          el("div", { class: "metric-label", text: label }),
          el("div", { class: "metric-value", text: value }),
          el("div", { class: "metric-detail", text: detail || "" }),
        ]);
      }

      function title(eyebrow, heading, subtitle, status) {
        const headingBlock = el("div", {}, [
          el("p", { class: "eyebrow", text: eyebrow }),
          el("h1", { text: heading }),
          subtitle ? el("p", { class: "subtitle", text: subtitle }) : null,
        ]);
        return el("div", { class: "title-row" }, [headingBlock, status ? el("span", { class: "status " + statusClass(status), text: status }) : null]);
      }

      function renderOverview() {
        setViewerMode("standard");
        renderNav("overview");
        crumb.textContent = "Run overview";
        const direct = result.trials.filter((trial) => trial.lane === "direct");
        const replay = result.trials.filter((trial) => trial.lane === "replay");
        const qualified = (result.capture.tasks || []).filter((task) => task.qualificationPassed).length;
        content.replaceChildren(
          title("Benchmark run", result.runId, "A reviewable record of direct agent work, workflow capture, and unchanged Cori replays.", result.status),
          ...(result.error ? [el("div", { class: "notice", text: result.error })] : []),
          el("div", { class: "metrics" }, [
            metric("Direct score", scoreText(result.summary.directScore), direct.length + " trial" + plural(direct.length)),
            metric("Replay score", scoreText(result.summary.replayScore), replay.length + " trial" + plural(replay.length)),
            metric("Captured workflows", qualified + "/" + (result.capture.tasks || []).length, "qualified for replay"),
            metric("Break-even", result.metrics.breakEvenRepetitions === null ? "—" : result.metrics.breakEvenRepetitions + " runs", result.summary.reuseAdvantageDemonstrated ? "reuse advantage demonstrated" : "comparison is inconclusive"),
          ]),
          el("h2", { text: "Task review" }),
          taskPanel(),
          el("h2", { text: "Run context" }),
          environmentPanel(),
        );
      }

      function renderSessionTable() {
        setViewerMode("table");
        renderNav("session-table");
        content.replaceChildren(sessionTablePanel());
      }

      function sessionTablePanel() {
        let engineFilter = "all";
        let searchValue = "";
        const panel = el("section", { class: "session-table-panel", "aria-label": "All benchmark sessions" });
        const toolbar = el("div", { class: "session-table-toolbar" });
        const controls = el("div", { class: "session-table-controls" });
        const summary = el("span", { class: "session-table-summary" });
        const input = el("input", {
          class: "session-table-search",
          type: "search",
          placeholder: "Filter task, session, seed, or status",
          oninput: () => { searchValue = input.value.trim().toLowerCase(); draw(); },
        });
        controls.append(input);
        ["all", "agent", "cori"].forEach((engine) => {
          const button = el("button", {
            class: "session-filter",
            "aria-pressed": String(engineFilter === engine),
            onClick: () => {
              engineFilter = engine;
              controls.querySelectorAll(".session-filter").forEach((item) => item.setAttribute("aria-pressed", String(item === button)));
              draw();
            },
            text: engine === "all" ? "All sessions" : engine === "agent" ? "Agent only" : "Cori only",
          });
          controls.append(button);
        });
        toolbar.append(controls, el("div", {}, [summary, el("div", { class: "trend-legend", text: "↗ / ↘ vs previous session in the same track · paired Δ vs direct baseline" })]));

        const scroll = el("div", { class: "session-table-scroll" });
        const table = el("table", { class: "session-table" });
        const headers = [
          ["#", "sticky-order"], ["Session", "sticky-session"], ["Started", ""], ["Task", ""], ["Engine", ""],
          ["Attempt / seed", ""], ["Score", "number-cell"], ["Status", ""], ["Duration", "number-cell"],
          ["Tools / activities", "number-cell"], ["CLI", "number-cell"], ["LLM", "number-cell"], ["Code", "number-cell"],
          ["Messages", "number-cell"], ["Events", "number-cell"], ["Input tokens", "number-cell"],
          ["Output tokens", "number-cell"], ["Total tokens", "number-cell"], ["Tokens / sec", "number-cell"],
          ["Price USD", "number-cell"], ["Paired Δ score", "pair-cell"], ["Paired Δ time", "pair-cell"],
          ["Paired Δ tokens", "pair-cell"], ["Evidence", ""],
        ];
        const thead = el("thead", {}, [el("tr", {}, headers.map(([label, className]) => el("th", { class: className, text: label })))]);
        const tbody = el("tbody");
        table.append(thead, tbody);
        scroll.append(table);
        panel.append(toolbar, scroll);

        function draw() {
          const visible = sessionRows.filter((row) => {
            if (engineFilter !== "all" && row.engine !== engineFilter) return false;
            if (!searchValue) return true;
            return [row.taskId, row.label, row.status, row.seed, row.attempt, row.workflowHash]
              .some((value) => String(value ?? "").toLowerCase().includes(searchValue));
          });
          summary.textContent = visible.length + " of " + sessionRows.length + " sessions";
          tbody.replaceChildren(...visible.map(sessionTableRow));
          if (!visible.length) tbody.append(el("tr", {}, [el("td", { class: "session-table-empty", colspan: String(headers.length), text: "No sessions match this filter." })]));
        }

        draw();
        return panel;
      }

      function sessionTableRow(row) {
        const order = sessionRows.indexOf(row) + 1;
        const started = tableDate(row.startedAt);
        const attemptSeed = [row.attempt === null ? null : "attempt " + row.attempt, row.seed === null ? null : "seed " + row.seed].filter(Boolean);
        const evidence = artifacts.find((artifact) => artifact.path === row.evidencePath);
        const href = evidence?.href || "./" + row.evidencePath.split("/").map(encodeURIComponent).join("/");
        return el("tr", { "data-engine": row.engine }, [
          el("td", { class: "sticky-order", text: String(order) }),
          el("td", { class: "sticky-session" }, [
            el("span", { class: "session-primary", text: row.label }),
            el("span", { class: "session-secondary", text: row.id }),
          ]),
          el("td", {}, [el("span", { class: "session-primary", text: started.time }), el("span", { class: "session-secondary", text: started.date })]),
          el("td", {}, [el("span", { class: "session-primary", text: prettyTask(row.taskId) }), el("span", { class: "session-secondary", text: row.track })]),
          el("td", {}, [el("span", { class: "engine-pill " + row.engine, text: row.engine })]),
          el("td", {}, attemptSeed.length ? attemptSeed.map((value) => el("span", { class: "session-secondary", text: value })) : [el("span", { class: "small", text: "—" })]),
          numericTrendCell(row, "score", scoreText),
          el("td", {}, [el("span", { class: "table-status " + statusClass(row.status), text: row.status })]),
          numericTrendCell(row, "durationMs", formatDuration),
          numericTrendCell(row, "toolCalls", formatNumber),
          numericTrendCell(row, "cliCalls", formatNumber),
          numericTrendCell(row, "llmCalls", formatNumber),
          numericTrendCell(row, "codeCalls", formatNumber),
          numericTrendCell(row, "messages", formatNumber),
          numericTrendCell(row, "events", formatNumber),
          numericTrendCell(row, "inputTokens", formatShortNumber),
          numericTrendCell(row, "outputTokens", formatShortNumber),
          numericTrendCell(row, "totalTokens", formatShortNumber),
          numericTrendCell(row, "tokensPerSecond", formatRate),
          numericTrendCell(row, "priceUsd", formatPrice),
          pairedDeltaCell(row, "score", scoreText),
          pairedDeltaCell(row, "durationMs", formatDuration),
          pairedDeltaCell(row, "totalTokens", formatShortNumber),
          el("td", {}, [
            el("a", { class: "session-evidence", href, text: "Open evidence" }),
            row.workflowHash ? el("span", { class: "session-secondary", text: "workflow " + row.workflowHash.slice(0, 10) }) : null,
          ]),
        ]);
      }

      function numericTrendCell(row, field, formatter) {
        const value = row[field];
        if (value === null || value === undefined) return el("td", { class: "number-cell" }, [el("span", { class: "number-main", text: "—" })]);
        const previous = previousComparable(row, field);
        const children = [el("span", { class: "number-main", text: formatter(value) })];
        if (!previous) children.push(el("span", { class: "trend first", text: "• first in track" }));
        else children.push(trendMarker(value, previous[field], previous.label));
        return el("td", { class: "number-cell" }, children);
      }

      function previousComparable(row, field) {
        for (let index = sessionRows.indexOf(row) - 1; index >= 0; index -= 1) {
          const candidate = sessionRows[index];
          if (candidate.taskId === row.taskId && candidate.track === row.track && candidate[field] !== null && candidate[field] !== undefined) return candidate;
        }
        return null;
      }

      function trendMarker(value, previousValue, previousLabel) {
        const delta = Number(value) - Number(previousValue);
        const percent = Number(previousValue) === 0 ? null : Math.abs(delta / Number(previousValue) * 100);
        const direction = delta > 0 ? "up" : delta < 0 ? "down" : "flat";
        const arrow = delta > 0 ? "↗" : delta < 0 ? "↘" : "→";
        const change = percent === null ? signedCompact(delta) : percent.toFixed(percent >= 10 ? 0 : 1) + "%";
        return el("span", {
          class: "trend " + direction,
          title: "Compared with " + previousLabel + ": " + signedCompact(delta),
          text: arrow + " " + change,
        });
      }

      function pairedDeltaCell(row, field, formatter) {
        if (!row.pairId) return el("td", { class: "pair-cell" }, [el("span", { class: "pair-baseline", text: "—" })]);
        if (row.engine === "agent") return el("td", { class: "pair-cell" }, [el("span", { class: "pair-baseline", text: "baseline" })]);
        const baseline = sessionRows.find((candidate) => candidate.pairId === row.pairId && candidate.engine === "agent");
        const value = row[field];
        const baselineValue = baseline?.[field];
        if (value === null || value === undefined || baselineValue === null || baselineValue === undefined) {
          return el("td", { class: "pair-cell" }, [el("span", { class: "pair-baseline", text: "—" })]);
        }
        const delta = Number(value) - Number(baselineValue);
        const percent = Number(baselineValue) === 0 ? null : delta / Number(baselineValue) * 100;
        const direction = delta > 0 ? "up" : delta < 0 ? "down" : "flat";
        const arrow = delta > 0 ? "↗" : delta < 0 ? "↘" : "→";
        return el("td", { class: "pair-cell" }, [
          el("span", { class: "number-main", text: arrow + " " + (percent === null ? signedCompact(delta) : Math.abs(percent).toFixed(Math.abs(percent) >= 10 ? 0 : 1) + "%") }),
          el("span", { class: "trend " + direction, text: (delta >= 0 ? "+" : "−") + formatter(Math.abs(delta)) }),
        ]);
      }

      function tableDate(value) {
        if (!value) return { date: "time unavailable", time: "—" };
        const date = new Date(value);
        if (Number.isNaN(date.valueOf())) return { date: String(value), time: "—" };
        return {
          date: date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "2-digit" }),
          time: date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }),
        };
      }

      function formatShortNumber(value) {
        if (value === null || value === undefined) return "—";
        const number = Number(value);
        if (Math.abs(number) >= 1_000_000) return (number / 1_000_000).toFixed(2) + "m";
        if (Math.abs(number) >= 1_000) return (number / 1_000).toFixed(number >= 100_000 ? 0 : 1) + "k";
        return formatNumber(number);
      }

      function formatRate(value) { return value === null || value === undefined ? "—" : Number(value).toFixed(Number(value) >= 100 ? 0 : 1); }
      function formatPrice(value) { return value === null || value === undefined ? "—" : "$" + Number(value).toFixed(4); }
      function signedCompact(value) { return (Number(value) > 0 ? "+" : "") + formatShortNumber(Number(value)); }

      function taskPanel() {
        const panel = el("section", { class: "panel" });
        panel.append(el("div", { class: "panel-head" }, [
          el("h3", { text: "Each task keeps its evidence together" }),
          el("span", { class: "small", text: taskIds.length + " task" + plural(taskIds.length) }),
        ]));
        const grid = el("div", { class: "task-grid" });
        for (const taskId of taskIds) {
          const capture = (result.capture.tasks || []).find((item) => item.taskId === taskId);
          const direct = result.trials.filter((trial) => trial.taskId === taskId && trial.lane === "direct");
          const replay = result.trials.filter((trial) => trial.taskId === taskId && trial.lane === "replay");
          const directScore = mean(direct.map((trial) => trial.grade.score));
          const replayScore = mean(replay.map((trial) => trial.grade.score));
          const card = el("button", { class: "task-card", onClick: () => renderTask(taskId) }, [
            el("div", { class: "task-title" }, [el("span", { class: "task-id", text: prettyTask(taskId) }), capture ? gradeBadge(capture.authorGrade.score) : null]),
            el("div", { class: "task-meta" }, [
              el("span", { text: "Direct " + scoreText(directScore) }),
              el("span", { text: "Replay " + scoreText(replayScore) }),
              el("span", { text: capture?.qualificationPassed ? "qualified" : "review" }),
            ]),
          ]);
          grid.append(card);
        }
        panel.append(grid);
        return panel;
      }

      function environmentPanel() {
        const panel = el("section", { class: "panel" });
        const rows = [
          ["Profile", result.profile], ["Harness", result.harness], ["Seed", String(result.seed)],
          ["Started", formatDate(result.startedAt)], ["Finished", formatDate(result.finishedAt)],
          ...Object.entries(result.environment || {}).map(([key, value]) => [key.replaceAll("_", " "), value || "—"]),
        ];
        panel.append(el("div", { class: "rubric" }, rows.map(([label, value]) =>
          el("div", { class: "rubric-item" }, [el("div", { class: "rubric-score", text: label }), el("div", { class: "rubric-title", text: String(value) })]),
        )));
        return panel;
      }

      function renderTask(taskId, tab = "summary") {
        setViewerMode("standard");
        renderNav("task:" + taskId);
        crumb.textContent = prettyTask(taskId);
        const capture = (result.capture.tasks || []).find((item) => item.taskId === taskId);
        const direct = result.trials.filter((trial) => trial.taskId === taskId && trial.lane === "direct");
        const replay = result.trials.filter((trial) => trial.taskId === taskId && trial.lane === "replay");
        const tabs = [
          ["summary", "Review"], ["conversation", "Conversation"], ["evidence", "Evidence"], ["workflow", "Workflow"],
        ];
        const tabBar = el("div", { class: "tabs", role: "tablist" }, tabs.map(([key, label]) =>
          el("button", { class: "tab", role: "tab", "aria-selected": String(tab === key), onClick: () => renderTask(taskId, key), text: label }),
        ));
        const intro = el("div", { class: "task-intro" }, [
          el("button", { class: "back", onClick: () => renderOverview(), text: "← All tasks" }),
          el("p", { class: "eyebrow", text: "Task review" }),
          el("h1", { text: prettyTask(taskId) }),
          el("div", { class: "task-facts" }, [
            el("span", { class: "fact", text: "author score " + (capture ? capture.authorGrade.score : "—") }),
            el("span", { class: "fact", text: capture?.qualificationPassed ? "qualification passed" : "qualification needs review" }),
            el("span", { class: "fact", text: direct.length + " direct / " + replay.length + " replay" }),
          ]),
        ]);
        content.replaceChildren(intro, tabBar, taskTab(taskId, tab, capture, direct, replay));
      }

      function taskTab(taskId, tab, capture, direct, replay) {
        if (tab === "conversation") return conversationPanel(taskId);
        if (tab === "evidence") return evidencePanel(taskId, capture, direct, replay);
        if (tab === "workflow") return workflowPanel(taskId, capture);
        return summaryPanel(taskId, capture, direct, replay);
      }

      function summaryPanel(taskId, capture, direct, replay) {
        const node = el("div");
        if (capture?.error) node.append(el("div", { class: "notice", text: capture.error }));
        node.append(el("div", { class: "comparison" }, [lanePanel("Direct agent", direct), lanePanel("Cori replay", replay)]));
        if (capture) {
          node.append(el("h2", { text: "Capture gates" }), capturePanel(capture));
        }
        node.append(el("h2", { text: "Scoring evidence" }));
        const grades = [...direct, ...replay].map((trial) => gradePanel(trial));
        node.append(...(grades.length ? grades : [el("div", { class: "empty", text: "No held-out trial was recorded for this task." })]));
        return node;
      }

      function lanePanel(titleText, trials) {
        const lane = el("section", { class: "lane" });
        lane.append(el("div", { class: "lane-title" }, [el("span", { text: titleText }), gradeBadge(scoreText(mean(trials.map((trial) => trial.grade.score))))]));
        if (!trials.length) {
          lane.append(el("div", { class: "small", text: "No recorded trial" }));
          return lane;
        }
        trials.forEach((trial) => lane.append(el("div", { class: "lane-row" }, [
          el("span", { text: "Seed " + trial.seed }),
          el("b", { text: trial.grade.score + "/100" }),
          el("span", { text: formatDuration(trial.harness?.wallTimeMs ?? trial.runtime?.wallTimeMs) }),
        ])));
        return lane;
      }

      function capturePanel(capture) {
        const panel = el("section", { class: "panel" });
        const items = [
          ["Preview", capture.previewDidNotWrite ? "passed — no workflow write" : "failed"],
          ["Static check", capture.checkPassed ? "passed" : "failed"],
          ["Policy", capture.policy?.ok ? "passed" : capture.policy ? capture.policy.violations.join("; ") : "not recorded"],
          ["Qualification", capture.qualificationPassed ? "passed at 100/100" : "not passed"],
          ["Attempts", String((capture.attempts || []).length || 1) + (capture.selectedAttempt ? "; selected #" + capture.selectedAttempt : "")],
        ];
        panel.append(el("div", { class: "rubric" }, items.map(([label, value]) =>
          el("div", { class: "rubric-item" }, [el("div", { class: "rubric-score", text: label }), el("div", { class: "rubric-title", text: value })]),
        )));
        return panel;
      }

      function gradePanel(trial) {
        const panel = el("section", { class: "panel" });
        panel.append(el("div", { class: "panel-head" }, [
          el("h3", { text: (trial.lane === "direct" ? "Direct agent" : "Cori replay") + " · seed " + trial.seed }),
          gradeBadge(trial.grade.score),
        ]));
        const list = el("div", { class: "rubric" });
        const items = trial.grade.items || [];
        if (!items.length) list.append(el("div", { class: "small", text: "No rubric rows were recorded." }));
        items.forEach((item) => list.append(el("div", { class: "rubric-item" }, [
          el("div", { class: "rubric-score", text: item.earned + "/" + item.max }),
          el("div", {}, [el("div", { class: "rubric-title", text: item.id }), el("div", { class: "rubric-note", text: item.note })]),
        ])));
        (trial.grade.safetyViolations || []).forEach((violation) => list.append(el("div", { class: "notice", text: "Safety: " + violation })));
        panel.append(list);
        return panel;
      }

      function conversationPanel(taskId) {
        const taskSessions = sessions.filter((session) => session.taskId === taskId);
        if (!taskSessions.length) return el("div", { class: "empty", text: "No harness transcript is available for this task." });
        return conversationBrowser(taskSessions);
      }

      function renderConversations() {
        setViewerMode("standard");
        renderNav("conversations");
        crumb.textContent = "Agent exchange";
        const taskCount = new Set(sessions.map((session) => session.taskId)).size;
        content.replaceChildren(
          title("Agent exchange", "Benchmark conversations", "Review the direct agent work and every workflow-capture turn as a structured conversation. Tool calls keep their output available without overwhelming the main exchange.", null),
          el("div", { class: "metrics" }, [
            metric("Sessions", String(sessions.length), "recorded conversations"),
            metric("Tasks", String(taskCount), "with conversation evidence"),
            metric("Messages", formatNumber(sessions.reduce((sum, session) => sum + session.messages.length, 0)), "normalized events"),
            metric("Transcripts", String(artifacts.filter((artifact) => artifact.kind === "transcript").length), "raw files preserved"),
          ]),
          sessions.length ? conversationBrowser(sessions, true) : el("div", { class: "empty", text: "No harness transcripts were preserved for this run." }),
        );
      }

      function conversationBrowser(availableSessions, showTask = false) {
        const root = el("section", { class: "conversation-layout" });
        const list = el("div", { class: "session-list" });
        const chat = el("div", { class: "chat" });
        function show(session, button) {
          list.querySelectorAll("button").forEach((item) => item.removeAttribute("aria-current"));
          button.setAttribute("aria-current", "true");
          chat.replaceChildren(chatView(session));
        }
        availableSessions.forEach((session, index) => {
          const button = el("button", { class: "session-button", onClick: () => show(session, button) }, [
            el("span", { class: "session-name", text: session.name }),
            el("span", { class: "session-meta", text: session.meta }),
            showTask ? el("span", { class: "session-task", text: prettyTask(session.taskId) }) : null,
          ]);
          list.append(button);
          if (index === 0) show(session, button);
        });
        root.append(list, chat);
        return root;
      }

      function chatView(session) {
        const root = el("div");
        root.append(
          el("div", { class: "chat-head" }, [el("div", {}, [el("p", { class: "eyebrow", text: prettyTask(session.taskId) }), el("h2", { text: session.name })]), el("span", { text: session.meta })]),
          artifactFileLink(session.sourcePath, "Open raw transcript"),
        );
        if (!session.messages.length) {
          root.append(el("div", { class: "empty", text: "No conversational content could be normalized from this transcript. Open the raw transcript for the complete event stream." }));
        } else {
          const list = el("div", { class: "message-list" });
          session.messages.forEach((message) => list.append(chatMessage(message)));
          root.append(list);
        }
        return root;
      }

      function chatMessage(message) {
        const block = el("article", { class: "message " + message.role });
        block.append(el("div", { class: "message-head" }, [el("span", { text: message.label }), el("span", { text: message.status || ("event " + message.index) })]));
        if (message.text) block.append(linkedText("p", { class: "message-text" }, message.text));
        if (message.detail) {
          const details = el("details", {}, [el("summary", { text: "Show tool output" })]);
          details.append(linkedPre(message.detail));
          block.append(details);
        }
        return block;
      }

      function evidencePanel(taskId, capture, direct, replay) {
        const taskArtifacts = artifacts.filter((artifact) => artifact.path.includes(taskId) || artifact.path === "result.json");
        const workspaceLinks = uniqueLinks(taskArtifacts.flatMap((artifact) => artifact.links || []))
          .filter((link) => link.label.startsWith("Open "));
        const root = el("div");
        root.append(el("div", { class: "comparison" }, [
          lanePanel("Direct snapshots and grades", direct), lanePanel("Replay snapshots and trace", replay),
        ]));
        if (capture?.qualificationGrade) {
          root.append(el("h2", { text: "Qualification grading" }), gradePanel({ lane: "qualification", seed: capture.selectedAttempt || 1, grade: capture.qualificationGrade }));
        }
        if (workspaceLinks.length) {
          root.append(el("h2", { text: "Workspace resources" }), resourceLinkPanel(workspaceLinks));
        }
        root.append(el("h2", { text: "Files for this task" }), artifactBrowser(taskArtifacts));
        return root;
      }

      function workflowPanel(taskId, capture) {
        const files = artifacts.filter((artifact) => artifact.kind === "workflow" && (artifact.path.includes("/" + taskId + "/") || artifact.path.includes("/" + taskId + "-")));
        const root = el("div");
        if (capture?.policy) {
          root.append(el("section", { class: "panel" }, [
            el("div", { class: "panel-head" }, [el("h3", { text: "Workflow policy" }), el("span", { class: "status " + statusClass(capture.policy.ok ? "ok" : "failed"), text: capture.policy.ok ? "passed" : "failed" })]),
            el("div", { class: "rubric" }, [
              el("div", { class: "rubric-item" }, [el("div", { class: "rubric-score", text: "hash" }), el("div", { class: "rubric-title", text: capture.policy.workflowHash || "—" })]),
              ...(capture.policy.violations || []).map((item) => el("div", { class: "notice", text: item })),
            ]),
          ]));
        }
        root.append(el("h2", { text: "Captured workflow files" }));
        if (!files.length) root.append(el("div", { class: "empty", text: "No captured workflow files were preserved for this task." }));
        else {
          const tree = el("div", { class: "file-tree" });
          files.forEach((artifact) => {
            const file = el("details", { class: "workflow-file" }, [el("summary", { text: artifact.path })]);
            file.append(artifact.content ? linkedPre(artifact.content) : artifactFileLink(artifact.path, "Open workflow file"));
            tree.append(file);
          });
          root.append(tree);
        }
        return root;
      }

      function renderArtifacts() {
        setViewerMode("standard");
        renderNav("artifacts");
        crumb.textContent = "All artifacts";
        content.replaceChildren(
          title("Raw evidence", "All benchmark artifacts", "Browse the complete run bundle in review order. Small files are embedded, large evidence includes a readable preview, and every original file remains one click away.", null),
          artifactBrowser(artifacts, true),
        );
      }

      function artifactBrowser(files, showSearch = false) {
        let visible = files;
        let selectedKind = "all";
        let searchNeedle = "";
        let selected = selectedArtifact && files.includes(selectedArtifact) ? selectedArtifact : files[0] || null;
        const root = el("section", { class: "evidence-grid panel" });
        const listSide = el("div");
        const list = el("div", { class: "evidence-list" });
        const viewer = el("div", { class: "artifact-view" });
        const summary = el("div", { class: "artifact-list-summary" });
        function applyFilters() {
          visible = files.filter((artifact) =>
            (selectedKind === "all" || artifact.kind === selectedKind)
            && (!searchNeedle || (artifact.path + " " + artifact.kind).toLowerCase().includes(searchNeedle))
          );
          if (!visible.includes(selected)) selected = visible[0] || null;
          summary.textContent = visible.length + " of " + files.length + " artifact" + plural(files.length);
          drawList();
          drawViewer();
        }
        function drawList() {
          list.replaceChildren(...visible.map((artifact) => {
            const button = el("button", { class: "artifact-button", "aria-current": selected === artifact ? "true" : undefined, onClick: () => { selected = artifact; selectedArtifact = artifact; drawList(); drawViewer(); } }, [
              el("span", { class: "artifact-kind", text: artifact.kind }),
              el("span", { class: "artifact-path", text: artifact.path }),
            ]);
            return button;
          }));
          if (!visible.length) list.append(el("div", { class: "empty", text: "No matching artifacts" }));
        }
        function drawViewer() {
          viewer.replaceChildren();
          if (!selected) { viewer.append(el("div", { class: "empty", text: "Choose an artifact to inspect it." })); return; }
          viewer.append(el("h3", { text: selected.path }), artifactContent(selected));
        }
        if (showSearch) {
          const input = el("input", { class: "artifact-filter", type: "search", placeholder: "Filter by file name or evidence type", oninput: () => {
            searchNeedle = input.value.trim().toLowerCase();
            applyFilters();
          }});
          const counts = new Map();
          files.forEach((artifact) => counts.set(artifact.kind, (counts.get(artifact.kind) || 0) + 1));
          const kinds = ["all", ...counts.keys()];
          const filters = el("div", { class: "artifact-filters" });
          kinds.forEach((kind) => {
            const button = el("button", { class: "artifact-filter-button", "aria-pressed": String(selectedKind === kind), onClick: () => {
              selectedKind = kind;
              filters.querySelectorAll("button").forEach((item) => item.setAttribute("aria-pressed", String(item === button)));
              applyFilters();
            } }, [
              el("span", { text: kind === "all" ? "All" : kind }),
              el("b", { text: String(kind === "all" ? files.length : counts.get(kind)) }),
            ]);
            filters.append(button);
          });
          listSide.append(el("div", { class: "panel-head" }, [el("div", { class: "artifact-toolbar" }, [input, filters])]));
        }
        listSide.append(summary, list);
        root.append(listSide, viewer);
        applyFilters();
        return root;
      }

      function artifactContent(artifact) {
        const wrapper = el("div");
        wrapper.append(el("div", { class: "artifact-actions" }, [
          artifactFileLink(artifact.path, "Open raw file"),
          el("span", { class: "small", text: formatBytes(artifact.size) + " · " + artifact.kind }),
        ]));
        if (artifact.links?.length) {
          wrapper.append(el("h3", { text: "Links found in this artifact · " + artifact.links.length }));
          if (artifact.links.length > 8) {
            const linkDetails = el("details", {}, [el("summary", { text: "Show extracted links" })]);
            linkDetails.append(resourceLinkPanel(artifact.links));
            wrapper.append(linkDetails);
          } else {
            wrapper.append(resourceLinkPanel(artifact.links));
          }
        }
        if (artifact.content) {
          let parsed = null;
          try { parsed = JSON.parse(artifact.content); } catch {}
          wrapper.append(linkedPre(parsed ? prettyJson(parsed) : artifact.content));
          if (artifact.contentTruncated) wrapper.append(el("p", { class: "small", text: "Preview truncated for performance. Open the raw file above to inspect the complete artifact." }));
        } else {
          wrapper.append(el("div", { class: "empty", text: artifact.kind === "transcript"
            ? "This transcript was normalized into the task Conversation view. Open the raw file for its complete event stream."
            : "Large raw evidence stays in its original file so this review page remains fast."
          }));
        }
        return wrapper;
      }

      function resourceLinkPanel(links) {
        return el("div", { class: "resource-links" }, links.map((link) =>
          el("a", { class: "resource-link", href: link.url, rel: "noopener noreferrer", text: link.label + " · " + link.url }),
        ));
      }

      function uniqueLinks(links) {
        return [...new Map(links.map((link) => [link.url, link])).values()];
      }

      function artifactFileLink(path, label) {
        const artifact = artifacts.find((item) => item.path === path);
        const href = artifact?.href || "./" + path.split("/").map(encodeURIComponent).join("/");
        return el("a", { class: "artifact-open", href, text: label });
      }

      function formatBytes(value) {
        const bytes = Number(value || 0);
        if (bytes < 1024) return bytes + " B";
        if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
        return (bytes / (1024 * 1024)).toFixed(1) + " MB";
      }

      function mean(values) {
        return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null;
      }

      function scoreText(value) {
        return value === null || value === undefined ? "—" : (Number.isInteger(value) ? String(value) : Number(value).toFixed(1));
      }

      function plural(value) { return value === 1 ? "" : "s"; }
      function formatDate(value) { const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : date.toLocaleString(); }
      function prettyJson(value) { try { return typeof value === "string" ? value : JSON.stringify(value, null, 2); } catch { return String(value); } }

      renderOverview();
    })();
  </script>
</body>
</html>`;
}

/** Write a reviewable HTML bundle beside a run's other output artifacts. */
export async function writeBenchmarkViewer(
  runId: string,
  artifactsRoot = "artifacts",
): Promise<string> {
  return writeBenchmarkViewerForRun(resolve(artifactsRoot, runId));
}

/** Regenerate the review page after any command changes a run's artifacts. */
export async function writeBenchmarkViewerForRun(runDir: string): Promise<string> {
  const result = await readJson<BenchmarkResultV1>(join(runDir, "result.json"));
  const artifacts = await collectArtifacts(runDir);
  const destination = join(runDir, "viewer.html");
  await writeFile(destination, benchmarkViewerDocument(result, artifacts), "utf8");
  return destination;
}

async function collectArtifacts(runDir: string): Promise<ViewerArtifact[]> {
  const paths = await walk(runDir);
  const artifacts: ViewerArtifact[] = [];
  for (const path of paths) {
    const relativePath = relative(runDir, path).replaceAll("\\", "/");
    // Re-running `benchmark view` must not recursively embed earlier viewers.
    if (relativePath === "viewer.html") continue;
    const metadata = await stat(path);
    artifacts.push({
      path: relativePath,
      content: await readFile(path, "utf8"),
      kind: classifyArtifact(relativePath),
      size: metadata.size,
      modifiedAt: metadata.mtime.toISOString(),
    });
  }
  return artifacts.sort((left, right) => left.path.localeCompare(right.path));
}

async function walk(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await walk(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function classifyArtifact(path: string): ArtifactKind {
  if (path === "result.json") return "result";
  if (["progress.json", "cleanup-registry.json", "preflight.json"].includes(path)) return "run";
  if (path.startsWith("transcripts/")) return "transcript";
  if (path.startsWith("snapshots/")) return "snapshot";
  if (path.startsWith("author-grades/")) return "grade";
  if (path.startsWith("qualification/") && /-(before|after)\.json$/u.test(path)) return "snapshot";
  if (path.startsWith("cori-traces/") || path.startsWith("qualification/")) return "trace";
  if (path.startsWith("generated-workflows/")) return "workflow";
  if (path.startsWith("cori-check/")) return "check";
  if (path === "scorecard.md" || path === "results.csv") return "report";
  if (path.startsWith("agent-workspace/")) return "workspace";
  return "other";
}

const embeddedArtifactLimit = 128 * 1024;
const artifactPreviewLimit = 12 * 1024;
const viewerMessageLimit = 12_000;
const viewerSessionLimit = 400;
const viewerLinkLimit = 40;

function compactResult(result: BenchmarkResultV1): BenchmarkResultV1 {
  return {
    ...result,
    trials: result.trials.map((trial) => trial.harness
      ? {
          ...trial,
          harness: {
            ...trial.harness,
            transcript: [],
            stdout: "",
            stderr: "",
          },
        }
      : trial),
  };
}

function prepareViewerArtifact(artifact: ViewerArtifact): ViewerArtifact {
  const rawContent = artifact.content ?? "";
  const size = artifact.size ?? Buffer.byteLength(rawContent, "utf8");
  const embedContent = rawContent.length > 0
    && size <= embeddedArtifactLimit
    && artifact.kind !== "transcript";
  const previewContent = rawContent.length > 0
    && !embedContent
    && artifact.kind !== "transcript"
    ? rawContent.slice(0, artifactPreviewLimit)
    : undefined;
  return {
    path: artifact.path,
    kind: artifact.kind,
    size,
    modifiedAt: artifact.modifiedAt,
    href: artifact.href ?? artifactHref(artifact.path),
    links: artifact.links ?? extractViewerLinks(rawContent),
    ...(embedContent ? { content: rawContent } : previewContent
      ? { content: previewContent, contentTruncated: true }
      : {}),
  };
}

const artifactKindOrder: readonly ArtifactKind[] = [
  "result",
  "report",
  "run",
  "transcript",
  "grade",
  "snapshot",
  "workflow",
  "check",
  "trace",
  "workspace",
  "other",
];

function compareViewerArtifacts(left: ViewerArtifact, right: ViewerArtifact): number {
  const kindDifference = artifactKindOrder.indexOf(left.kind) - artifactKindOrder.indexOf(right.kind);
  if (kindDifference) return kindDifference;
  const leftSequence = artifactPathSequence(left.path);
  const rightSequence = artifactPathSequence(right.path);
  return leftSequence.base.localeCompare(rightSequence.base, undefined, { numeric: true })
    || leftSequence.order - rightSequence.order
    || left.path.localeCompare(right.path, undefined, { numeric: true });
}

function artifactPathSequence(path: string): { base: string; order: number } {
  const transcript = path.match(/^(.*?)(?:-attempt-(\d+))?-(direct|capture-preview|capture-approval)\.json$/u);
  if (transcript) {
    const attempt = Number(transcript[2] ?? "1");
    const phase = transcript[3] === "direct" ? 0 : transcript[3] === "capture-preview" ? 1 : 2;
    return { base: transcript[1] ?? path, order: attempt * 10 + phase };
  }
  const snapshot = path.match(/^(.*)-(before|after)\.json$/u);
  if (snapshot) return { base: snapshot[1] ?? path, order: snapshot[2] === "before" ? 0 : 1 };
  return { base: path, order: 0 };
}

function artifactHref(path: string): string {
  return `./${path.split("/").map(encodeURIComponent).join("/")}`;
}

function buildViewerSessions(
  result: BenchmarkResultV1,
  artifacts: readonly ViewerArtifact[],
): ViewerSession[] {
  const taskIds = [...new Set([
    ...result.capture.tasks.map((task) => task.taskId),
    ...result.trials.map((trial) => trial.taskId),
  ])];
  const sessions: ViewerSession[] = [];

  for (const artifact of artifacts) {
    if (artifact.kind !== "transcript" || !artifact.content) continue;
    const taskId = taskIds.find((id) => artifact.path.includes(id));
    if (!taskId) continue;
    const envelope = parseJsonValue(artifact.content);
    const events = transcriptEvents(artifact.content);
    const recordedPrompt = isRecord(envelope) && typeof envelope.prompt === "string"
      ? envelope.prompt
      : null;
    const fallbackPrompt = recordedPrompt ? null : fallbackSessionPrompt(artifact.path, taskId, artifacts);
    const messages = prependViewerPrompt(
      normalizeTranscript(events),
      recordedPrompt ?? fallbackPrompt,
      recordedPrompt ? "Recorded prompt" : "Review context",
    );
    sessions.push({
      taskId,
      name: viewerSessionName(artifact.path, taskId),
      meta: `${messages.length} message${messages.length === 1 ? "" : "s"} · ${events.length} event${events.length === 1 ? "" : "s"}`,
      messages,
      sourcePath: artifact.path,
    });
  }

  for (const trial of result.trials) {
    const transcript = trial.harness?.transcript;
    if (trial.lane !== "direct" || !transcript?.length) continue;
    const messages = prependViewerPrompt(
      normalizeTranscript(transcript),
      trial.harness?.prompt ?? null,
      "Recorded prompt",
    );
    sessions.push({
      taskId: trial.taskId,
      name: `Held-out direct · seed ${trial.seed}`,
      meta: `${messages.length} message${messages.length === 1 ? "" : "s"} · ${transcript.length} event${transcript.length === 1 ? "" : "s"}`,
      messages,
      sourcePath: "result.json",
    });
  }

  return sessions.sort(compareViewerSessions);
}

function buildViewerSessionComparisons(
  result: BenchmarkResultV1,
  artifacts: readonly ViewerArtifact[],
): ViewerSessionComparison[] {
  const rows: ViewerSessionComparison[] = [];
  const taskIds = [...new Set([
    ...result.capture.tasks.map((task) => task.taskId),
    ...result.trials.map((trial) => trial.taskId),
  ])];

  for (const artifact of artifacts) {
    if (artifact.kind !== "transcript" || !artifact.content || !artifact.path.startsWith("transcripts/authors/")) continue;
    const taskId = taskIds.find((id) => artifact.path.includes(id));
    if (!taskId) continue;
    const envelope = parseJsonValue(artifact.content);
    if (!isRecord(envelope)) continue;
    const capture = result.capture.tasks.find((task) => task.taskId === taskId);
    const attempt = Number(artifact.path.match(/-attempt-(\d+)-/u)?.[1] ?? "1");
    const attemptRecord = capture?.attempts?.find((item) => item.attempt === attempt);
    const phase = artifact.path.includes("capture-preview")
      ? "preview"
      : artifact.path.includes("capture-approval") ? "approval" : "direct";
    const events = transcriptEvents(artifact.content);
    const normalized = normalizeTranscript(events);
    const durationMs = numberOrNull(envelope.wallTimeMs);
    const usage = isRecord(envelope.usage) ? envelope.usage : {};
    const inputTokens = numberOrNull(usage.inputTokens ?? usage.input_tokens);
    const outputTokens = numberOrNull(usage.outputTokens ?? usage.output_tokens);
    const totals = viewerTokenTotals(inputTokens, outputTokens, durationMs);
    const exitCode = numberOrNull(envelope.exitCode ?? envelope.exit_code);
    const sessionId = typeof envelope.sessionId === "string"
      ? envelope.sessionId
      : typeof envelope.session_id === "string" ? envelope.session_id : null;
    rows.push({
      id: `${sessionId ?? artifact.path}:${phase}:attempt-${attempt}`,
      taskId,
      label: viewerSessionName(artifact.path, taskId),
      engine: "agent",
      track: "author",
      status: exitCode === null || exitCode === 0 ? "succeeded" : "failed",
      startedAt: inferredHarnessStart(artifact.modifiedAt, durationMs, sessionId),
      durationMs,
      attempt,
      seed: attemptRecord?.seed ?? (phase === "direct" ? result.seed : null),
      score: phase === "direct"
        ? attemptRecord?.authorGrade.score ?? capture?.authorGrade.score ?? null
        : null,
      toolCalls: normalized.filter((message) => message.role === "tool").length,
      cliCalls: normalized.filter((message) => message.role === "tool").length,
      llmCalls: null,
      codeCalls: null,
      messages: normalized.length + (typeof envelope.prompt === "string" ? 1 : 0),
      events: events.length,
      inputTokens,
      outputTokens,
      ...totals,
      workflowHash: phase === "approval" ? capture?.policy?.workflowHash ?? null : null,
      evidencePath: artifact.path,
      pairId: phase === "direct" ? `capture:${taskId}:${attempt}` : null,
    });
  }

  for (const capture of result.capture.tasks) {
    if (!capture.qualificationGrade && !capture.qualificationTracePath) continue;
    const traceArtifact = findTraceArtifact(
      artifacts,
      capture.qualificationTracePath,
      `qualification/${capture.taskId}-trace.json`,
    );
    const trace = traceArtifact ? parseViewerTrace(traceArtifact.content) : null;
    const activities = traceActivities(trace);
    const traceTokens = traceTokenTotals(activities);
    const durationMs = numberOrNull(trace?.duration_ms);
    const totals = viewerTokenTotals(traceTokens.inputTokens, traceTokens.outputTokens, durationMs);
    const selectedAttempt = capture.selectedAttempt ?? null;
    const attemptRecord = capture.attempts?.find((item) => item.attempt === selectedAttempt);
    rows.push({
      id: stringOrNull(trace?.run_id) ?? `qualification:${capture.taskId}`,
      taskId: capture.taskId,
      label: "Cori qualification",
      engine: "cori",
      track: "cori",
      status: stringOrNull(trace?.status) ?? (capture.qualificationPassed ? "succeeded" : "failed"),
      startedAt: stringOrNull(trace?.started_at),
      durationMs,
      attempt: selectedAttempt,
      seed: attemptRecord?.seed ?? null,
      score: capture.qualificationGrade?.score ?? null,
      toolCalls: activities.length || null,
      cliCalls: activityKindCount(activities, "cli"),
      llmCalls: activityKindCount(activities, "llm"),
      codeCalls: activityKindCount(activities, "code"),
      messages: null,
      events: null,
      inputTokens: traceTokens.inputTokens,
      outputTokens: traceTokens.outputTokens,
      ...totals,
      workflowHash: capture.policy?.workflowHash ?? null,
      evidencePath: traceArtifact?.path ?? "result.json",
      pairId: selectedAttempt === null ? null : `capture:${capture.taskId}:${selectedAttempt}`,
    });
  }

  for (const trial of result.trials) {
    const pairId = `trial:${trial.taskId}:${trial.seed}`;
    if (trial.lane === "direct") {
      const harness = trial.harness;
      const normalized = normalizeTranscript(harness?.transcript ?? []);
      const durationMs = numberOrNull(harness?.wallTimeMs);
      const inputTokens = numberOrNull(harness?.usage.inputTokens);
      const outputTokens = numberOrNull(harness?.usage.outputTokens);
      const totals = viewerTokenTotals(inputTokens, outputTokens, durationMs);
      rows.push({
        id: harness?.sessionId ?? `direct:${trial.taskId}:${trial.seed}`,
        taskId: trial.taskId,
        label: `Held-out direct · seed ${trial.seed}`,
        engine: "agent",
        track: "direct",
        status: harness && harness.exitCode !== 0 ? "failed" : "succeeded",
        startedAt: uuidV7Timestamp(harness?.sessionId),
        durationMs,
        attempt: null,
        seed: trial.seed,
        score: trial.grade.score,
        toolCalls: normalized.filter((message) => message.role === "tool").length,
        cliCalls: normalized.filter((message) => message.role === "tool").length,
        llmCalls: null,
        codeCalls: null,
        messages: normalized.length + (harness?.prompt ? 1 : 0),
        events: harness?.transcript.length ?? null,
        inputTokens,
        outputTokens,
        ...totals,
        workflowHash: null,
        evidencePath: "result.json",
        pairId,
      });
      continue;
    }

    const traceArtifact = findTraceArtifact(artifacts, trial.tracePath);
    const trace = traceArtifact ? parseViewerTrace(traceArtifact.content) : null;
    const activities = traceActivities(trace);
    const traceTokens = traceTokenTotals(activities);
    const durationMs = numberOrNull(trial.runtime?.wallTimeMs) ?? numberOrNull(trace?.duration_ms);
    const inputTokens = numberOrNull(trial.runtime?.inputTokens) ?? traceTokens.inputTokens;
    const outputTokens = numberOrNull(trial.runtime?.outputTokens) ?? traceTokens.outputTokens;
    const totals = viewerTokenTotals(inputTokens, outputTokens, durationMs);
    rows.push({
      id: stringOrNull(trace?.run_id) ?? `replay:${trial.taskId}:${trial.seed}`,
      taskId: trial.taskId,
      label: `Cori replay · seed ${trial.seed}`,
      engine: "cori",
      track: "cori",
      status: stringOrNull(trace?.status) ?? "succeeded",
      startedAt: stringOrNull(trace?.started_at),
      durationMs,
      attempt: null,
      seed: trial.seed,
      score: trial.grade.score,
      toolCalls: activities.length || null,
      cliCalls: activityKindCount(activities, "cli"),
      llmCalls: activityKindCount(activities, "llm"),
      codeCalls: activityKindCount(activities, "code"),
      messages: null,
      events: null,
      inputTokens,
      outputTokens,
      ...totals,
      workflowHash: trial.workflowHash ?? null,
      evidencePath: traceArtifact?.path ?? "result.json",
      pairId,
    });
  }

  return rows
    .map((row, index) => ({ row, index }))
    .sort((left, right) => {
      const leftTime = left.row.startedAt ? Date.parse(left.row.startedAt) : Number.NaN;
      const rightTime = right.row.startedAt ? Date.parse(right.row.startedAt) : Number.NaN;
      if (Number.isFinite(leftTime) && Number.isFinite(rightTime) && leftTime !== rightTime) return leftTime - rightTime;
      if (Number.isFinite(leftTime) !== Number.isFinite(rightTime)) return Number.isFinite(leftTime) ? -1 : 1;
      return left.index - right.index;
    })
    .map(({ row }) => row);
}

function viewerTokenTotals(
  inputTokens: number | null,
  outputTokens: number | null,
  durationMs: number | null,
): Pick<ViewerSessionComparison, "totalTokens" | "tokensPerSecond" | "priceUsd"> {
  if (inputTokens === null || outputTokens === null) {
    return { totalTokens: null, tokensPerSecond: null, priceUsd: null };
  }
  const totalTokens = inputTokens + outputTokens;
  return {
    totalTokens,
    tokensPerSecond: durationMs && durationMs > 0 ? totalTokens / (durationMs / 1_000) : null,
    priceUsd: inputTokens * INPUT_TOKEN_PRICE_PER_MILLION_USD / 1_000_000
      + outputTokens * OUTPUT_TOKEN_PRICE_PER_MILLION_USD / 1_000_000,
  };
}

function inferredHarnessStart(
  modifiedAt: string | undefined,
  durationMs: number | null,
  sessionId: string | null,
): string | null {
  const endedAt = modifiedAt ? Date.parse(modifiedAt) : Number.NaN;
  if (Number.isFinite(endedAt) && durationMs !== null) return new Date(endedAt - durationMs).toISOString();
  return uuidV7Timestamp(sessionId);
}

function uuidV7Timestamp(value: string | null | undefined): string | null {
  if (!value || !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-/iu.test(value)) return null;
  const milliseconds = Number.parseInt(value.replaceAll("-", "").slice(0, 12), 16);
  return Number.isSafeInteger(milliseconds) ? new Date(milliseconds).toISOString() : null;
}

function findTraceArtifact(
  artifacts: readonly ViewerArtifact[],
  tracePath: string | undefined,
  fallbackPath?: string,
): ViewerArtifact | undefined {
  if (fallbackPath) {
    const fallback = artifacts.find((artifact) => artifact.path === fallbackPath);
    if (fallback) return fallback;
  }
  const fileName = tracePath?.replaceAll("\\", "/").split("/").at(-1);
  return fileName
    ? artifacts.find((artifact) => artifact.kind === "trace" && artifact.path.endsWith(`/${fileName}`))
    : undefined;
}

function parseViewerTrace(content: string | undefined): Record<string, unknown> | null {
  const outer = parseJsonValue(content ?? "");
  if (!isRecord(outer)) return null;
  const inner = typeof outer.stdout === "string" ? parseJsonValue(outer.stdout) : outer;
  return isRecord(inner) ? inner : null;
}

function traceActivities(trace: Record<string, unknown> | null): Record<string, unknown>[] {
  return Array.isArray(trace?.activities) ? trace.activities.filter(isRecord) : [];
}

function traceTokenTotals(activities: readonly Record<string, unknown>[]): {
  inputTokens: number | null;
  outputTokens: number | null;
} {
  const usages = activities.flatMap((activity) => isRecord(activity.token_usage) ? [activity.token_usage] : []);
  const inputs = usages.flatMap((usage) => {
    const value = numberOrNull(usage.input_tokens ?? usage.inputTokens);
    return value === null ? [] : [value];
  });
  const outputs = usages.flatMap((usage) => {
    const value = numberOrNull(usage.output_tokens ?? usage.outputTokens);
    return value === null ? [] : [value];
  });
  return {
    inputTokens: inputs.length ? inputs.reduce((sum, value) => sum + value, 0) : null,
    outputTokens: outputs.length ? outputs.reduce((sum, value) => sum + value, 0) : null,
  };
}

function activityKindCount(
  activities: readonly Record<string, unknown>[],
  kind: "cli" | "llm" | "code",
): number | null {
  if (!activities.length) return null;
  return activities.filter((activity) => String(activity.kind ?? "").replace(/^cori_/u, "") === kind).length;
}

function numberOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function stringOrNull(value: unknown): string | null {
  return typeof value === "string" && value ? value : null;
}

function compareViewerSessions(left: ViewerSession, right: ViewerSession): number {
  const taskDifference = left.taskId.localeCompare(right.taskId);
  if (taskDifference) return taskDifference;
  return viewerSessionOrder(left) - viewerSessionOrder(right)
    || left.name.localeCompare(right.name, undefined, { numeric: true });
}

function viewerSessionOrder(session: ViewerSession): number {
  if (!session.sourcePath.startsWith("transcripts/authors/")) return 10_000;
  const attempt = Number(session.sourcePath.match(/-attempt-(\d+)-/u)?.[1] ?? "1");
  const phase = session.sourcePath.includes("-direct.json")
    ? 0
    : session.sourcePath.includes("capture-preview") ? 1 : 2;
  return attempt * 10 + phase;
}

function prependViewerPrompt(
  messages: readonly ViewerMessage[],
  prompt: string | null,
  status: string,
): ViewerMessage[] {
  if (!prompt?.trim()) return [...messages];
  return [{
    role: "user",
    label: "Benchmark",
    text: truncateViewerText(prompt),
    status,
    index: 0,
  }, ...messages];
}

function fallbackSessionPrompt(
  path: string,
  taskId: string,
  artifacts: readonly ViewerArtifact[],
): string | null {
  if (path.includes("capture-preview")) {
    return "The direct task attempt is complete. Capture the demonstrated procedure as a reusable Cori workflow. Show a read-only preview only; do not create, edit, or run workflow files yet.";
  }
  if (path.includes("capture-approval")) {
    return "Approved. Write the reviewed workflow to ./captured-workflow and run the pinned Cori check. Do not run the workflow.";
  }
  const fileName = path.split("/").at(-1)?.replace(/-direct\.json$/u, "") ?? taskId;
  const exactPath = `agent-workspace/authors/${fileName}/TASK.md`;
  return artifacts.find((artifact) => artifact.path === exactPath)?.content
    ?? taskPromptArtifact(artifacts, taskId)?.content
    ?? null;
}

function taskPromptArtifact(
  artifacts: readonly ViewerArtifact[],
  taskId: string,
): ViewerArtifact | undefined {
  return artifacts.find((artifact) =>
    artifact.path.startsWith("agent-workspace/")
    && artifact.path.includes(taskId)
    && artifact.path.endsWith("/TASK.md")
    && artifact.content
  );
}

function viewerSessionName(path: string, taskId: string): string {
  const stem = path.split("/").at(-1)?.replace(/\.json$/iu, "") ?? path;
  const suffix = stem.startsWith(taskId) ? stem.slice(taskId.length).replace(/^-+/u, "") : stem;
  const attempt = suffix.match(/^attempt-(\d+)-(.+)$/u);
  const phase = attempt?.[2] ?? suffix;
  const prefix = attempt ? `Author attempt ${attempt[1]}` : "Author";
  if (phase === "direct") return `${prefix} · direct task`;
  if (phase === "capture-preview") return `${prefix} · capture preview`;
  if (phase === "capture-approval") return `${prefix} · capture approval`;
  return suffix.replaceAll("-", " ");
}

function transcriptEvents(content: string): readonly unknown[] {
  const parsed = parseJsonValue(content);
  const direct = eventArray(parsed);
  if (direct) return direct;

  const lines = content
    .split(/\r?\n/gu)
    .map((line) => line.trim())
    .filter(Boolean)
    .map(parseJsonValue)
    .filter((value) => value !== undefined);
  return lines.flatMap((value) => eventArray(value) ?? [value]);
}

function parseJsonValue(value: unknown, depth = 0): unknown {
  if (typeof value !== "string" || depth >= 3) return value;
  const trimmed = value.trim();
  if (!trimmed || !["{", "[", '"'].includes(trimmed[0] ?? "")) return value;
  try {
    return parseJsonValue(JSON.parse(trimmed), depth + 1);
  } catch {
    return depth === 0 ? undefined : value;
  }
}

function eventArray(value: unknown): readonly unknown[] | null {
  if (Array.isArray(value)) return value;
  if (!isRecord(value)) return null;
  for (const key of ["transcript", "events", "messages", "items"]) {
    if (Array.isArray(value[key])) return value[key];
  }
  return value.type ? [value] : null;
}

function normalizeTranscript(events: readonly unknown[]): ViewerMessage[] {
  const messages: ViewerMessage[] = [];
  const toolIndexes = new Map<string, number>();
  for (const [offset, rawEvent] of events.entries()) {
    if (messages.length >= viewerSessionLimit) break;
    const unwrapped = parseJsonValue(rawEvent);
    const container = isRecord(unwrapped) ? unwrapped : { text: String(unwrapped ?? "") };
    const payload = isRecord(container.item)
      ? container.item
      : isRecord(container.message)
        ? container.message
        : container;
    const type = String(payload.type ?? container.type ?? "event");
    const tool = normalizedTool(payload) ?? normalizedTool(container);
    const text = tool?.text ?? normalizedText(payload) ?? normalizedText(container);
    if (!text?.trim()) continue;
    const declaredRole = String(payload.role ?? container.role ?? inferredRole(type)).toLowerCase();
    const role: ViewerMessage["role"] = tool ? "tool" : declaredRole === "user" ? "user" : "assistant";
    const message: ViewerMessage = {
      role,
      label: tool?.label ?? (role === "user" ? "Benchmark" : "Agent"),
      text: truncateViewerText(text),
      ...(tool?.detail ? { detail: truncateViewerText(tool.detail) } : {}),
      ...(tool?.status ? { status: tool.status } : {}),
      index: offset + 1,
    };
    if (tool?.key) {
      const existing = toolIndexes.get(tool.key);
      if (existing !== undefined) {
        messages[existing] = message;
        continue;
      }
      toolIndexes.set(tool.key, messages.length);
    }
    const previous = messages.at(-1);
    if (previous
      && previous.role === message.role
      && previous.label === message.label
      && previous.text === message.text) continue;
    messages.push(message);
  }
  return messages;
}

function normalizedText(value: Record<string, unknown>): string | null {
  for (const key of ["text", "output_text", "content", "message", "summary", "reasoning"]) {
    const candidate = value[key];
    if (typeof candidate === "string" && candidate.trim()) return candidate;
    if (!Array.isArray(candidate)) continue;
    const joined = candidate
      .map((entry) => {
        if (typeof entry === "string") return entry;
        if (!isRecord(entry)) return "";
        return typeof entry.text === "string"
          ? entry.text
          : typeof entry.content === "string"
            ? entry.content
            : "";
      })
      .filter(Boolean)
      .join("\n");
    if (joined) return joined;
  }
  return null;
}

function normalizedTool(value: Record<string, unknown>): {
  label: string;
  text: string;
  detail?: string;
  status?: string;
  key?: string;
} | null {
  const type = String(value.type ?? "").toLowerCase();
  const isTool = type.includes("tool")
    || type.includes("command")
    || type.includes("function")
    || value.command !== undefined
    || value.arguments !== undefined
    || value.input !== undefined;
  if (!isTool) return null;
  const name = value.name ?? value.tool_name ?? value.function_name ?? (type || "tool");
  const input = value.command ?? value.input ?? value.arguments ?? "";
  const output = value.aggregated_output ?? value.output ?? value.result ?? "";
  const statusParts = [
    value.status === undefined ? "" : String(value.status).replaceAll("_", " "),
    value.exit_code === undefined || value.exit_code === null ? "" : `exit ${String(value.exit_code)}`,
  ].filter(Boolean);
  return {
    label: `Tool · ${String(name).replaceAll("_", " ")}`,
    text: input === ""
      ? String(name).replaceAll("_", " ")
      : typeof input === "string" ? input : safeStringify(input),
    ...(output === "" ? {} : { detail: typeof output === "string" ? output : safeStringify(output) }),
    ...(statusParts.length ? { status: statusParts.join(" · ") } : {}),
    ...(typeof value.id === "string" && value.id ? { key: value.id } : {}),
  };
}

function inferredRole(type: string): ViewerMessage["role"] {
  const value = type.toLowerCase();
  if (value.includes("user") || value.includes("prompt")) return "user";
  if (value.includes("tool") || value.includes("command") || value.includes("function")) return "tool";
  return "assistant";
}

function truncateViewerText(value: string): string {
  const normalized = value.replaceAll("\0", "");
  if (normalized.length <= viewerMessageLimit) return normalized;
  return `${normalized.slice(0, viewerMessageLimit)}\n… truncated in viewer; open the raw transcript for the complete event`;
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function extractViewerLinks(content: string): ViewerLink[] {
  if (!content) return [];
  const normalized = content
    .replaceAll("\\/", "/")
    .replace(/\\u002f/giu, "/")
    .replace(/\\u0026/giu, "&")
    .replace(/\\u003d/giu, "=");
  const seen = new Set<string>();
  const links: ViewerLink[] = [];
  for (const match of normalized.matchAll(/https?:\/\/[^\s<>"'\\]+/giu)) {
    const url = cleanViewerUrl(match[0]);
    if (!url || seen.has(url)) continue;
    try {
      const parsed = new URL(url);
      if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) continue;
      seen.add(url);
      links.push({ label: viewerLinkLabel(parsed), url });
    } catch {
      continue;
    }
  }
  return links
    .sort((left, right) => viewerLinkPriority(left.url) - viewerLinkPriority(right.url))
    .slice(0, viewerLinkLimit);
}

function cleanViewerUrl(value: string): string {
  let end = value.length;
  while (end > 0 && /[.,;:!?]/u.test(value[end - 1] ?? "")) end -= 1;
  for (const [open, close] of [["(", ")"], ["[", "]"], ["{", "}"]] as const) {
    while (end > 0
      && value[end - 1] === close
      && countCharacter(value.slice(0, end), close) > countCharacter(value.slice(0, end), open)) {
      end -= 1;
    }
  }
  return value.slice(0, end);
}

function countCharacter(value: string, character: string): number {
  return [...value].filter((item) => item === character).length;
}

function viewerLinkPriority(url: string): number {
  if (/^https:\/\/(docs|drive|calendar)\.google\.com\//iu.test(url)) return 0;
  if (/^https:\/\/mail\.google\.com\//iu.test(url)) return 0;
  if (/^https:\/\/(developers|support)\.google\.com\//iu.test(url)) return 2;
  return 1;
}

function viewerLinkLabel(url: URL): string {
  if (url.hostname === "docs.google.com") {
    if (url.pathname.startsWith("/spreadsheets/")) return "Open spreadsheet";
    if (url.pathname.startsWith("/document/")) return "Open document";
    if (url.pathname.startsWith("/presentation/")) return "Open presentation";
  }
  if (url.hostname === "drive.google.com") return "Open Drive resource";
  if (url.hostname === "calendar.google.com") return "Open calendar resource";
  if (url.hostname === "mail.google.com") return "Open Gmail";
  return url.hostname;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function safeJson(value: unknown): string {
  return JSON.stringify(value)
    .replaceAll("<", "\\u003c")
    .replaceAll(">", "\\u003e")
    .replaceAll("&", "\\u0026")
    .replaceAll("\u2028", "\\u2028")
    .replaceAll("\u2029", "\\u2029");
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

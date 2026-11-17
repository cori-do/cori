import { useState, type ReactNode } from "react";
import { Check, ChevronDown, X } from "lucide-react";
import { api, authConfig, claudeMcpAdd } from "../lib/api";
import { brand } from "../lib/brand";
import { starterCalls } from "../lib/format";
import { useQuery } from "../lib/query";
import { href } from "../lib/router";
import type { StartState } from "../lib/types";
import { Card, CopyField, cx, IconButton } from "../ui/core";
import { PersonName } from "../ui/people";

// The first steps of an organization, from a coding agent to a first release people can try. Each ticks itself from what
// the server saw (`GET /v1/start`, asked again every few seconds), so a step done in a terminal or by an agent ticks here
// on its own. Hidden once every step is done, or when the person closes it.

const DISMISSED = "cori.start.dismissed";

export function GetStarted() {
  const [dismissed, setDismissed] = useState(() => { try { return localStorage.getItem(DISMISSED) === "1"; } catch { return false; } });
  const q = useQuery<StartState>(dismissed ? null : "/v1/start", () => api<StartState>("/v1/start"), { every: 5000 });
  const [opened, setOpened] = useState<string | null>(null);
  const d = q.data;
  if (dismissed || !d) return null;
  const mcp = authConfig().mcp || `${location.origin}/mcp`;
  const plugin = authConfig().agent_plugin;
  const steps: { key: string; title: string; done: boolean; doneText: ReactNode; body: ReactNode }[] = [
    {
      key: "agent", title: "Connect your coding agent", done: d.agent.done,
      doneText: <>{d.agent.detail || "An agent"} is connected{d.agent.by ? <> by <PersonName id={d.agent.by} /></> : null}</>,
      body: <>
        <p>Your agent builds, checks and deploys apps for you. Add {brand.name} to it; the first time, it opens your browser for you to sign in.</p>
        <CopyField className="mt-3" value={plugin || claudeMcpAdd(mcp)} />
        <p className="mt-2 text-[13px] text-fg-3">Using Cursor, VS Code or another agent? <a href={href("connect")} className="link">See every agent</a></p>
      </>,
    },
    {
      key: "cli", title: "Install the command line", done: d.cli.done,
      doneText: <>{d.cli.detail?.replace(/^cori\//, "cori ") || "The command line"} is in use</>,
      body: <>
        <p>Your agent checks and deploys with it. macOS and Linux (Windows through WSL).</p>
        <CopyField className="mt-3" value="npm install -g @cori-do/cli" />
      </>,
    },
    {
      key: "llm", title: "Choose a model for AI steps", done: d.llm.done,
      doneText: d.llm.own || !d.llm.starter ? <>Your own provider answers AI steps</>
        : <>The starter model answers for now ({starterCalls(d.llm.starter)}){d.llm.can_add && <> · <a href={href("settings", "models")} className="link">add your own provider</a></>}</>,
      body: <>
        <p>AI steps run on your organization's own key: OpenAI, Anthropic or any OpenAI-compatible endpoint. Apps without AI steps need none.</p>
        {d.llm.can_add ? <a href={href("settings", "models")} className="link mt-2 inline-block font-medium">Add a provider</a> : <p className="mt-2 text-fg-3">An admin of your organization adds it.</p>}
      </>,
    },
    {
      key: "app", title: "Create your first app", done: d.app.done,
      doneText: d.app.id ? <><a href={href("app", d.app.id)} className="link">{d.app.id}</a> is created</> : <>An app is created</>,
      body: <>
        <p>Ask your agent, in your own words:</p>
        <CopyField className="mt-3" multiline value={`Build me an internal app on ${brand.name} for … Then deploy it and give me its preview link.`} />
        <p className="mt-2 text-[13px] text-fg-3">Or start from a finished example: <code className="text-fg-2">cori init my-app --template team-requests</code></p>
      </>,
    },
    {
      key: "release", title: "Try it, then make it live", done: d.release.done,
      doneText: d.release.app ? <>{d.release.release || "A release"} of <a href={href("app", d.release.app, "releases")} className="link">{d.release.app}</a> is deployed{d.release.preview_url && <> · <a href={d.release.preview_url} target="_blank" rel="noreferrer" className="link">open its preview</a></>}{d.portal_url && <> · your colleagues find it in the <a href={d.portal_url} target="_blank" rel="noreferrer" className="link">apps portal</a></>}</> : <>A release is deployed</>,
      body: <>
        <p>Each release runs on its own preview data at its own address. Nothing your colleagues use changes until a person makes it live.</p>
        <CopyField className="mt-3" value="cori deploy" />
      </>,
    },
  ];
  const done = steps.filter((x) => x.done).length;
  if (done === steps.length) return null;
  const next = steps.find((x) => !x.done)!.key;
  const open = opened ?? next;
  const close = () => { try { localStorage.setItem(DISMISSED, "1"); } catch { /* private mode */ } setDismissed(true); };
  return (
    <Card as="section" className="mb-8 overflow-hidden">
      <div className="flex items-center gap-4 px-5 pt-4 pb-3">
        <div className="min-w-0 flex-1">
          <h2 className="text-[15px] font-semibold">Get started</h2>
          <p className="text-[13px] text-fg-3">From your coding agent to a first app your colleagues can use. Steps tick themselves.</p>
        </div>
        <div className="hidden items-center gap-2.5 sm:flex">
          <div className="h-1.5 w-28 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-valuemin={0} aria-valuemax={steps.length} aria-valuenow={done} aria-label="Steps done">
            <div className="h-full rounded-full bg-emerald-500 transition-[width]" style={{ width: `${(done / steps.length) * 100}%` }} />
          </div>
          <span className="text-[13px] text-fg-3 tabular-nums">{done} of {steps.length}</span>
        </div>
        <IconButton label="Hide the first steps" size="xs" onClick={close}><X /></IconButton>
      </div>
      <ol className="border-t border-line">
        {steps.map((x, i) => {
          const isOpen = open === x.key && !x.done;
          const mark = (
            <span className={cx("flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
              x.done ? "bg-emerald-500 text-white" : x.key === next ? "bg-brand-600 text-white" : "bg-surface-3 text-fg-3")}>
              {x.done ? <Check className="size-3.5" aria-label="done" /> : i + 1}
            </span>
          );
          return (
            <li key={x.key} className="border-b border-line last:border-0">
              {x.done ? (
                <div className="flex items-center gap-3 px-5 py-3">
                  {mark}
                  <span className="min-w-0 flex-1 text-fg-3">{x.title}<span className="mt-0.5 block truncate text-[13px]">{x.doneText}</span></span>
                </div>
              ) : (
                <button type="button" onClick={() => setOpened(isOpen ? "" : x.key)} aria-expanded={isOpen}
                  className="flex w-full cursor-pointer items-center gap-3 px-5 py-3 text-left hover:bg-surface-2/60">
                  {mark}
                  <span className="min-w-0 flex-1 font-medium text-fg">{x.title}</span>
                  <ChevronDown className={cx("size-4 shrink-0 text-fg-4 transition-transform", isOpen && "rotate-180")} aria-hidden />
                </button>
              )}
              {isOpen && <div className="animate-fade px-5 pb-4 pl-14 text-[14px] leading-relaxed text-fg-2">{x.body}</div>}
            </li>
          );
        })}
      </ol>
    </Card>
  );
}

// The picked workflow as a graph — the Cori Graph Canvas.
//
// The compiled plan is drawn as a vertical flow on a dotted canvas: the
// run trigger on top, one node per compiled step, the result at the
// bottom, edges between them. The same nodes carry every phase of the
// workflow's life: an agent writing it (journalled authoring session),
// the frozen proposal awaiting the human's call, the compiled plan,
// a live run with data travelling the edges, and the settled trace.
//
// The runtime exposes the plan as an ordered list (no explicit edge
// set), so the canvas draws the honest graph: a chain in plan order.
// Everything shown is derived — compiled steps, journal events, trace
// activities — never an agent's claims.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  acceptAuthoringProposal,
  rejectAuthoringProposal,
  rewindAuthoringSession,
  sessionJournal,
  stepMedians,
  stopAuthoringSession,
  type ActivityTrace,
  type AuthoringSession,
  type AuthoringSessionEvent,
  type Capability,
  type ProposalChange,
  type ProposalStep,
  type RunTrace,
  type SessionProposal,
  type StepMedianEntry,
  type StepSummary,
  type WorkflowPreflight,
} from "../lib/api";
import { formatCost, formatDuration } from "../lib/format";
import { stepField, whatRuns } from "../lib/step-source";
import { BackendLogo } from "./provider-icons";

// ─── Run state (shared with WorkflowPane) ────────────────────────────────

export interface LiveStep {
  step_name: string;
  kind?: string;
  status: "queued" | "running" | "succeeded" | "failed" | "skipped" | "not_taken";
  duration_ms?: number;
  error?: string | null;
  /** Broker/workflow notes — carry the control-flow decisions
   *  ("took `then`", "matched `cases.big`") that light the taken path. */
  notes?: string[];
  /** performance.now() at step_start — drives the live per-node clock. */
  started_at_ms?: number;
}

export interface RunState {
  runId: string | null;
  /** activity_ids in plan order — the order the steps are shown in. */
  order: string[];
  steps: Record<string, LiveStep>;
  trace: RunTrace | null;
  error: string | null;
  closed: boolean;
  /** This run was started as a dry run — external steps are stubbed. */
  dry: boolean;
}

// ─── Geometry ────────────────────────────────────────────────────────────
//
// The plan is still a vertical flow, but a control-flow step is no
// longer one opaque box: its nested steps render as child nodes below
// it. `branch` / `switch` fan out into one lane per path and the lanes
// converge on the next step; `for_each` / `loop` show their body with a
// loop-back edge. The canvas widens to fit the widest fan.

const NODE_W = 280;
const NODE_H = 48;
const NODE_GAP = 26;
const RUN_W = 212;
const RUN_H = 42;
const OUT_W = 212;
const OUT_H = 38;
const MIN_CANVAS_W = NODE_W + 40;
const CHILD_W = 176;
const CHILD_H = 40;
const CHILD_GAP = 16;
/** Vertical gap parent → children row, and children row → merge. */
const FAN_GAP = 30;
/** Extra right margin reserved for a loop-back arc. */
const LOOP_MARGIN = 44;
/** Extra left margin reserved for goto rails and their labels. */
const ROUTE_MARGIN = 68;

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One nested lane under a control-flow step. */
interface ChildGeo {
  /** Compiler slot, doubling as the runner selector ("then", "cases.big"…). */
  slot: string;
  /** Short lane label drawn on the fan-out edge. */
  label: string;
  /** Nested step kind (cli / mcp_tool / code / llm). */
  kind: string;
  box: Box;
  /** Lane loops back to the parent (for_each / loop). */
  back: boolean;
}

interface StepGeo {
  box: Box;
  children: ChildGeo[];
}

/** Display label for a branch/switch slot. */
function slotLabel(slot: string): string {
  return slot.startsWith("cases.") ? slot.slice("cases.".length) : slot;
}

/** A `goto` path: drawn as an edge to its target step, never a lane. */
interface RouteSpec {
  slot: string;
  label: string;
  /** Target activity id, or `"end"` for the result node. */
  target: string;
  targetName: string;
}

/** The routing (`goto`) paths of a branch / switch step. */
function routeSpecs(step: StepSummary): RouteSpec[] {
  if (step.kind !== "builtin" || !step.builtin) return [];
  const nested = step.builtin_detail?.nested ?? {};
  return Object.entries(nested)
    .filter(([, s]) => s.goto != null)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([slot, s]) => ({
      slot,
      label: slotLabel(slot),
      target: s.goto as string,
      targetName: s.goto_name ?? (s.goto as string),
    }));
}

/** The nested lanes a step fans out into, in display order. */
function childSpecs(
  step: StepSummary,
): Array<{ slot: string; label: string; kind: string; back: boolean }> {
  if (step.kind !== "builtin" || !step.builtin) return [];
  const nested = step.builtin_detail?.nested ?? {};
  const kindOf = (slot: string) => nested[slot]?.kind ?? "code";
  const isLane = (slot: string) => slot in nested && nested[slot]?.goto == null;
  switch (step.builtin) {
    case "branch": {
      const out = [];
      if (isLane("then")) {
        out.push({ slot: "then", label: "then", kind: kindOf("then"), back: false });
      }
      if (isLane("else")) {
        out.push({ slot: "else", label: "else", kind: kindOf("else"), back: false });
      }
      return out;
    }
    case "switch": {
      const cases = Object.keys(nested)
        .filter((slot) => slot.startsWith("cases.") && isLane(slot))
        .sort()
        .map((slot) => ({
          slot,
          label: slotLabel(slot),
          kind: kindOf(slot),
          back: false,
        }));
      if (isLane("default")) {
        cases.push({
          slot: "default",
          label: "default",
          kind: kindOf("default"),
          back: false,
        });
      }
      return cases;
    }
    case "for_each":
      return "apply" in nested
        ? [{ slot: "apply", label: "each item", kind: kindOf("apply"), back: true }]
        : [];
    case "loop":
      return "body" in nested
        ? [{ slot: "body", label: "repeat", kind: kindOf("body"), back: true }]
        : [];
    default:
      return [];
  }
}

function layout(steps: StepSummary[]): {
  run: Box;
  steps: StepGeo[];
  out: Box;
  width: number;
  height: number;
} {
  const specs = steps.map(childSpecs);
  const hasRoutes = steps.some((s) => routeSpecs(s).length > 0);

  // Canvas width: widest fan wins; loop-back arcs (right) and goto
  // rails (left) reserve side room.
  let width = MIN_CANVAS_W;
  if (hasRoutes) {
    width = Math.max(width, NODE_W + 2 * ROUTE_MARGIN);
  }
  for (const children of specs) {
    if (children.length === 0) continue;
    const fan =
      children.length * CHILD_W + (children.length - 1) * CHILD_GAP + 40;
    const loop = children.some((c) => c.back) ? NODE_W + 2 * LOOP_MARGIN : 0;
    width = Math.max(width, fan, loop);
  }

  const run: Box = { x: (width - RUN_W) / 2, y: 0, w: RUN_W, h: RUN_H };
  let y = RUN_H + NODE_GAP + 4;
  const stepGeos: StepGeo[] = [];
  for (const children of specs) {
    const box: Box = { x: (width - NODE_W) / 2, y, w: NODE_W, h: NODE_H };
    y += NODE_H;
    const childGeos: ChildGeo[] = [];
    if (children.length > 0) {
      const rowW =
        children.length * CHILD_W + (children.length - 1) * CHILD_GAP;
      const x0 = (width - rowW) / 2;
      const cy = y + FAN_GAP;
      children.forEach((c, j) => {
        childGeos.push({
          ...c,
          box: { x: x0 + j * (CHILD_W + CHILD_GAP), y: cy, w: CHILD_W, h: CHILD_H },
        });
      });
      y = cy + CHILD_H + FAN_GAP;
    }
    stepGeos.push({ box, children: childGeos });
    y += NODE_GAP;
  }
  const outY = y + 4 - NODE_GAP + NODE_GAP; // keep the historic spacing
  const out: Box = { x: (width - OUT_W) / 2, y: outY, w: OUT_W, h: OUT_H };
  return { run, steps: stepGeos, out, width, height: outY + OUT_H + 8 };
}

function edgePath(a: Box, b: Box): string {
  const sx = a.x + a.w / 2;
  const sy = a.y + a.h;
  const tx = b.x + b.w / 2;
  const ty = b.y;
  const dy = Math.max(16, (ty - sy) * 0.6);
  return `M ${sx} ${sy} C ${sx} ${sy + dy} ${tx} ${ty - dy} ${tx} ${ty}`;
}

/** A goto edge: out of the router's left side, down a side rail, into
 *  the target's top. `rail` staggers parallel routes. */
function gotoPath(from: Box, to: Box, rail: number): string {
  const sx = from.x;
  const sy = from.y + from.h / 2;
  const tx = to.x + Math.min(28, to.w / 4);
  const ty = to.y;
  return `M ${sx} ${sy} C ${rail} ${sy} ${rail} ${sy} ${rail} ${
    sy + 24
  } L ${rail} ${ty - 26} C ${rail} ${ty - 4} ${tx} ${ty - 18} ${tx} ${ty}`;
}

/** Arc from a child lane's side back up to its parent (loop-back). */
function loopBackPath(child: Box, parent: Box): string {
  const sx = child.x + child.w;
  const sy = child.y + child.h / 2;
  const tx = parent.x + parent.w;
  const ty = parent.y + parent.h / 2;
  const reach = Math.max(sx, tx) + LOOP_MARGIN - 12;
  return `M ${sx} ${sy} C ${reach} ${sy} ${reach} ${ty} ${tx + 6} ${ty}`;
}

// ─── Phase model ─────────────────────────────────────────────────────────

export type CanvasPhase =
  | "writing" // an agent session is journalling into this folder
  | "proposed" // a frozen proposal awaits the human's call
  | "compiled" // preflight is ready, nothing moving
  | "running"
  | "failed"
  | "done";

function phaseOf(
  run: RunState | null,
  session: AuthoringSession | null,
): CanvasPhase {
  // A run in the pane (live or settled) leads; the session's presence
  // stays visible through the agent chip and the library badge.
  if (run && !run.closed) return "running";
  if (run?.closed) {
    return run.error != null || run.trace?.status === "failed"
      ? "failed"
      : "done";
  }
  if (session?.state === "proposed") return "proposed";
  if (session?.state === "writing") return "writing";
  return "compiled";
}

type NodeStatus =
  | "idle"
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "skipped"
  | "not_taken";

function nodeStatus(run: RunState | null, activityId: string): NodeStatus {
  if (!run) return "idle";
  const live = run.steps[activityId];
  if (!live) return "queued";
  return live.status;
}

// ─── The canvas ──────────────────────────────────────────────────────────

export function GraphCanvas({
  preflight,
  run,
  session,
  elapsedMs,
  historyKey,
  onRun,
  onSessionChanged,
}: {
  /** Null while a session's folder does not compile yet. */
  preflight: WorkflowPreflight | null;
  run: RunState | null;
  session: AuthoringSession | null;
  /** Live elapsed clock from the pane — already ticking during a run. */
  elapsedMs: number | null;
  historyKey: string | null;
  onRun: (dryRun: boolean) => void;
  onSessionChanged: () => void;
}) {
  const steps = preflight?.steps ?? [];
  const [sel, setSel] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  // Wide fans (a switch with many cases) auto-fit the stage width; the
  // user's zoom composes on top of the fit factor.
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [stageW, setStageW] = useState<number | null>(null);
  const [medians, setMedians] = useState<Record<string, StepMedianEntry>>({});
  const phase = phaseOf(run, session);
  const geo = useMemo(() => layout(steps), [steps]);
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width;
      if (width != null) setStageW(width);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [steps.length, session]);
  const fit = stageW != null ? Math.min(1, (stageW - 24) / geo.width) : 1;
  const scale = zoom * fit;

  // Per-step medians drive the running node's progress fill and the
  // timing tab — this workflow's own history, never a guess.
  useEffect(() => {
    if (!historyKey) {
      setMedians({});
      return;
    }
    let cancelled = false;
    stepMedians({ history_key: historyKey })
      .then((rows) => {
        if (cancelled) return;
        const map: Record<string, StepMedianEntry> = {};
        for (const r of rows) map[r.activity_id] = r;
        setMedians(map);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [historyKey]);

  // Escape closes the inspector (scoped: only when one is open).
  useEffect(() => {
    if (sel == null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSel(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sel]);

  const selStep = sel ? (steps.find((s) => s.activity_id === sel) ?? null) : null;

  // The step an agent is writing right now, from the journal's last event.
  const writingPath =
    session?.state === "writing" && session.last_event?.kind === "file_written"
      ? session.last_event.rel_path
      : null;

  const proposalByStep = useMemo(() => {
    const map = new Map<string, ProposalStep>();
    if (session?.proposal) {
      for (const st of session.proposal.steps) map.set(st.activity_id, st);
    }
    return map;
  }, [session?.proposal]);

  const doneCount = run
    ? Object.entries(run.steps).filter(
        ([id, s]) =>
          !id.includes("#") && s.status !== "queued" && s.status !== "running",
      ).length
    : 0;

  const showThinking = steps.length === 0 && session != null;

  // The proposal / trace bars overlay the canvas bottom; reserve room so
  // they never sit on top of the result node.
  const bottomBar =
    (phase === "proposed" && session?.proposal != null) ||
    ((phase === "done" || phase === "failed") && run?.trace != null);

  return (
    <div className="gc">
      <div
        className={`gc-canvas is-${phase}`}
        style={{
          height: Math.max(240, geo.height * scale + 72 + (bottomBar ? 148 : 0)),
        }}
      >
        <div className="gc-top">
          <PhasePill
            phase={phase}
            run={run}
            session={session}
            steps={steps}
            doneCount={doneCount}
          />
          {session &&
            (session.state === "writing" || session.state === "proposed") && (
              <AgentChip
                session={session}
                onSessionChanged={onSessionChanged}
              />
            )}
        </div>

        {!showThinking && (
        <div className="gc-stage" ref={stageRef}>
          <div
            className="gc-flow"
            style={{
              width: geo.width,
              height: geo.height,
              transform: `translateX(-50%) scale(${scale})`,
            }}
          >
            <Edges geo={geo} steps={steps} run={run} phase={phase} />

            <RunNode
              box={geo.run}
              phase={phase}
              run={run}
              ready={
                preflight != null &&
                preflight.ready &&
                !preflight.has_builtin_step
              }
              total={steps.length}
              doneCount={doneCount}
              onRun={onRun}
            />

            {steps.map((s, i) => (
              <StepNode
                key={s.activity_id}
                step={s}
                index={i}
                box={geo.steps[i].box}
                status={nodeStatus(run, s.activity_id)}
                live={run?.steps[s.activity_id]}
                selected={sel === s.activity_id}
                writing={
                  writingPath != null && writingPath.includes(s.name)
                }
                change={proposalByStep.get(s.activity_id)?.change}
                median={medians[s.activity_id]}
                elapsedMs={elapsedMs}
                onSelect={() =>
                  setSel((cur) => (cur === s.activity_id ? null : s.activity_id))
                }
              />
            ))}

            {steps.map((s, i) =>
              geo.steps[i].children.map((child) => (
                <ChildNode
                  key={`${s.activity_id}#${child.slot}`}
                  child={child}
                  parentStatus={nodeStatus(run, s.activity_id)}
                  taken={takenState(s, run, child.slot)}
                  lane={laneLive(run, s.activity_id, child.slot)}
                  onSelect={() =>
                    setSel((cur) =>
                      cur === s.activity_id ? null : s.activity_id,
                    )
                  }
                />
              )),
            )}

            <OutNode
              box={geo.out}
              phase={phase}
              run={run}
              stepCount={steps.length}
            />
          </div>
        </div>
        )}

        {showThinking && <ThinkingOverlay session={session} />}

        {!showThinking && (
        <div className="gc-zoom" role="group" aria-label="Canvas zoom">
          <button
            type="button"
            onClick={() => setZoom((z) => Math.min(1.4, z + 0.1))}
            aria-label="Zoom in"
          >
            +
          </button>
          <button
            type="button"
            onClick={() => setZoom((z) => Math.max(0.6, z - 0.1))}
            aria-label="Zoom out"
          >
            −
          </button>
          <button type="button" onClick={() => setZoom(1)} aria-label="Reset zoom">
            ⤢
          </button>
        </div>
        )}

        {phase === "proposed" && session?.proposal && (
          <ProposalBar
            session={session}
            proposal={session.proposal}
            onReviewStep={(activityId) => setSel(activityId)}
            onSessionChanged={onSessionChanged}
          />
        )}

        {(phase === "done" || phase === "failed") && run?.trace && (
          <TraceBar
            trace={run.trace}
            onSelect={(activityId) => setSel(activityId)}
          />
        )}
      </div>

      {selStep && (
        <Inspector
          step={selStep}
          index={steps.findIndex((s) => s.activity_id === selStep.activity_id)}
          status={nodeStatus(run, selStep.activity_id)}
          live={run?.steps[selStep.activity_id]}
          trace={run?.trace ?? null}
          capabilities={preflight?.capabilities?.capabilities ?? []}
          median={medians[selStep.activity_id]}
          session={session}
          proposalStep={proposalByStep.get(selStep.activity_id) ?? null}
          onClose={() => setSel(null)}
          onSessionChanged={onSessionChanged}
        />
      )}
    </div>
  );
}

// ─── Overlays + chrome ───────────────────────────────────────────────────

function PhasePill({
  phase,
  run,
  session,
  steps,
  doneCount,
}: {
  phase: CanvasPhase;
  run: RunState | null;
  session: AuthoringSession | null;
  steps: StepSummary[];
  doneCount: number;
}) {
  const total = steps.length;
  const billed = steps.filter((s) => s.kind === "llm").length;

  let label: string;
  let facts: string;
  let tone: "accent" | "green" | "red" | "muted" = "muted";
  let pulse = false;
  switch (phase) {
    case "writing":
      label = "writing";
      tone = "accent";
      pulse = true;
      facts = `${total} step${total === 1 ? "" : "s"} · journal ${session?.current_seq ?? 0} events`;
      break;
    case "proposed":
      label = "proposed";
      tone = "green";
      facts = session?.proposal
        ? `${session.proposal.steps.length} steps · publishes v${session.proposal.manifest_version + 1}`
        : `${total} steps`;
      break;
    case "running":
      label = run?.dry ? "dry run" : "running";
      tone = "accent";
      pulse = true;
      facts = `${doneCount} of ${total} done`;
      break;
    case "failed":
      label = "failed";
      tone = "red";
      facts = `${doneCount} of ${total} done`;
      break;
    case "done":
      label = "succeeded";
      tone = "green";
      facts = run?.trace
        ? `${run.trace.activities.length} steps · ${formatDuration(run.trace.duration_ms)}`
        : `${total} steps`;
      break;
    default:
      label = "compiled";
      facts =
        `${total} step${total === 1 ? "" : "s"}` +
        (billed > 0 ? ` · ${billed} billed` : "");
  }

  return (
    <div className="gc-phase" aria-live="polite">
      <span className={`gc-phase-state is-${tone}${pulse ? " is-pulse" : ""}`}>
        <span className="gc-phase-dot" aria-hidden />
        {label}
      </span>
      <span className="gc-phase-sep" aria-hidden>
        ·
      </span>
      <span className="gc-phase-facts">{facts}</span>
    </div>
  );
}

/** Top-right: which agent holds the pen, and the human's stop. */
function AgentChip({
  session,
  onSessionChanged,
}: {
  session: AuthoringSession;
  onSessionChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const writing = session.state === "writing";
  const line = writing
    ? session.last_event?.rel_path
      ? `${session.agent} · ${session.last_event.rel_path}`
      : `${session.agent} · writing`
    : `${session.agent} · awaiting your call`;
  return (
    <div className="gc-agent" title={`session ${session.session_id}`}>
      <span className="gc-agent-dot" aria-hidden />
      <span className="gc-agent-line">{line}</span>
      {writing && (
        <button
          type="button"
          className="gc-agent-stop"
          disabled={busy}
          title="Stop this session — the agent's next write is refused with the reason"
          onClick={() => {
            setBusy(true);
            stopAuthoringSession(session.session_id, "stopped from the Console")
              .then(() => onSessionChanged())
              .catch(() => {})
              .finally(() => setBusy(false));
          }}
        >
          stop
        </button>
      )}
    </div>
  );
}

/** The folder is open but nothing is compiled yet — the agent is reading. */
function ThinkingOverlay({ session }: { session: AuthoringSession | null }) {
  return (
    <div className="gc-thinking" role="status">
      <img
        src="/cori-mark.png"
        alt=""
        width={44}
        height={44}
        className="gc-thinking-mark"
      />
      <div className="gc-thinking-title">
        {session ? `${session.agent} opened this workflow` : "Nothing compiled yet"}
      </div>
      {session && (
        <div className="gc-thinking-meta">
          session {session.session_id} ·{" "}
          {session.last_event ? sessionEventSummary(session.last_event) : "started"} ·{" "}
          {session.current_seq} event{session.current_seq === 1 ? "" : "s"}
        </div>
      )}
      <div className="gc-thinking-slot" aria-hidden>
        <span />
        <span />
        <span />
      </div>
      <div className="gc-thinking-hint">the first step will land here</div>
    </div>
  );
}

// ─── Nodes + edges ───────────────────────────────────────────────────────

/** Where a nested lane stands once its parent has decided. */
type TakenState = "pending" | "taken" | "untaken";

/**
 * Which nested slot a finished control-flow step actually took, read
 * from its trace/live notes — derived facts, never a guess. `null`
 * while undecided (or when a false `if` with no `else` took nothing).
 */
function decidedSlot(
  step: StepSummary,
  run: RunState | null,
): { decided: boolean; slot: string | null } {
  if (!run) return { decided: false, slot: null };
  const live = run.steps[step.activity_id];
  const traceNotes = run.trace?.activities.find(
    (a) => a.activity_id === step.activity_id,
  )?.notes;
  const joined = [
    ...(live?.notes ?? []),
    ...(typeof traceNotes === "string" ? [traceNotes] : []),
  ].join(" · ");
  if (!joined) return { decided: false, slot: null };
  switch (step.builtin) {
    case "branch": {
      if (joined.includes("took `then`")) return { decided: true, slot: "then" };
      if (joined.includes("took `else`")) return { decided: true, slot: "else" };
      if (joined.includes("condition was false"))
        return { decided: true, slot: null };
      return { decided: false, slot: null };
    }
    case "switch": {
      const m = joined.match(/matched `(cases\.[^`]+|default)`/u);
      return m ? { decided: true, slot: m[1] } : { decided: false, slot: null };
    }
    // The body/apply lane is "taken" whenever the step finished.
    case "for_each":
      return { decided: true, slot: "apply" };
    case "loop":
      return { decided: true, slot: "body" };
    default:
      return { decided: false, slot: null };
  }
}

function takenState(
  step: StepSummary,
  run: RunState | null,
  slot: string,
): TakenState {
  const status = nodeStatus(run, step.activity_id);
  if (status !== "succeeded" && status !== "failed" && status !== "skipped") {
    return "pending";
  }
  const { decided, slot: taken } = decidedSlot(step, run);
  if (!decided) return "pending";
  return taken === slot ? "taken" : "untaken";
}

/** Live state of one lane, aggregated over its dispatches (`#apply[0]`,
 *  `#apply[1]`, …): whether it is running right now, and how many
 *  dispatches finished — the iteration counter for loop lanes. */
interface LaneLive {
  running: boolean;
  finished: number;
  failed: boolean;
}

function laneLive(run: RunState | null, stepId: string, slot: string): LaneLive {
  const out: LaneLive = { running: false, finished: 0, failed: false };
  if (!run) return out;
  const exact = `${stepId}#${slot}`;
  for (const [id, live] of Object.entries(run.steps)) {
    if (id !== exact && !id.startsWith(`${exact}[`)) continue;
    if (live.status === "running") out.running = true;
    else if (live.status === "failed") {
      out.failed = true;
      out.finished += 1;
    } else if (live.status !== "queued") out.finished += 1;
  }
  return out;
}

interface EdgeSpec {
  d: string;
  cls: string;
  flowing: boolean;
  label?: string;
  labelX?: number;
  labelY?: number;
  labelAnchor?: "middle" | "end";
  back?: boolean;
}

function Edges({
  geo,
  steps,
  run,
  phase,
}: {
  geo: ReturnType<typeof layout>;
  steps: StepSummary[];
  run: RunState | null;
  phase: CanvasPhase;
}) {
  const draft = phase === "writing" || phase === "proposed";
  const statusOf = (i: number): NodeStatus => {
    // Virtual endpoints borrow their neighbour's state.
    if (i < 0) return run ? "succeeded" : "idle";
    if (i >= steps.length) {
      return run?.closed && run.trace?.status === "succeeded"
        ? "succeeded"
        : "queued";
    }
    return nodeStatus(run, steps[i].activity_id);
  };
  const chainCls = (from: NodeStatus, to: NodeStatus): string => {
    if (from === "not_taken" || to === "not_taken") {
      return run ? "is-untaken" : "";
    }
    const flowing = phase === "running" && from === "succeeded" && to === "running";
    const done =
      from === "succeeded" && (to === "succeeded" || to === "failed");
    return draft ? "is-draft" : flowing ? "is-flowing" : run && done ? "is-done" : "";
  };

  const edges: EdgeSpec[] = [];

  // Walk the chain run → s0 → … → out. A step without children is one
  // edge to the next node; a step with children fans out to its lanes
  // and the lanes converge on the next node (loop lanes also arc back),
  // so the following chain edge is already drawn.
  const nextTop = (i: number): Box =>
    i + 1 < steps.length ? geo.steps[i + 1].box : geo.out;
  let prev: Box = geo.run;
  let prevIdx = -1;
  let prevFanned = false;
  for (let i = 0; i < steps.length; i += 1) {
    const { box, children } = geo.steps[i];
    const from = statusOf(prevIdx);
    const to = statusOf(i);
    if (!prevFanned) {
      edges.push({
        d: edgePath(prev, box),
        cls: chainCls(from, to),
        flowing:
          !draft && phase === "running" && from === "succeeded" && to === "running",
      });
    }
    prevFanned = false;

    if (children.length > 0) {
      const parentStatus = statusOf(i);
      const target = nextTop(i);
      const nextStatus = statusOf(i + 1);
      for (const child of children) {
        const state = takenState(steps[i], run, child.slot);
        const lane = laneLive(run, steps[i].activity_id, child.slot);
        const laneCls = draft
          ? "is-draft"
          : state === "taken"
            ? "is-done"
            : state === "untaken"
              ? "is-untaken"
              : lane.running
                ? "is-flowing"
                : lane.finished > 0 && parentStatus === "running"
                  ? "is-done"
                  : "";
        // Parent → lane, with the decision label riding the edge.
        edges.push({
          d: edgePath(box, child.box),
          cls: laneCls,
          flowing: !draft && lane.running,
          label: child.label,
          labelX: child.box.x + child.box.w / 2,
          labelY: child.box.y - 7,
        });
        if (child.back) {
          // Loop-back arc lane → parent.
          edges.push({
            d: loopBackPath(child.box, box),
            cls: `is-back ${laneCls}`,
            flowing: false,
            back: true,
          });
        }
        // Lane → merge on the next node.
        const mergeCls = draft
          ? "is-draft"
          : state === "taken" &&
              (nextStatus === "succeeded" ||
                nextStatus === "failed" ||
                nextStatus === "running")
            ? nextStatus === "running"
              ? "is-flowing"
              : "is-done"
            : state === "untaken"
              ? "is-untaken"
              : "";
        edges.push({
          d: edgePath(child.box, target),
          cls: mergeCls,
          flowing: !draft && state === "taken" && nextStatus === "running",
        });
      }
      prevIdx = i;
      prevFanned = true;
      continue;
    }

    prev = box;
    prevIdx = i;
  }
  // Final edge into the out node — skipped when the last step fanned
  // out (its lanes already converge on `out`).
  if (!prevFanned) {
    const from = statusOf(prevIdx);
    const to = statusOf(steps.length);
    edges.push({
      d: edgePath(prev, geo.out),
      cls: chainCls(from, to),
      flowing: false,
    });
  }

  // Goto routes: an edge from the router's left side down a side rail
  // into the target step (or the result node for `end`). Staggered
  // rails keep parallel routes apart; taken/untaken styling mirrors
  // the lanes.
  let railIndex = 0;
  for (let i = 0; i < steps.length; i += 1) {
    const routes = routeSpecs(steps[i]);
    for (let r = 0; r < routes.length; r += 1) {
      const route = routes[r];
      const from = geo.steps[i].box;
      const target =
        route.target === "end"
          ? geo.out
          : (geo.steps[steps.findIndex((s) => s.activity_id === route.target)]
              ?.box ?? geo.out);
      const rail = 14 + (railIndex % 4) * 9;
      railIndex += 1;
      const state = takenState(steps[i], run, route.slot);
      const cls = draft
        ? "is-draft"
        : state === "taken"
          ? "is-done"
          : state === "untaken"
            ? "is-untaken"
            : "";
      edges.push({
        d: gotoPath(from, target, rail),
        cls: `is-route ${cls}`,
        flowing: false,
        back: true, // reuse the arrowhead marker
        label: route.label,
        labelX: from.x - 10,
        // Stagger labels when several routes leave the same step.
        labelY: from.y + from.h / 2 - 6 + r * 13,
        labelAnchor: "end",
      });
    }
  }

  return (
    <svg
      className="gc-edges"
      viewBox={`0 0 ${geo.width} ${geo.height}`}
      style={{ width: geo.width, height: geo.height }}
      aria-hidden
    >
      <defs>
        <marker
          id="gc-loop-arrow"
          markerWidth="7"
          markerHeight="7"
          refX="5"
          refY="3.5"
          orient="auto"
        >
          <path d="M 0 0 L 6 3.5 L 0 7 z" className="gc-loop-arrowhead" />
        </marker>
      </defs>
      {edges.map((e, i) => (
        <path
          key={i}
          className={`gc-edge ${e.cls}`}
          d={e.d}
          markerEnd={e.back ? "url(#gc-loop-arrow)" : undefined}
        />
      ))}
      {edges
        .filter((e) => e.label != null)
        .map((e, i) => (
          <text
            key={`l${i}`}
            className={`gc-edge-label ${e.cls.includes("is-untaken") ? "is-untaken" : ""}`}
            x={e.labelX}
            y={e.labelY}
            textAnchor={e.labelAnchor ?? "middle"}
          >
            {e.label}
          </text>
        ))}
      {edges
        .filter((e) => e.flowing)
        .map((e, i) => (
          <circle key={`p${i}`} className="gc-pulse" r="3">
            <animateMotion dur="0.7s" repeatCount="indefinite" path={e.d} />
          </circle>
        ))}
    </svg>
  );
}

function childKindClass(kind: string): string {
  return KIND_CLASS[kind as StepSummary["kind"]] ?? "is-code";
}

/** One nested lane node under a control-flow step. */
function ChildNode({
  child,
  parentStatus,
  taken,
  lane,
  onSelect,
}: {
  child: ChildGeo;
  parentStatus: NodeStatus;
  taken: TakenState;
  lane: LaneLive;
  onSelect: () => void;
}) {
  void parentStatus; // lanes carry their own live state now
  const stateCls =
    taken === "taken"
      ? " is-taken"
      : taken === "untaken"
        ? " is-untaken"
        : lane.running
          ? " is-running"
          : lane.failed
            ? " is-failed"
            : "";
  // Loop/for_each lanes run many times; surface the live iteration count.
  const counter =
    child.back && (lane.running || lane.finished > 1)
      ? `×${lane.finished + (lane.running ? 1 : 0)}`
      : null;
  return (
    <div
      role="button"
      tabIndex={0}
      className={`gc-child${stateCls}`}
      style={{
        left: child.box.x,
        top: child.box.y,
        width: child.box.w,
        height: child.box.h,
      }}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      title={`${child.slot} — inspect the parent step`}
    >
      <span className="gc-child-slot">{child.label}</span>
      {counter != null && (
        <span className="gc-child-counter" aria-label="iterations">
          {counter}
        </span>
      )}
      <span className={`gc-node-kind ${childKindClass(child.kind)}`}>
        {child.kind === "mcp_tool" ? "mcp" : child.kind}
      </span>
      {taken === "taken" && (
        <span className="gc-child-check" aria-hidden>
          ✓
        </span>
      )}
    </div>
  );
}

function RunNode({
  box,
  phase,
  run,
  ready,
  total,
  doneCount,
  onRun,
}: {
  box: Box;
  phase: CanvasPhase;
  run: RunState | null;
  ready: boolean;
  total: number;
  doneCount: number;
  onRun: (dryRun: boolean) => void;
}) {
  let cls = "is-ready";
  let icon = "▶";
  let label = "RUN WORKFLOW";
  let hint = "⌘⏎";
  let disabled = !ready;

  if (phase === "writing" || phase === "proposed" || !ready) {
    cls = "is-blocked";
    icon = "·";
    label = "NOT RUNNABLE YET";
    hint = "";
    disabled = true;
  } else if (phase === "running") {
    cls = "is-running";
    icon = "◆";
    label = run?.dry ? "DRY RUN" : "RUNNING";
    hint = `${doneCount}/${total}`;
    disabled = true;
  } else if (phase === "done") {
    cls = "is-done";
    icon = "↺";
    label = "RUN AGAIN";
  } else if (phase === "failed") {
    cls = "is-failed";
    icon = "↻";
    label = "RETRY";
  }

  return (
    <button
      type="button"
      className={`gc-run ${cls}`}
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      disabled={disabled}
      onClick={() => onRun(false)}
      title={
        disabled
          ? "This workflow cannot run right now"
          : "Run this workflow for real (⌘⏎)"
      }
    >
      <span className="gc-run-icon" aria-hidden>
        {icon}
      </span>
      <span>{label}</span>
      {hint && <span className="gc-run-hint">{hint}</span>}
    </button>
  );
}

const KIND_CLASS: Record<StepSummary["kind"], string> = {
  cli: "is-cli",
  code: "is-code",
  llm: "is-llm",
  mcp_tool: "is-mcp",
  builtin: "is-flow",
};

/** Human labels for builtin control-flow sub-kinds. */
const BUILTIN_LABEL: Record<string, string> = {
  branch: "if / else",
  switch: "switch",
  for_each: "for each",
  loop: "loop",
  wait: "wait",
  map: "map",
  parallel: "parallel",
};

/** The badge text for a step's kind pill. */
function kindBadge(step: StepSummary): string {
  if (step.kind === "mcp_tool") return "mcp";
  if (step.kind === "builtin") {
    return (step.builtin && BUILTIN_LABEL[step.builtin]) || "builtin";
  }
  return step.kind;
}

function formatWaitSpec(wait: {
  timeout_ms?: number;
  until?: string;
  signal?: string;
}): string {
  const parts: string[] = [];
  if (wait.signal) parts.push(`event \`${wait.signal}\``);
  if (wait.until) parts.push(`until ${wait.until}`);
  if (wait.timeout_ms != null) parts.push(formatDuration(wait.timeout_ms));
  return parts.join(" · ");
}

/** One-line control-flow summary for a builtin node's subtitle. */
function builtinSub(step: StepSummary): string | null {
  if (step.kind !== "builtin" || !step.builtin) return null;
  const detail = step.builtin_detail;
  switch (step.builtin) {
    // branch / switch paths render as child lanes on the canvas; the
    // node subtitle keeps the human description instead.
    case "branch":
    case "switch":
      return null;
    case "for_each":
      return detail?.max_items != null ? `≤ ${detail.max_items} items` : null;
    case "loop":
      return detail?.max_iterations != null
        ? `≤ ${detail.max_iterations} iterations`
        : null;
    case "wait":
      return detail?.wait ? formatWaitSpec(detail.wait) : null;
    default:
      return "not executed in v1";
  }
}

function StepNode({
  step,
  index,
  box,
  status,
  live,
  selected,
  writing,
  change,
  median,
  elapsedMs,
  onSelect,
}: {
  step: StepSummary;
  index: number;
  box: Box;
  status: NodeStatus;
  live: LiveStep | undefined;
  selected: boolean;
  writing: boolean;
  change: ProposalChange | undefined;
  median: StepMedianEntry | undefined;
  elapsedMs: number | null;
  onSelect: () => void;
}) {
  const runningFor =
    status === "running" && live?.started_at_ms != null
      ? Math.max(0, performance.now() - live.started_at_ms)
      : null;

  let state: string;
  let stateCls: string;
  if (status === "succeeded") {
    state = `✓ ${live?.duration_ms != null ? formatDuration(live.duration_ms) : ""}`;
    stateCls = "is-ok";
  } else if (status === "failed") {
    state = `✗ ${live?.duration_ms != null ? formatDuration(live.duration_ms) : ""}`;
    stateCls = "is-bad";
  } else if (status === "not_taken") {
    state = "not taken";
    stateCls = "is-muted";
  } else if (status === "skipped") {
    state = "skipped";
    stateCls = "is-muted";
  } else if (status === "running") {
    state = runningFor != null ? `${(runningFor / 1000).toFixed(1)}s` : "…";
    stateCls = "is-live";
  } else if (change === "added" || writing) {
    state = writing ? "…" : "new";
    stateCls = "is-live";
  } else if (change === "modified" || change === "renamed") {
    state = "edited";
    stateCls = "is-live";
  } else {
    state = "·";
    stateCls = "is-muted";
  }

  // Progress against this step's own median; indeterminate without one.
  const progress =
    status === "running"
      ? median && runningFor != null
        ? Math.min(94, Math.max(6, (runningFor / median.median_ms) * 100))
        : null
      : status === "succeeded" || status === "failed"
        ? 100
        : 0;
  void elapsedMs; // ticking prop: its arrival re-renders the live clock

  const sub =
    step.kind === "llm" && step.llm_resolution
      ? `${step.llm_resolution.display_name} · ${step.llm_resolution.level} → ${step.llm_resolution.model}`
      : builtinSub(step) || step.description || placementLabel(step);

  return (
    <div
      role="button"
      tabIndex={0}
      className={`gc-node is-${status}${selected ? " is-selected" : ""}${writing ? " is-writing" : ""}`}
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      title={`${step.name} — inspect`}
    >
      <span className="gc-node-num">{String(index + 1).padStart(2, "0")}</span>
      <span className="gc-node-body">
        <span className="gc-node-name">{step.name}</span>
        <span className="gc-node-sub">{sub}</span>
      </span>
      <span className={`gc-node-kind ${KIND_CLASS[step.kind]}`}>
        {kindBadge(step)}
      </span>
      <span className={`gc-node-state ${stateCls}`}>{state}</span>
      {status === "running" && progress == null && (
        <span className="gc-node-progress is-indeterminate" aria-hidden />
      )}
      {progress != null && progress > 0 && status === "running" && (
        <span
          className="gc-node-progress"
          style={{ width: `${progress}%` }}
          aria-hidden
        />
      )}
    </div>
  );
}

function OutNode({
  box,
  phase,
  run,
  stepCount,
}: {
  box: Box;
  phase: CanvasPhase;
  run: RunState | null;
  stepCount: number;
}) {
  if (stepCount === 0) return null;
  const trace = run?.trace;
  const ok = phase === "done" && trace?.status === "succeeded";
  const label = ok
    ? (trace?.result?.headline ??
      `${trace?.activities.length ?? 0} steps · ${formatDuration(trace?.duration_ms ?? 0)}`)
    : "result";
  return (
    <div
      className={`gc-out${ok ? " is-ok" : ""}`}
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      title={ok ? label : "The run's declared result lands here"}
    >
      <span aria-hidden>{ok ? "✓" : "·"}</span>
      <span className="gc-out-label">{label}</span>
    </div>
  );
}

function placementLabel(step: StepSummary): string {
  if (step.placement.type === "capability") return `via ${step.placement.id}`;
  if (step.placement.type === "local_fs") return "this machine · filesystem";
  return "any worker";
}

// ─── Proposal bar ────────────────────────────────────────────────────────

/**
 * The frozen review card an agent submitted with `propose`, as the
 * canvas's bottom bar. Accept publishes the next version; reject stops
 * the session, optionally rewinding the folder to its pre-session state.
 */
function ProposalBar({
  session,
  proposal,
  onReviewStep,
  onSessionChanged,
}: {
  session: AuthoringSession;
  proposal: SessionProposal;
  onReviewStep: (activityId: string) => void;
  onSessionChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const act = (p: Promise<unknown>) => {
    setBusy(true);
    setError(null);
    p.then(() => onSessionChanged())
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  const changed = proposal.steps.find((st) => st.change !== "unchanged");
  const filesChanged = proposal.files.length;

  return (
    <div className="gc-proposal" role="region" aria-label="Agent proposal">
      <span className="gc-proposal-pill">proposed</span>
      <div className="gc-proposal-body">
        <div className="gc-proposal-summary">{proposal.summary}</div>
        <div className="gc-proposal-meta">
          {proposalRollupLine(proposal)} · {filesChanged} file
          {filesChanged === 1 ? "" : "s"} changed · publishes v
          {proposal.manifest_version + 1}
          {proposal.warnings.length > 0 &&
            ` · ${proposal.warnings.length} lint warning${proposal.warnings.length === 1 ? "" : "s"}`}
        </div>
        {error && <div className="gc-proposal-error">{error}</div>}
      </div>
      {changed && (
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => onReviewStep(changed.activity_id)}
        >
          Review
        </button>
      )}
      <button
        type="button"
        className="btn"
        disabled={busy}
        title="Stop the session; the files stay on disk for a human to pick up"
        onClick={() =>
          act(
            rejectAuthoringProposal(
              session.session_id,
              "rejected from the Console",
              false,
            ),
          )
        }
      >
        Reject
      </button>
      <button
        type="button"
        className="btn"
        disabled={busy}
        title="Stop the session and rewind the folder to its pre-session state (exact, via the journal)"
        onClick={() =>
          act(
            rejectAuthoringProposal(
              session.session_id,
              "rejected from the Console",
              true,
            ),
          )
        }
      >
        Reject &amp; discard
      </button>
      <button
        type="button"
        className="btn primary"
        disabled={busy}
        title={`Publish v${proposal.manifest_version + 1} and stop the session — acceptance is the approval`}
        onClick={() => act(acceptAuthoringProposal(session.session_id))}
      >
        Accept &amp; publish
      </button>
    </div>
  );
}

function proposalRollupLine(p: SessionProposal): string {
  const r = p.rollup;
  const parts: string[] = [];
  if (r.pure) parts.push(`${r.pure} pure`);
  if (r.reads) parts.push(`${r.reads} read${r.reads === 1 ? "" : "s"}`);
  if (r.prompts) parts.push(`${r.prompts} prompt${r.prompts === 1 ? "" : "s"}`);
  if (r.may_writes)
    parts.push(`${r.may_writes} may-write${r.may_writes === 1 ? "" : "s"}`);
  const effects = parts.length > 0 ? parts.join(", ") : "no effects";
  const external =
    r.external > 0 ? ` · ${r.external} reaching beyond this machine` : "";
  return `${p.steps.length} step${p.steps.length === 1 ? "" : "s"}: ${effects}${external}`;
}

// ─── Trace bar (gantt) ───────────────────────────────────────────────────

/** The settled run as horizontal time, one bar per activity. */
function TraceBar({
  trace,
  onSelect,
}: {
  trace: RunTrace;
  onSelect: (activityId: string) => void;
}) {
  const t0 = Date.parse(trace.started_at);
  const total = Math.max(1, trace.duration_ms);
  const cost = trace.cost.total_eur;
  return (
    <div className="gc-trace" role="region" aria-label="Run trace">
      <div className="gc-trace-head">
        <span className="label">Trace</span>
        <span className="gc-trace-meta">
          {trace.run_id} · {trace.activities.length} activities ·{" "}
          {formatDuration(trace.duration_ms)}
          {cost > 0 ? ` · ${formatCost(cost)}` : ""}
        </span>
      </div>
      <div className="gc-trace-rows">
        {trace.activities.map((a) => {
          const left = Math.max(
            0,
            Math.min(100, ((Date.parse(a.started_at) - t0) / total) * 100),
          );
          const width = Math.max(2, (a.duration_ms / total) * 100);
          const failed = a.status === "failed";
          return (
            <button
              key={a.activity_id}
              type="button"
              className={`gc-trace-row${failed ? " is-failed" : ""}`}
              onClick={() => onSelect(a.activity_id)}
            >
              <span className="gc-trace-name">{a.step_name}</span>
              <span className="gc-trace-track">
                <span
                  className="gc-trace-fill"
                  style={{ left: `${left}%`, width: `${Math.min(width, 100 - left)}%` }}
                />
              </span>
              <span className="gc-trace-dur">
                {formatDuration(a.duration_ms)}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ─── Inspector ───────────────────────────────────────────────────────────

type InspectorTab =
  | "step"
  | "args"
  | "output"
  | "capability"
  | "timing"
  | "journal"
  | "diff";

function Inspector({
  step,
  index,
  status,
  live,
  trace,
  capabilities,
  median,
  session,
  proposalStep,
  onClose,
  onSessionChanged,
}: {
  step: StepSummary;
  index: number;
  status: NodeStatus;
  live: LiveStep | undefined;
  trace: RunTrace | null;
  capabilities: Capability[];
  median: StepMedianEntry | undefined;
  session: AuthoringSession | null;
  proposalStep: ProposalStep | null;
  onClose: () => void;
  onSessionChanged: () => void;
}) {
  const hasSession = session != null;
  const tabs: InspectorTab[] = [
    "step",
    "args",
    "output",
    "capability",
    "timing",
    ...(hasSession ? (["journal"] as InspectorTab[]) : []),
    ...(proposalStep ? (["diff"] as InspectorTab[]) : []),
  ];
  const [tab, setTab] = useState<InspectorTab>(proposalStep ? "diff" : "step");
  useEffect(() => {
    if (!tabs.includes(tab)) setTab("step");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step.activity_id, hasSession, proposalStep != null]);

  const activity =
    trace?.activities.find((a) => a.activity_id === step.activity_id) ?? null;

  const stateLabel =
    status === "succeeded"
      ? `✓ ${live?.duration_ms != null ? formatDuration(live.duration_ms) : ""}`
      : status === "failed"
        ? `✗ ${live?.error ?? "failed"}`
        : status === "running"
          ? "running"
          : status === "idle"
            ? "compiled"
            : status;

  return (
    <aside className="gc-inspector" aria-label={`Step ${step.name}`}>
      <div className="gc-inspector-head">
        <span className="gc-inspector-num">
          {String(index + 1).padStart(2, "0")}
        </span>
        <span className="gc-inspector-name" title={step.name}>
          {step.name}
        </span>
        <span className="gc-inspector-kind">{kindBadge(step)}</span>
        <span
          className={`gc-inspector-state ${
            status === "succeeded"
              ? "is-ok"
              : status === "failed"
                ? "is-bad"
                : status === "running"
                  ? "is-live"
                  : "is-muted"
          }`}
        >
          {stateLabel}
        </span>
        <button
          type="button"
          className="gc-inspector-close"
          onClick={onClose}
          aria-label="Close inspector (Esc)"
        >
          ✕
        </button>
      </div>
      <div className="gc-inspector-tabs" role="tablist">
        {tabs.map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={tab === t}
            className={tab === t ? "is-active" : ""}
            onClick={() => setTab(t)}
          >
            {t}
          </button>
        ))}
      </div>
      <div className="gc-inspector-body">
        {tab === "step" && <StepDetail step={step} />}
        {tab === "args" && (
          <ActivityJson
            value={activity?.input_summary}
            note="exactly the bytes the worker received — resolved, not templated"
            empty="No arguments yet — this step has not run in this trace."
          />
        )}
        {tab === "output" && (
          <ActivityJson
            value={activity?.output_summary ?? activity?.output}
            note={
              activity
                ? `persisted with the trace in ~/.cori/runs/`
                : undefined
            }
            empty="No output yet — this step has not run in this trace."
          />
        )}
        {tab === "capability" && (
          <CapabilityDetail step={step} capabilities={capabilities} />
        )}
        {tab === "timing" && (
          <TimingDetail
            step={step}
            live={live}
            activity={activity}
            median={median}
          />
        )}
        {tab === "journal" && session && (
          <JournalDetail session={session} onSessionChanged={onSessionChanged} />
        )}
        {tab === "diff" && proposalStep && (
          <DiffDetail proposalStep={proposalStep} />
        )}
      </div>
    </aside>
  );
}

function Kv({ rows }: { rows: Array<[string, string, string?]> }) {
  return (
    <div className="gc-kv">
      {rows.map(([k, v, cls]) => (
        <div key={k} className="gc-kv-row">
          <span className="gc-kv-key">{k}</span>
          <span className={`gc-kv-val${cls ? ` ${cls}` : ""}`}>{v}</span>
        </div>
      ))}
    </div>
  );
}

function StepDetail({ step }: { step: StepSummary }) {
  const rows: Array<[string, string, string?]> = [
    [
      "kind",
      step.kind === "builtin" && step.builtin
        ? `builtin · ${BUILTIN_LABEL[step.builtin] ?? step.builtin}`
        : step.kind,
    ],
    ["placement", placementLabel(step)],
  ];
  if (step.kind === "cli" && step.binary) {
    rows.push(["binary", step.binary]);
  }
  if (step.kind === "mcp_tool") {
    if (step.server) rows.push(["server", step.server]);
    if (step.tool) rows.push(["tool", step.tool]);
  }
  if (step.kind === "builtin") {
    const detail = step.builtin_detail;
    const nested = detail?.nested ?? {};
    const slots = Object.entries(nested);
    if (slots.length > 0) {
      rows.push([
        "paths",
        slots
          .map(([slot, spec]) =>
            spec.goto != null
              ? `${slot} → ${spec.goto_name ?? spec.goto}`
              : `${slot} (${spec.kind ?? "code"})`,
          )
          .join(", "),
      ]);
    }
    if (detail?.wait) {
      rows.push(["waits for", formatWaitSpec(detail.wait)]);
    }
    if (detail?.max_items != null) {
      rows.push(["item cap", String(detail.max_items)]);
    }
    if (detail?.max_iterations != null) {
      rows.push(["iteration cap", String(detail.max_iterations)]);
    }
    if (step.builtin === "map" || step.builtin === "parallel") {
      rows.push(["status", "deferred — the runtime skips this step", "is-warn"]);
    }
  }
  if (step.kind === "llm" && step.llm_resolution) {
    rows.push([
      "model",
      `${step.llm_resolution.level} → ${step.llm_resolution.model}`,
      "is-warn",
    ]);
    rows.push(["provider", step.llm_resolution.display_name]);
    rows.push(["billed", "declared llm step — the only kind that can cost", "is-warn"]);
  }
  return (
    <>
      {step.description && (
        <div className="gc-inspector-note">{step.description}</div>
      )}
      <Kv rows={rows} />
      {step.kind === "llm" && step.llm_resolution && (
        <div className="gc-inspector-provider">
          <BackendLogo backendId={step.llm_resolution.backend_id} size={14} />
          <span>{step.llm_resolution.display_name}</span>
        </div>
      )}
      <WhatRunsDetail key={step.activity_id} step={step} />
    </>
  );
}

/** One-line caption above the extracted expression, per step kind. */
const WHAT_RUNS_NOTE: Record<string, string> = {
  cli: "argv built from the step input at run time",
  code: "runs in the Deno sandbox — read-only, no network",
  llm: "sent to the AI provider with the accumulated input",
  mcp_tool: "arguments built from the step input at run time",
  builtin: "pure function of the accumulated input — picks the path",
};

/**
 * What this step actually executes, lifted verbatim from its source
 * file: the `cli` command, the `code` run function, the `llm` prompt,
 * the `mcp_tool` args, or the builtin's control-flow selector. The full
 * step source stays one click away — extraction can fail on
 * non-canonical files, the source cannot.
 */
function WhatRunsDetail({ step }: { step: StepSummary }) {
  const [showSource, setShowSource] = useState(false);
  if (step.source == null) {
    return (
      <div className="gc-inspector-note is-muted">
        Source not available — {step.source_path} could not be read back.
      </div>
    );
  }
  const spec = whatRuns(step);
  const snippet = spec != null ? stepField(step.source, spec.field) : null;
  // No extractable expression (non-canonical file, or a kind without
  // one): the source itself is the detail, shown outright.
  const sourceOpen = showSource || snippet == null;
  return (
    <div className="gc-whatruns">
      {snippet != null && spec != null && (
        <>
          <div className="gc-whatruns-head">
            <span className="gc-whatruns-key">{spec.key}</span>
            <span className="gc-whatruns-note">{WHAT_RUNS_NOTE[step.kind]}</span>
          </div>
          <pre className="gc-inspector-code">{snippet}</pre>
        </>
      )}
      {snippet != null ? (
        <button
          type="button"
          className="gc-whatruns-toggle"
          onClick={() => setShowSource((s) => !s)}
        >
          {sourceOpen ? "hide" : "view"} {step.source_path}
        </button>
      ) : (
        <div className="gc-whatruns-head">
          <span className="gc-whatruns-key">{step.source_path}</span>
        </div>
      )}
      {sourceOpen && <pre className="gc-inspector-code">{step.source}</pre>}
    </div>
  );
}

function ActivityJson({
  value,
  note,
  empty,
}: {
  value: unknown;
  note?: string;
  empty: string;
}) {
  if (value == null) {
    return <div className="gc-inspector-note is-muted">{empty}</div>;
  }
  let text: string;
  try {
    text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  } catch {
    text = String(value);
  }
  return (
    <>
      <pre className="gc-inspector-code">{text}</pre>
      {note && <div className="gc-inspector-note is-muted">{note}</div>}
    </>
  );
}

function CapabilityDetail({
  step,
  capabilities,
}: {
  step: StepSummary;
  capabilities: Capability[];
}) {
  if (step.placement.type !== "capability") {
    return (
      <Kv
        rows={[
          [
            "capability",
            step.kind === "code"
              ? "none — sandboxed code step"
              : "none required",
            "is-cyan",
          ],
          [
            "effects",
            step.kind === "code"
              ? "no network, no filesystem, no credentials"
              : "runs wherever the plan places it",
          ],
        ]}
      />
    );
  }
  const id = step.placement.id;
  const cap = capabilities.find((c) => c.id === id);
  const rows: Array<[string, string, string?]> = [
    ["capability", `${id} · declared in tools_required`, cap?.authed ? "is-ok" : "is-bad"],
    [
      "auth",
      cap == null
        ? "unknown — worker did not report it"
        : cap.authed
          ? (cap.detail ?? "ready")
          : (cap.detail ?? `needs sign-in — cori login ${id}`),
      cap?.authed ? "is-ok" : "is-bad",
    ],
    ["credential", "held by the worker — never in workflow code"],
  ];
  return <Kv rows={rows} />;
}

function TimingDetail({
  step,
  live,
  activity,
  median,
}: {
  step: StepSummary;
  live: LiveStep | undefined;
  activity: ActivityTrace | null;
  median: StepMedianEntry | undefined;
}) {
  const dur = activity?.duration_ms ?? live?.duration_ms;
  const rows: Array<[string, string, string?]> = [];
  if (dur != null) rows.push(["duration", formatDuration(dur), "is-cyan"]);
  if (median != null) {
    rows.push([
      "median",
      `${formatDuration(median.median_ms)} over ${median.samples} real run${median.samples === 1 ? "" : "s"}`,
    ]);
    if (dur != null) {
      const delta = dur - median.median_ms;
      if (Math.abs(delta) >= Math.max(median.median_ms * 0.05, 100)) {
        rows.push([
          "delta",
          `${delta > 0 ? "+" : "−"}${formatDuration(Math.abs(delta))} vs median`,
          delta > 0 ? "is-warn" : "is-ok",
        ]);
      }
    }
  }
  if (activity != null) {
    rows.push(["attempts", String(activity.attempts)]);
    if (activity.cost_eur != null && activity.cost_eur > 0) {
      rows.push([
        "cost",
        `${formatCost(activity.cost_eur)}${
          activity.tokens
            ? ` · ${activity.tokens.input_tokens} in / ${activity.tokens.output_tokens} out tokens`
            : ""
        }`,
        "is-warn",
      ]);
    } else {
      rows.push([
        "cost",
        step.kind === "llm" ? "not recorded" : "€0.00 · not billed",
      ]);
    }
  }
  if (rows.length === 0) {
    return (
      <div className="gc-inspector-note is-muted">
        No timing yet — run the workflow (or a dry run) to record it.
      </div>
    );
  }
  return <Kv rows={rows} />;
}

/**
 * The session's journal, oldest-first — every write attributable, and
 * (while the agent is still writing) rewindable to any file operation.
 */
function JournalDetail({
  session,
  onSessionChanged,
}: {
  session: AuthoringSession;
  onSessionChanged: () => void;
}) {
  const [events, setEvents] = useState<AuthoringSessionEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const writing = session.state === "writing";

  // `current_seq` bumps on every journalled op — refetch on each bump so
  // the ledger is live without a timer.
  useEffect(() => {
    let cancelled = false;
    sessionJournal(session.session_id)
      .then((rows) => {
        if (!cancelled) {
          setEvents(rows);
          setError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [session.session_id, session.current_seq]);

  const rewindTo = (seq: number) => {
    setBusy(true);
    rewindAuthoringSession(session.session_id, seq)
      .then(() => onSessionChanged())
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  if (error) return <div className="gc-inspector-note is-bad">{error}</div>;
  if (events == null)
    return <div className="gc-inspector-note is-muted">Loading journal…</div>;

  return (
    <>
      <div className="gc-journal">
        {events.map((e) => (
          <div key={e.seq} className="gc-journal-row">
            <span className="gc-journal-seq">
              {String(e.seq).padStart(3, "0")}
            </span>
            <span className="gc-journal-text">{sessionEventSummary(e)}</span>
            {writing && isFileOp(e) && (
              <button
                type="button"
                className="gc-journal-rewind"
                disabled={busy}
                title={`Rewind the folder to the state after event ${e.seq}`}
                onClick={() => rewindTo(e.seq)}
              >
                ⌘Z here
              </button>
            )}
          </div>
        ))}
      </div>
      <div className="gc-inspector-note is-accent">
        every write attributable to {session.agent} · rewind restores any
        earlier state
      </div>
    </>
  );
}

/** What the session did to this step's source file — never a claim. */
function DiffDetail({ proposalStep }: { proposalStep: ProposalStep }) {
  const access =
    proposalStep.access === "none"
      ? "pure"
      : proposalStep.access.replace("_", " ");
  return (
    <Kv
      rows={[
        [
          "change",
          proposalStep.change +
            (proposalStep.renamed_from
              ? ` from ${proposalStep.renamed_from}`
              : ""),
          proposalStep.change === "added"
            ? "is-ok"
            : proposalStep.change === "deleted"
              ? "is-bad"
              : undefined,
        ],
        ["file", proposalStep.source_path],
        ...(proposalStep.source_sha256
          ? ([
              ["sha256", proposalStep.source_sha256.slice(0, 12)],
            ] as Array<[string, string]>)
          : []),
        [
          "effect",
          `${access}${proposalStep.external ? " · reaches beyond this machine" : ""}`,
          proposalStep.access === "may_write" ? "is-warn" : undefined,
        ],
        ["target", proposalStep.target || "—"],
      ]}
    />
  );
}

// ─── Shared helpers ──────────────────────────────────────────────────────

function isFileOp(e: AuthoringSessionEvent): boolean {
  return (
    e.kind === "file_written" ||
    e.kind === "file_deleted" ||
    e.kind === "file_renamed"
  );
}

export function sessionEventSummary(e: AuthoringSessionEvent): string {
  switch (e.kind) {
    case "file_written":
      return `wrote ${e.rel_path ?? "?"}`;
    case "file_deleted":
      return `deleted ${e.rel_path ?? "?"}`;
    case "file_renamed":
      return `renamed ${e.rel_path ?? "?"} → ${e.to_rel_path ?? "?"}`;
    case "rewind":
      return e.note ?? "rewind";
    case "proposal_submitted":
      return e.note ? `proposed — ${e.note}` : "proposal submitted";
    case "proposal_accepted":
      return e.note ?? "proposal accepted";
    case "proposal_rejected":
      return e.note ?? "proposal rejected";
    case "approval_granted":
      return e.note ?? "approval granted";
    case "session_started":
      return "session started";
    case "session_stopped":
      return e.note ? `stopped — ${e.note}` : "stopped";
    default:
      return e.kind;
  }
}

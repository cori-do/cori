import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useRevalidator } from "react-router";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MiddleTruncate } from "../components/middle-truncate";
import { ThemeIconButton } from "../components/theme-icon-button";
import {
  WorkflowPane,
  type WorkflowPaneHandle,
} from "../components/workflow-pane";
import {
  decideApproval,
  getCliInstallStatus,
  getLastLocalDir,
  getStackStatus,
  getStatus,
  installCli,
  installUpdate,
  isIpcError,
  listApprovals,
  onUpdaterAvailable,
  listDir,
  listRecentWorkflows,
  listRemoteWorkflows,
  nearestExistingDirectory,
  onApprovalsChanged,
  onStackStatus,
  peekSource,
  sourceToCli,
  type ApprovalRequest,
  type DirEntry,
  type DirListing,
  type PeekResult,
  type RecentWorkflow,
  type RemoteListing,
  type RemoteWorkflowEntry,
  type StackStatus,
  type StatusResponse,
} from "../lib/api";
import { fuzzyFilter } from "../lib/fuzzy";
import {
  STARTER_CATEGORIES,
  STARTER_WORKFLOWS,
  starterCategory,
  starterEffectLabel,
  startersInCategory,
  type StarterCategory,
  type StarterWorkflow,
} from "../lib/starter-workflows";
import { openRun, openSettings } from "../lib/windows";
import { Inbox } from "./manage.approvals";
import { ScheduleList } from "./manage.schedules";

export function meta() {
  return [{ title: "Cori" }];
}

interface LauncherData {
  status: StatusResponse | null;
  recents: RecentWorkflow[];
}

const LAUNCHER_LIST_WIDTH_KEY = "cori-launcher-list-width";
const DEFAULT_LAUNCHER_LIST_WIDTH = 240;
const MIN_LAUNCHER_LIST_WIDTH = 192;
const MAX_LAUNCHER_LIST_WIDTH = 480;
const MIN_LAUNCHER_DETAIL_WIDTH = 320;
const LAUNCHER_LIST_KEYBOARD_STEP = 16;

function readLauncherListWidth(): number {
  if (typeof window === "undefined") return DEFAULT_LAUNCHER_LIST_WIDTH;
  try {
    const raw = window.localStorage.getItem(LAUNCHER_LIST_WIDTH_KEY);
    const stored = raw === null || raw.trim() === "" ? Number.NaN : Number(raw);
    if (Number.isFinite(stored)) {
      return Math.min(
        MAX_LAUNCHER_LIST_WIDTH,
        Math.max(MIN_LAUNCHER_LIST_WIDTH, stored),
      );
    }
  } catch {
    // Storage can be unavailable in privacy-restricted webviews.
  }
  return DEFAULT_LAUNCHER_LIST_WIDTH;
}

function saveLauncherListWidth(width: number) {
  try {
    window.localStorage.setItem(LAUNCHER_LIST_WIDTH_KEY, String(width));
  } catch {
    // Resizing still works for this session when storage is unavailable.
  }
}

export async function clientLoader(): Promise<LauncherData> {
  const [recents, status] = await Promise.all([
    listRecentWorkflows().catch(() => [] as RecentWorkflow[]),
    getStatus().catch(() => null),
  ]);
  return { recents, status };
}

// ─── Context model ────────────────────────────────────────────────────────

type LauncherContext =
  | { kind: "recents" }
  | { kind: "library"; category: StarterCategory }
  | {
      kind: "local";
      path: string;
      listing: DirListing | null;
      loading: boolean;
      error: string | null;
    }
  | {
      kind: "remote";
      refStr: string;
      listing: RemoteListing | null;
      loading: boolean;
      error: string | null;
    };

type LauncherSection = "workflows" | "inbox" | "schedules";

/**
 * Unified items model for the results pane. Each context yields a
 * sequence of `ListedItem`s; the selection / keyboard nav code below
 * doesn't care which kind it's looking at.
 */
type ListedItem =
  | { kind: "recent"; recent: RecentWorkflow; key: string }
  | { kind: "starter"; starter: StarterWorkflow; key: string }
  | { kind: "dir-entry"; entry: DirEntry; key: string }
  | {
      kind: "remote-entry";
      entry: RemoteWorkflowEntry;
      listing: RemoteListing;
      key: string;
    };

export default function Launcher({ loaderData }: { loaderData: LauncherData }) {
  const { recents, status } = loaderData;
  const revalidator = useRevalidator();
  const [input, setInput] = useState("");
  const [peek, setPeek] = useState<PeekResult | null>(null);
  const [selIndex, setSelIndex] = useState(0);
  const [stack, setStack] = useState<StackStatus | undefined>(undefined);
  const [ctx, setCtx] = useState<LauncherContext>(() =>
    recents.length === 0
      ? { kind: "library", category: "all" }
      : { kind: "recents" },
  );
  const [dragOver, setDragOver] = useState(false);
  const [listWidth, setListWidth] = useState(readLauncherListWidth);
  const [listResizing, setListResizing] = useState(false);
  const [section, setSection] = useState<LauncherSection>("workflows");
  // The workflow filling the right pane. Picking one on the left sets
  // this; nothing about it opens a window.
  const [picked, setPicked] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const paneRef = useRef<WorkflowPaneHandle>(null);
  const panesRef = useRef<HTMLDivElement>(null);
  const listWidthRef = useRef(listWidth);
  // Bumped on every async context-entry request; only the latest
  // response is applied (older ones land in the background but are
  // discarded). Avoids the user-typed-faster race.
  const contextRequestId = useRef(0);

  // Stack-status snapshot + live subscription for the footer indicator.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    getStackStatus()
      .then((s) => !cancelled && setStack(s))
      .catch(() => {});
    onStackStatus((s) => !cancelled && setStack(s))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Approval inbox: snapshot + live subscription. Items are human
  // gates (MCP run confirms, trust consent) — surfaced above the
  // search bar until decided.
  const [approvals, setApprovals] = useState<ApprovalRequest[]>([]);
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    listApprovals()
      .then((a) => !cancelled && setApprovals(a))
      .catch(() => {});
    onApprovalsChanged((pending) => !cancelled && setApprovals(pending))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Deep links (cori://…) land here after the Rust side surfaces the
  // launcher: the link is a doorbell, the UI decides what to open.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ kind: string; run_id?: string }>("deeplink:open", (e) => {
      const p = e.payload;
      if (p?.kind === "inbox") setSection("inbox");
      else if (p?.kind === "run" && p.run_id) void openRun(p.run_id);
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Tray navigation for the three launcher sections stays in this window.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ section?: LauncherSection }>(
      "launcher:set-section",
      (e) => {
        const next = e.payload?.section;
        if (next === "workflows" || next === "inbox" || next === "schedules") {
          setSection(next);
        }
      },
    )
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Settings is the only tabbed destination that opens a separate window.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ tab?: "runs" | "capabilities" | "providers" | "workers" }>(
      "tray:open-settings",
      (e) => void openSettings(e.payload?.tab),
    )
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // `peek_source` on every keystroke. Cheap on the backend, so no
  // debounce — the chip reacts instantly so Enter is never a surprise.
  useEffect(() => {
    let cancelled = false;
    peekSource(input)
      .then((p) => !cancelled && setPeek(p))
      .catch(() => {
        if (!cancelled) setPeek(null);
      });
    return () => {
      cancelled = true;
    };
  }, [input]);

  // The workflow directory is reconstructed from persisted run traces,
  // so the loader's snapshot goes stale when another source is run for
  // the first time. Re-run the loader whenever the launcher regains
  // focus — individual run details live in the selected workflow pane.
  useEffect(() => {
    if (!isTauri()) return;
    const w = getCurrentWebviewWindow();
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    w.onFocusChanged(({ payload: focused }) => {
      if (focused) revalidator.revalidate();
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [revalidator]);

  // ⌘/Ctrl-L focuses + selects the bar contents from anywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // ─── Context loading ──────────────────────────────────────────────────

  const enterLocalContext = useCallback((path: string) => {
    const reqId = ++contextRequestId.current;
    setCtx({ kind: "local", path, listing: null, loading: true, error: null });
    setInput("");
    setSelIndex(0);
    listDir(path)
      .then((listing) => {
        if (reqId !== contextRequestId.current) return;
        setCtx({
          kind: "local",
          path: listing.path,
          listing,
          loading: false,
          error: null,
        });
        // Land on the first real entry, not the ".." row — so Enter
        // right after drilling in doesn't bounce back up.
        setSelIndex(listing.parent && listing.entries.length > 0 ? 1 : 0);
      })
      .catch((e) => {
        if (reqId !== contextRequestId.current) return;
        setCtx({
          kind: "local",
          path,
          listing: null,
          loading: false,
          error: formatErr(e),
        });
      });
  }, []);

  const locateMissingWorkflow = useCallback(
    async (source: string) => {
      const start = await nearestExistingDirectory(source);
      setSection("workflows");
      enterLocalContext(start);
      inputRef.current?.focus();
    },
    [enterLocalContext],
  );

  const enterRemoteContext = useCallback(
    (refStr: string, update = false) => {
      const reqId = ++contextRequestId.current;
      // Preserve the existing listing while a refresh is in flight so
      // the user sees what they're refreshing instead of a flash of
      // empty pane. Initial loads have no prior listing — null is fine.
      setCtx((prev) => ({
        kind: "remote",
        refStr,
        listing:
          prev.kind === "remote" && prev.refStr === refStr
            ? prev.listing
            : null,
        loading: true,
        error: null,
      }));
      if (!update) {
        setInput("");
        setSelIndex(0);
      }
      listRemoteWorkflows(refStr, update)
        .then((listing) => {
          if (reqId !== contextRequestId.current) return;
          setCtx({
            kind: "remote",
            refStr,
            listing,
            loading: false,
            error: null,
          });
        })
        .catch((e) => {
          if (reqId !== contextRequestId.current) return;
          setCtx({
            kind: "remote",
            refStr,
            listing: null,
            loading: false,
            error: formatErr(e),
          });
        });
    },
    [],
  );

  const popContext = useCallback(() => {
    contextRequestId.current += 1; // discard any in-flight context load
    setCtx({ kind: "recents" });
    setInput("");
    setSelIndex(0);
  }, []);

  const enterLibraryContext = useCallback((category: StarterCategory) => {
    contextRequestId.current += 1;
    setCtx({ kind: "library", category });
    setSection("workflows");
    setPicked(null);
    setInput("");
    setSelIndex(0);
  }, []);

  const showFrequentlyRun = useCallback(() => {
    contextRequestId.current += 1;
    setCtx({ kind: "recents" });
    setSection("workflows");
    setPicked(null);
    setInput("");
    setSelIndex(0);
  }, []);

  // Drop-a-folder support. Workflow folder → openLaunch directly;
  // plain folder → enter the local-browse context. Multi-path drops:
  // pick the first path that classifies as a local directory.
  useEffect(() => {
    if (!isTauri()) return;
    const w = getCurrentWebviewWindow();
    let unlistenFn: UnlistenFn | undefined;
    let cancelled = false;

    const handleDrop = async (paths: string[]) => {
      for (const path of paths) {
        try {
          const peek = await peekSource(path);
          if (peek.kind === "local" && peek.local_exists) {
            if (peek.is_workflow_dir) {
              setPicked(peek.normalized);
            } else {
              enterLocalContext(peek.normalized);
            }
            return;
          }
        } catch {
          // try the next dropped path
        }
      }
    };

    w.onDragDropEvent((event) => {
      if (cancelled) return;
      const payload = event.payload;
      switch (payload.type) {
        case "enter":
        case "over":
          setDragOver(true);
          break;
        case "leave":
          setDragOver(false);
          break;
        case "drop":
          setDragOver(false);
          void handleDrop(payload.paths);
          break;
      }
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlistenFn = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  }, [enterLocalContext]);

  // ─── Items derivation ────────────────────────────────────────────────

  const items = useMemo<ListedItem[]>(() => {
    if (ctx.kind === "recents") {
      const frequentWorkflows = rankFrequentlyRunWorkflows(recents);
      return fuzzyFilter(frequentWorkflows, input.trim(), (r) => [
        r.name ?? "",
        r.workflow_id,
        describeRecentSource(r),
      ]).map((r, i) => ({
        kind: "recent",
        recent: r,
        key: `${r.key}-${i}`,
      }));
    }
    if (ctx.kind === "library") {
      return fuzzyFilter(
        startersInCategory(ctx.category),
        input.trim(),
        (workflow) => [
          workflow.name,
          workflow.description,
          ...workflow.tags,
          ...workflow.tools,
          ...(workflow.google_services ?? []),
        ],
      ).map((starter) => ({
        kind: "starter",
        starter,
        key: `starter:${starter.id}`,
      }));
    }
    if (ctx.kind === "local") {
      const entries = ctx.listing?.entries ?? [];
      const matches = fuzzyFilter(entries, input.trim(), (e) => e.name).map(
        (e): ListedItem => ({
          kind: "dir-entry",
          entry: e,
          key: e.path,
        }),
      );
      // A ".." row leads back up — only while not filtering, so a typed
      // filter matches folder contents alone.
      const parent = ctx.listing?.parent;
      if (parent && input.trim().length === 0) {
        return [
          {
            kind: "dir-entry",
            entry: { name: "..", kind: "dir", path: parent },
            key: `parent:${parent}`,
          },
          ...matches,
        ];
      }
      return matches;
    }
    if (ctx.kind === "remote") {
      const listing = ctx.listing;
      if (!listing) return [];
      return fuzzyFilter(listing.workflows, input.trim(), (w) => [
        w.subpath || w.name,
        w.name,
        w.description,
      ]).map((w) => ({
        kind: "remote-entry",
        entry: w,
        listing,
        key: `${listing.sha}:${w.subpath || "."}`,
      }));
    }
    return [];
  }, [ctx, recents, input]);

  // Keep selection in bounds whenever the filtered list shrinks.
  useEffect(() => {
    setSelIndex((i) => Math.min(i, Math.max(0, items.length - 1)));
  }, [items.length]);

  function moveSelection(delta: 1 | -1) {
    setSelIndex((i) => {
      if (items.length === 0) return 0;
      const next = i + delta;
      if (next < 0) return 0;
      if (next >= items.length) return items.length - 1;
      return next;
    });
  }

  /** Fill the right pane with a workflow. Folders drill in instead. */
  function activateItem(item: ListedItem) {
    if (item.kind === "recent") {
      const src = sourceToCli(item.recent.source);
      if (src) {
        setPicked(src);
        setSection("workflows");
      }
      return;
    }
    if (item.kind === "starter") {
      setPicked(item.starter.source);
      setSection("workflows");
      return;
    }
    if (item.kind === "dir-entry") {
      const e = item.entry;
      if (e.kind === "workflow") {
        setPicked(e.path);
        setSection("workflows");
        return;
      }
      if (e.kind === "dir") {
        enterLocalContext(e.path);
        return;
      }
      // plain file — selectable but does nothing
      return;
    }
    if (item.kind === "remote-entry") {
      setPicked(buildRemoteSource(item.listing, item.entry));
      setSection("workflows");
      return;
    }
  }

  /** The source string the highlighted row would put in the pane, if any. */
  function sourceOf(item: ListedItem | undefined): string | null {
    if (!item) return null;
    if (item.kind === "recent") return sourceToCli(item.recent.source);
    if (item.kind === "starter") return item.starter.source;
    if (item.kind === "dir-entry")
      return item.entry.kind === "workflow" ? item.entry.path : null;
    return buildRemoteSource(item.listing, item.entry);
  }

  function handleEnter() {
    // When the bar has a typed input, Enter prefers the classifier
    // outcome over selecting from the current list — the user just
    // told us what they want.
    if (peek && input.trim().length > 0) {
      if (peek.kind === "local" && peek.local_exists) {
        enterLocalContext(peek.normalized);
        return;
      }
      if (peek.kind === "remote") {
        if (remoteRefHasSubpath(peek.normalized)) {
          // A subpath-bearing ref names a specific workflow — put it
          // straight in the pane, which surfaces consent or capability
          // gaps as it resolves.
          setPicked(peek.normalized);
          return;
        }
        // Bare `host/owner/repo[@ref]` — list the repo's workflows so
        // the user can pick a subpath.
        enterRemoteContext(peek.normalized);
        return;
      }
    }

    const item = items[selIndex];
    if (!item) return;

    // Open it. Press run. The second Enter on a workflow already in the
    // pane starts it, rather than resolving the same source again.
    if (sourceOf(item) === picked && paneRef.current?.canRun()) {
      paneRef.current.run();
      return;
    }
    activateItem(item);
  }

  function handleArrowRight() {
    // → drills into a highlighted folder (or activates a workflow,
    // same as Enter).
    const item = items[selIndex];
    if (!item) return;
    if (item.kind === "dir-entry" && item.entry.kind === "dir") {
      enterLocalContext(item.entry.path);
    }
  }

  function handleBarKey(e: React.KeyboardEvent<HTMLInputElement>) {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        moveSelection(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        moveSelection(-1);
        break;
      case "ArrowRight":
        // Only intercept → when the caret is at the end of the input;
        // otherwise let it move the cursor normally.
        if (
          e.currentTarget.selectionStart === e.currentTarget.value.length &&
          e.currentTarget.selectionEnd === e.currentTarget.value.length
        ) {
          e.preventDefault();
          handleArrowRight();
        }
        break;
      case "ArrowLeft":
        // ← climbs to the parent folder, mirroring →'s drill-in. Only
        // when the bar is empty so it never fights caret movement.
        if (input.length === 0 && ctx.kind === "local" && ctx.listing?.parent) {
          e.preventDefault();
          enterLocalContext(ctx.listing.parent);
        }
        break;
      case "Enter":
        e.preventDefault();
        handleEnter();
        break;
      case "Escape":
        e.preventDefault();
        if (input.length > 0) {
          setInput("");
        } else if (ctx.kind !== "recents") {
          popContext();
        }
        break;
      case "Backspace":
        if (input.length === 0 && ctx.kind !== "recents") {
          e.preventDefault();
          // In a folder, Backspace climbs one level; only at the
          // filesystem root (or in remote context) does it pop home.
          if (ctx.kind === "local" && ctx.listing?.parent) {
            enterLocalContext(ctx.listing.parent);
          } else {
            popContext();
          }
        }
        break;
    }
  }

  function maxListWidth(): number {
    const panesWidth = panesRef.current?.getBoundingClientRect().width;
    if (panesWidth === undefined) return MAX_LAUNCHER_LIST_WIDTH;
    return Math.max(
      MIN_LAUNCHER_LIST_WIDTH,
      Math.min(
        MAX_LAUNCHER_LIST_WIDTH,
        panesWidth - MIN_LAUNCHER_DETAIL_WIDTH,
      ),
    );
  }

  function updateListWidth(nextWidth: number, persist = false) {
    const next = Math.round(
      Math.min(maxListWidth(), Math.max(MIN_LAUNCHER_LIST_WIDTH, nextWidth)),
    );
    listWidthRef.current = next;
    setListWidth(next);
    if (persist) saveLauncherListWidth(next);
  }

  function resizeListFromPointer(e: ReactPointerEvent<HTMLDivElement>) {
    const panes = panesRef.current;
    if (!panes) return;
    updateListWidth(e.clientX - panes.getBoundingClientRect().left);
  }

  function handleListResizeStart(e: ReactPointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    setListResizing(true);
    resizeListFromPointer(e);
  }

  function handleListResizeEnd(e: ReactPointerEvent<HTMLDivElement>) {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    setListResizing(false);
    saveLauncherListWidth(listWidthRef.current);
  }

  function handleListResizeKey(e: React.KeyboardEvent<HTMLDivElement>) {
    let next: number | undefined;
    if (e.key === "ArrowLeft") {
      next = listWidthRef.current - LAUNCHER_LIST_KEYBOARD_STEP;
    } else if (e.key === "ArrowRight") {
      next = listWidthRef.current + LAUNCHER_LIST_KEYBOARD_STEP;
    } else if (e.key === "Home") {
      next = MIN_LAUNCHER_LIST_WIDTH;
    } else if (e.key === "End") {
      next = maxListWidth();
    }
    if (next === undefined) return;
    e.preventDefault();
    updateListWidth(next, true);
  }

  return (
    <div className={`launcher${dragOver ? " is-drag-over" : ""}`}>
      <header
        className="launcher-head"
        data-tauri-drag-region="deep"
        onDoubleClick={toggleLauncherMaximize}
      >
        <WindowControls />
        <span className="launcher-head-divider" aria-hidden />
        <img
          src="/cori-mark.png"
          alt=""
          className="launcher-mark"
          width={18}
          height={18}
          draggable={false}
        />
        <div className="launcher-title">cori</div>
        <div className="launcher-head-spacer" />
        <EngineBadge stack={stack} status={status} />
        <ThemeIconButton />
      </header>

      <UpdateBanner />

      <FirstRunCliPrompt />

      {approvals.length > 0 && section !== "inbox" && (
        <ApprovalsPanel
          approvals={approvals}
          onOpenInbox={() => setSection("inbox")}
        />
      )}

      <SearchBar
        value={input}
        onChange={setInput}
        onKeyDown={handleBarKey}
        peek={peek}
        inputRef={inputRef}
        placeholder={placeholderFor(ctx)}
        onBrowse={() => {
          void getLastLocalDir().then((p) => enterLocalContext(p));
        }}
      />

      {/* Your workflows on the left, the one you picked on the right. */}
      <div
        ref={panesRef}
        className={`launcher-panes${listResizing ? " is-resizing" : ""}`}
        style={
          { "--launcher-list-width": `${listWidth}px` } as CSSProperties
        }
      >
        <div className="launcher-list">
          <LauncherSectionNav
            section={section}
            context={ctx}
            pendingApprovals={approvals.length}
            onSelectSection={setSection}
            onShowFrequentlyRun={showFrequentlyRun}
            onShowLibrary={enterLibraryContext}
          />

          {ctx.kind !== "recents" && (
            <Breadcrumb
              context={ctx}
              onPop={popContext}
              onRefresh={
                ctx.kind === "remote"
                  ? () => enterRemoteContext(ctx.refStr, true)
                  : undefined
              }
            />
          )}

          <ResultsPane
            ctx={ctx}
            items={items}
            selectedIndex={selIndex}
            pickedSource={picked}
            sourceOf={sourceOf}
            onSelect={(i) => {
              setSelIndex(i);
              activateItem(items[i]);
            }}
            onHover={setSelIndex}
            inputIsEmpty={input.length === 0}
            recentsCount={recents.length}
            onOpenLibrary={() => enterLibraryContext("all")}
          />
        </div>

        <div
          className="launcher-pane-resizer"
          role="separator"
          aria-label="Resize workflow list"
          aria-orientation="vertical"
          aria-valuemin={MIN_LAUNCHER_LIST_WIDTH}
          aria-valuemax={MAX_LAUNCHER_LIST_WIDTH}
          aria-valuenow={listWidth}
          tabIndex={0}
          title="Drag to resize the workflow list; double-click to reset"
          onPointerDown={handleListResizeStart}
          onPointerMove={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              resizeListFromPointer(e);
            }
          }}
          onPointerUp={handleListResizeEnd}
          onPointerCancel={handleListResizeEnd}
          onLostPointerCapture={() => setListResizing(false)}
          onKeyDown={handleListResizeKey}
          onDoubleClick={() =>
            updateListWidth(DEFAULT_LAUNCHER_LIST_WIDTH, true)
          }
        />

        <div className="launcher-detail">
          {section === "workflows" && (
            ctx.kind === "library" && picked === null ? (
              <StarterLibraryHome
                category={ctx.category}
                searchQuery={input.trim()}
                visibleWorkflows={items.flatMap((item) =>
                  item.kind === "starter" ? [item.starter] : [],
                )}
                onChangeCategory={enterLibraryContext}
                onPick={(starter) => {
                  const index = items.findIndex(
                    (item) =>
                      item.kind === "starter" &&
                      item.starter.id === starter.id,
                  );
                  if (index >= 0) setSelIndex(index);
                  setPicked(starter.source);
                }}
              />
            ) : (
              <WorkflowPane
                source={picked}
                handleRef={paneRef}
                onLocateMissing={locateMissingWorkflow}
              />
            )
          )}
          {section === "inbox" && (
            <LauncherSectionContent title="Inbox">
              <Inbox />
            </LauncherSectionContent>
          )}
          {section === "schedules" && (
            <LauncherSectionContent title="Schedules">
              <ScheduleList />
            </LauncherSectionContent>
          )}
        </div>
      </div>

      <footer className="launcher-foot">
        <MachineFacts status={status} />
        <div className="launcher-foot-actions">
          <CliInstallAction />
          <button
            type="button"
            className="btn"
            onClick={() => void openSettings()}
            title="History, capabilities, AI providers, and workers"
          >
            Settings
          </button>
        </div>
      </footer>
    </div>
  );
}

function LauncherSectionNav({
  section,
  context,
  pendingApprovals,
  onSelectSection,
  onShowFrequentlyRun,
  onShowLibrary,
}: {
  section: LauncherSection;
  context: LauncherContext;
  pendingApprovals: number;
  onSelectSection: (section: LauncherSection) => void;
  onShowFrequentlyRun: () => void;
  onShowLibrary: (category: StarterCategory) => void;
}) {
  const workflowActive = section === "workflows";
  const frequentActive = workflowActive && context.kind === "recents";
  const libraryActive =
    workflowActive && context.kind === "library" && context.category === "all";
  const workspaceActive =
    workflowActive &&
    context.kind === "library" &&
    context.category === "google_workspace";
  const localActive =
    workflowActive && context.kind === "library" && context.category === "local";

  return (
    <nav className="launcher-sections" aria-label="Launcher sections">
      <div className="launcher-nav-group">
        <div className="launcher-nav-label">My workflows</div>
        <button
          type="button"
          className={`launcher-section${frequentActive ? " is-active" : ""}`}
          aria-current={frequentActive ? "page" : undefined}
          onClick={onShowFrequentlyRun}
        >
          <LauncherSectionIcon kind="frequent" />
          <span>Frequently run</span>
        </button>
      </div>

      <div className="launcher-nav-group">
        <div className="launcher-nav-label">Discover</div>
        <button
          type="button"
          className={`launcher-section${libraryActive ? " is-active" : ""}`}
          aria-current={libraryActive ? "page" : undefined}
          onClick={() => onShowLibrary("all")}
        >
          <LauncherSectionIcon kind="library" />
          <span>Starter library</span>
        </button>
        <button
          type="button"
          className={`launcher-section${workspaceActive ? " is-active" : ""}`}
          aria-current={workspaceActive ? "page" : undefined}
          onClick={() => onShowLibrary("google_workspace")}
        >
          <LauncherSectionIcon kind="google" />
          <span>Google Workspace</span>
        </button>
        <button
          type="button"
          className={`launcher-section${localActive ? " is-active" : ""}`}
          aria-current={localActive ? "page" : undefined}
          onClick={() => onShowLibrary("local")}
        >
          <LauncherSectionIcon kind="local" />
          <span>Local computer</span>
        </button>
      </div>

      <div className="launcher-nav-group">
        <div className="launcher-nav-label">Activity</div>
        <button
          type="button"
          className={`launcher-section${section === "inbox" ? " is-active" : ""}`}
          aria-current={section === "inbox" ? "page" : undefined}
          onClick={() => onSelectSection("inbox")}
        >
          <LauncherSectionIcon kind="inbox" />
          <span>Inbox</span>
          {pendingApprovals > 0 && (
            <span
              className="launcher-section-count"
              aria-label={`${pendingApprovals} pending`}
            >
              {pendingApprovals}
            </span>
          )}
        </button>
        <button
          type="button"
          className={`launcher-section${section === "schedules" ? " is-active" : ""}`}
          aria-current={section === "schedules" ? "page" : undefined}
          onClick={() => onSelectSection("schedules")}
        >
          <LauncherSectionIcon kind="schedules" />
          <span>Schedules</span>
        </button>
      </div>
    </nav>
  );
}

type LauncherNavIconKind =
  | "frequent"
  | "library"
  | "google"
  | "local"
  | "inbox"
  | "schedules";

function LauncherSectionIcon({ kind }: { kind: LauncherNavIconKind }) {
  if (kind === "inbox") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden>
        <path d="M2.5 3.5h11v8.5h-11zM2.5 9h3l1 1.5h3L10.5 9h3" />
      </svg>
    );
  }
  if (kind === "schedules") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden>
        <circle cx="8" cy="8.5" r="5.5" />
        <path d="M8 5.2v3.6l2.3 1.4M5 1.8v2M11 1.8v2" />
      </svg>
    );
  }
  if (kind === "library") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden>
        <path d="m8 2 .8 2.2L11 5l-2.2.8L8 8l-.8-2.2L5 5l2.2-.8zM12.2 8.2l.5 1.3 1.3.5-1.3.5-.5 1.3-.5-1.3-1.3-.5 1.3-.5zM3.5 9.5v3h5" />
      </svg>
    );
  }
  if (kind === "google") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden>
        <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1" />
        <rect x="9" y="2.5" width="4.5" height="4.5" rx="1" />
        <rect x="2.5" y="9" width="4.5" height="4.5" rx="1" />
        <rect x="9" y="9" width="4.5" height="4.5" rx="1" />
      </svg>
    );
  }
  if (kind === "local") {
    return (
      <svg viewBox="0 0 16 16" aria-hidden>
        <rect x="2.5" y="3" width="11" height="8" rx="1.5" />
        <path d="M6 13h4M8 11v2" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 16 16" aria-hidden>
      <path d="M3 11.5V8.7M6.3 11.5V6M9.7 11.5V3.5M13 11.5V5" />
    </svg>
  );
}

function LauncherSectionContent({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="launcher-section-content">
      <h1>{title}</h1>
      {children}
    </section>
  );
}

function WindowControls() {
  const act = (action: "close" | "minimize" | "maximize") => {
    if (!isTauri()) return;
    const window = getCurrentWebviewWindow();
    if (action === "close") void window.close();
    else if (action === "minimize") void window.minimize();
    else void window.toggleMaximize();
  };

  return (
    <div className="window-controls" aria-label="Window controls">
      <button
        type="button"
        className="window-control is-close"
        onClick={() => act("close")}
        onDoubleClick={(event) => event.stopPropagation()}
        aria-label="Close launcher"
        title="Close launcher"
      >
        <WindowControlGlyph kind="close" />
      </button>
      <button
        type="button"
        className="window-control is-minimize"
        onClick={() => act("minimize")}
        onDoubleClick={(event) => event.stopPropagation()}
        aria-label="Minimize launcher"
        title="Minimize launcher"
      >
        <WindowControlGlyph kind="minimize" />
      </button>
      <button
        type="button"
        className="window-control is-maximize"
        onClick={() => act("maximize")}
        onDoubleClick={(event) => event.stopPropagation()}
        aria-label="Maximize launcher"
        title="Maximize launcher"
      >
        <WindowControlGlyph kind="maximize" />
      </button>
    </div>
  );
}

function toggleLauncherMaximize() {
  if (!isTauri()) return;
  void getCurrentWebviewWindow().toggleMaximize();
}

function WindowControlGlyph({
  kind,
}: {
  kind: "close" | "minimize" | "maximize";
}) {
  if (kind === "close") {
    return (
      <svg viewBox="0 0 8 8" aria-hidden>
        <path d="m2 2 4 4M6 2 2 6" />
      </svg>
    );
  }
  if (kind === "minimize") {
    return (
      <svg viewBox="0 0 8 8" aria-hidden>
        <path d="M1.5 4h5" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 8 8" aria-hidden>
      <path d="m2 5.75 3.75-3.5M3.25 2.25h2.5v2.5M4.75 5.75h-2.5v-2.5" />
    </svg>
  );
}

// ─── Approvals panel ─────────────────────────────────────────────────────

/**
 * Compact attention banner for pending human gates. The launcher is an
 * interrupter, not a reading surface: it shows what's being asked and
 * the primary action; the readable detail (per-param table, history)
 * lives in the launcher's Inbox section.
 */
function ApprovalsPanel({
  approvals,
  onOpenInbox,
}: {
  approvals: ApprovalRequest[];
  onOpenInbox: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const decide = (nonce: string, approved: boolean) => {
    setBusy(nonce);
    decideApproval(nonce, approved)
      // The watcher's `approvals:changed` removes the row; on error just
      // release the buttons (the item may have expired meanwhile).
      .catch(() => {})
      .finally(() => setBusy((b) => (b === nonce ? null : b)));
  };
  return (
    <div className="approvals" role="region" aria-label="Pending approvals">
      <div className="approvals-header">
        <span>
          {approvals.length} approval{approvals.length > 1 ? "s" : ""} waiting
        </span>
        <button
          type="button"
          className="approvals-open-inbox"
          onClick={onOpenInbox}
        >
          Open inbox →
        </button>
      </div>
      {approvals.map((a) => {
        const isAction = a.kind === "reauth_required";
        return (
          <div key={a.nonce} className="approval-row">
            <div className="approval-body">
              <div className="approval-head">
                <span className={`pill ${approvalPill(a.kind)}`}>
                  {approvalKindLabel(a.kind)}
                </span>
                <span className="approval-from">via {a.requested_by}</span>
              </div>
              <div className="approval-summary">{approvalSummary(a)}</div>
            </div>
            <div className="approval-actions">
              {isAction ? (
                <button
                  type="button"
                  className="btn"
                  disabled={busy === a.nonce}
                  onClick={() => decide(a.nonce, false)}
                >
                  Dismiss
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    className="btn approval-approve"
                    disabled={busy === a.nonce}
                    onClick={() => decide(a.nonce, true)}
                  >
                    Approve
                  </button>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy === a.nonce}
                    onClick={() => decide(a.nonce, false)}
                  >
                    Decline
                  </button>
                </>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/**
 * One readable line per item — structured facts over requester prose.
 * The full message + per-param table are one click away in the Inbox.
 */
function approvalSummary(a: ApprovalRequest): string {
  const p = a.payload;
  const name =
    (typeof p.workflow_name === "string" && p.workflow_name) ||
    (typeof p.workflow_id === "string" && p.workflow_id) ||
    (typeof p.remote_ref === "string" && p.remote_ref) ||
    (typeof p.source === "string" && p.source) ||
    "";
  switch (a.kind) {
    case "run_confirm": {
      const nParams =
        p.params && typeof p.params === "object"
          ? Object.keys(p.params as object).length
          : 0;
      const dry = p.dry_run === true ? " · dry run" : "";
      return `Run "${name}" — ${String(p.steps ?? "?")} steps, ${nParams} param${nParams === 1 ? "" : "s"}${dry}`;
    }
    case "trust_consent":
      return `Trust ${name} @ ${typeof p.sha === "string" ? p.sha.slice(0, 8) : "?"} (first run)`;
    case "schedule_reconsent":
      return `Schedule for "${name}" paused — workflow changed upstream`;
    case "step_gate":
      return `"${name}" is waiting on step approval`;
    case "reauth_required":
      return `${String(p.capability ?? "a capability")} needs sign-in — ${String(p.login_command ?? "")}`;
  }
}

function approvalKindLabel(kind: ApprovalRequest["kind"]): string {
  switch (kind) {
    case "run_confirm":
      return "run request";
    case "trust_consent":
      return "trust request";
    case "schedule_reconsent":
      return "schedule changed";
    case "step_gate":
      return "step approval";
    case "reauth_required":
      return "sign-in needed";
  }
}

function approvalPill(kind: ApprovalRequest["kind"]): string {
  return kind === "trust_consent" ? "bad" : "warn";
}

function placeholderFor(ctx: LauncherContext): string {
  if (ctx.kind === "library")
    return `Search ${starterCategory(ctx.category).label.toLowerCase()}`;
  if (ctx.kind === "local")
    return "Filter folder, or type a new path to navigate";
  if (ctx.kind === "remote")
    return "Filter workflows in this repo, or type a new path / ref";
  return "Type to filter, or paste a path / host/owner/repo";
}

/**
 * Build a `cori run`-compatible source string for a remote workflow
 * selected from a repo listing. Combines the listing's `host/repo`
 * and original `@ref` with the workflow's `subpath`.
 */
function buildRemoteSource(
  listing: RemoteListing,
  entry: RemoteWorkflowEntry,
): string {
  const base = entry.subpath
    ? `${listing.host}/${listing.repo}/${entry.subpath}`
    : `${listing.host}/${listing.repo}`;
  return listing.ref_str ? `${base}@${listing.ref_str}` : base;
}

// ─── SearchBar ────────────────────────────────────────────────────────────

interface SearchBarProps {
  value: string;
  onChange: (v: string) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  peek: PeekResult | null;
  inputRef: React.RefObject<HTMLInputElement | null>;
  placeholder: string;
  onBrowse: () => void;
}

/** ⌘ on Apple hardware, Ctrl everywhere else — the same key the global
 *  handler above listens for. Read once: it cannot change at runtime. */
const FOCUS_KEY_HINT = /Mac|iPhone|iPad/.test(
  typeof navigator === "undefined" ? "" : navigator.userAgent,
)
  ? "⌘L"
  : "^L";

function SearchBar({
  value,
  onChange,
  onKeyDown,
  peek,
  inputRef,
  placeholder,
  onBrowse,
}: SearchBarProps) {
  return (
    <div className="search-bar">
      {/* One recessed pill holds the shortcut, the input and the chip, so
          the bar reads as a single control rather than three. */}
      <div className="search-bar-field">
        <span className="search-bar-key" aria-hidden>
          {FOCUS_KEY_HINT}
        </span>
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          aria-label="Search workflows or paste a path / ref"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          autoFocus
        />
        {value.length > 0 && <Chip peek={peek} />}
      </div>
      <button
        type="button"
        className="search-bar-browse"
        onClick={onBrowse}
        title="Browse a local folder"
        aria-label="Browse a local folder"
      >
        <FolderIcon />
      </button>
    </div>
  );
}

function Chip({ peek }: { peek: PeekResult | null }) {
  if (!peek) return null;
  if (peek.kind === "filter") {
    return <span className="search-bar-chip">filter</span>;
  }
  if (peek.kind === "local") {
    return (
      <span
        className="search-bar-chip local"
        title={peek.normalized + (peek.local_exists ? "" : " — not found")}
      >
        folder{!peek.local_exists && " ✗"}
      </span>
    );
  }
  return (
    <span className="search-bar-chip remote" title={peek.normalized}>
      <MiddleTruncate
        text={peek.normalized}
        tail={Math.min(20, Math.floor(peek.normalized.length / 2))}
        className="search-bar-chip-label"
      />
    </span>
  );
}

// ─── Breadcrumb ───────────────────────────────────────────────────────────

function Breadcrumb({
  context,
  onPop,
  onRefresh,
}: {
  context: LauncherContext;
  onPop: () => void;
  /** Only meaningful in remote context — re-resolve the ref + re-list. */
  onRefresh?: () => void;
}) {
  if (context.kind === "recents") {
    return (
      <div className="crumb">
        <span className="crumb-label">Workflows</span>
      </div>
    );
  }
  if (context.kind === "library") {
    return (
      <div className="crumb">
        <button
          type="button"
          className="crumb-pop"
          onClick={onPop}
          title="Back to frequently run workflows (Esc)"
          aria-label="Back to frequently run workflows"
        >
          ←
        </button>
        <span className="crumb-value">
          {starterCategory(context.category).label}
        </span>
        <span className="crumb-count">
          {startersInCategory(context.category).length}
        </span>
      </div>
    );
  }
  if (context.kind === "local") {
    return (
      <div className="crumb">
        <button
          type="button"
          className="crumb-pop"
          onClick={onPop}
          title="Back to workflows (Esc)"
          aria-label="Back to workflows"
        >
          ←
        </button>
        <MiddleTruncate
          text={context.path}
          tail={Math.min(28, Math.floor(context.path.length / 2))}
          className="crumb-value"
        />
      </div>
    );
  }
  if (context.kind === "remote") {
    const pin = remoteCrumbText(context);
    const refreshing = context.loading;
    return (
      <div className="crumb">
        <button
          type="button"
          className="crumb-pop"
          onClick={onPop}
          title="Back to workflows (Esc)"
          aria-label="Back to workflows"
        >
          ←
        </button>
        <MiddleTruncate
          text={pin}
          tail={Math.min(24, Math.max(8, Math.floor(pin.length / 2)))}
          className="crumb-value"
        />
        {onRefresh && (
          <button
            type="button"
            className={`crumb-refresh${refreshing ? " is-spinning" : ""}`}
            onClick={onRefresh}
            disabled={refreshing}
            title={
              refreshing
                ? "Refreshing…"
                : "Refresh — re-resolve the ref and re-list workflows"
            }
            aria-label="Refresh repository cache"
          >
            <RefreshIcon />
          </button>
        )}
      </div>
    );
  }
  return null;
}

function remoteCrumbText(
  ctx: Extract<LauncherContext, { kind: "remote" }>,
): string {
  if (ctx.listing) {
    const refPart = ctx.listing.ref_str ? ` @ ${ctx.listing.ref_str}` : "";
    const shaPart = ` · ${ctx.listing.sha.slice(0, 8)}`;
    return `${ctx.listing.host}/${ctx.listing.repo}${refPart}${shaPart}`;
  }
  // Loading or errored — show what the user typed.
  return ctx.refStr;
}

// ─── ResultsPane ──────────────────────────────────────────────────────────

interface ResultsPaneProps {
  ctx: LauncherContext;
  items: ListedItem[];
  selectedIndex: number;
  /** Source currently filling the right pane, so its row can say so. */
  pickedSource: string | null;
  sourceOf: (item: ListedItem | undefined) => string | null;
  onSelect: (index: number) => void;
  onHover: (index: number) => void;
  inputIsEmpty: boolean;
  recentsCount: number;
  onOpenLibrary: () => void;
}

function ResultsPane({
  ctx,
  items,
  selectedIndex,
  pickedSource,
  sourceOf,
  onSelect,
  onHover,
  inputIsEmpty,
  recentsCount,
  onOpenLibrary,
}: ResultsPaneProps) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  useEffect(() => {
    const el = refs.current[selectedIndex];
    if (el) el.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  if (ctx.kind === "local") {
    if (ctx.loading) {
      return (
        <div className="results">
          <div className="results-loading">Loading…</div>
        </div>
      );
    }
    if (ctx.error) {
      return (
        <div className="results">
          <div className="results-error">{ctx.error}</div>
        </div>
      );
    }
    if (items.length === 0) {
      return (
        <div className="results">
          <div className="results-empty">
            {inputIsEmpty
              ? "This folder is empty (no visible entries)."
              : "No match in this folder."}
          </div>
        </div>
      );
    }
  }

  if (ctx.kind === "remote") {
    if (ctx.loading) {
      return (
        <div className="results">
          <div className="results-loading">
            <span>Resolving + listing…</span>
          </div>
        </div>
      );
    }
    if (ctx.error) {
      return (
        <div className="results">
          <div className="results-error">{ctx.error}</div>
        </div>
      );
    }
    if (items.length === 0) {
      return (
        <div className="results">
          <div className="results-empty">
            {inputIsEmpty
              ? "No workflows found in this repo (looked for manifest.md)."
              : "No workflow matches that filter."}
          </div>
        </div>
      );
    }
  }

  if (ctx.kind === "library" && items.length === 0) {
    return (
      <div className="results">
        <div className="results-empty" role="status">
          No starter matches that search.
        </div>
      </div>
    );
  }

  if (ctx.kind === "recents" && items.length === 0) {
    if (!inputIsEmpty) {
      return (
        <div className="results">
          <div className="results-empty">
            No workflow matches that filter.
          </div>
        </div>
      );
    }
    if (recentsCount === 0) {
      return (
        <div className="results">
          <EmptyHistory onOpenLibrary={onOpenLibrary} />
        </div>
      );
    }
    return (
      <div className="results">
        <div className="results-empty">No workflows to show.</div>
      </div>
    );
  }

  return (
    <div className="results" role="listbox">
      {items.map((item, i) => (
        <ItemRow
          key={item.key}
          item={item}
          selected={i === selectedIndex}
          open={pickedSource != null && sourceOf(item) === pickedSource}
          onClick={() => onSelect(i)}
          onHover={() => onHover(i)}
          buttonRef={(el) => {
            refs.current[i] = el;
          }}
        />
      ))}
    </div>
  );
}

interface ItemRowProps {
  item: ListedItem;
  selected: boolean;
  /** This row's workflow is the one currently in the right pane. */
  open: boolean;
  onClick: () => void;
  onHover: () => void;
  buttonRef: (el: HTMLButtonElement | null) => void;
}

function ItemRow(props: ItemRowProps) {
  if (props.item.kind === "recent") {
    return <RecentRow {...props} item={props.item} />;
  }
  if (props.item.kind === "starter") {
    return <StarterRow {...props} item={props.item} />;
  }
  if (props.item.kind === "dir-entry") {
    return <DirEntryRow {...props} item={props.item} />;
  }
  return <RemoteEntryRow {...props} item={props.item} />;
}

function RecentRow({
  item,
  selected,
  open,
  onClick,
  onHover,
  buttonRef,
}: ItemRowProps & { item: Extract<ListedItem, { kind: "recent" }> }) {
  const r = item.recent;
  const src = sourceToCli(r.source);
  const sourceLabel = describeRecentSource(r);
  const displayName = r.name ?? r.workflow_id;
  const disabled = !src;
  return (
    <button
      type="button"
      ref={buttonRef}
      className={rowClass(selected, open)}
      onClick={onClick}
      onMouseEnter={onHover}
      disabled={disabled}
      role="option"
      aria-selected={selected}
      title={
        disabled
          ? "Older run — no recoverable source on disk"
          : `${displayName} — open in the pane`
      }
    >
      <span className="result-row-icon" aria-hidden>
        <WorkflowIcon />
      </span>
      <div className="result-row-body">
        <div className="result-row-name">{displayName}</div>
        <div className="result-row-meta">
          <span>
            {r.run_count} {r.run_count === 1 ? "run" : "runs"}
          </span>
          {sourceLabel && (
            <>
              <span aria-hidden>·</span>
              <MiddleTruncate
                text={sourceLabel}
                tail={Math.min(18, Math.floor(sourceLabel.length / 2))}
                className={
                  r.source?.kind === "remote"
                    ? "result-row-source is-remote"
                    : "result-row-source"
                }
              />
            </>
          )}
        </div>
      </div>
    </button>
  );
}

function StarterRow({
  item,
  selected,
  open,
  onClick,
  onHover,
  buttonRef,
}: ItemRowProps & { item: Extract<ListedItem, { kind: "starter" }> }) {
  const workflow = item.starter;
  const category = starterCategory(workflow.category);
  return (
    <button
      type="button"
      ref={buttonRef}
      className={rowClass(selected, open)}
      onClick={onClick}
      onMouseEnter={onHover}
      role="option"
      aria-selected={selected}
      aria-label={`${workflow.name}, ${category.label}, ${starterEffectLabel(workflow.effect)}`}
      title={`${workflow.name} — preview this starter`}
    >
      <span className="result-row-icon" aria-hidden>
        <StarterIcon category={workflow.category} />
      </span>
      <div className="result-row-body">
        <div className="result-row-name">{workflow.name}</div>
        <div className="result-row-meta">
          <span className="result-row-category">{category.shortLabel}</span>
          <span aria-hidden>·</span>
          <span>{starterEffectLabel(workflow.effect)}</span>
          {workflow.requires_llm && (
            <>
              <span aria-hidden>·</span>
              <span>AI</span>
            </>
          )}
        </div>
        <div className="result-row-desc">{workflow.description}</div>
      </div>
    </button>
  );
}

function DirEntryRow({
  item,
  selected,
  open,
  onClick,
  onHover,
  buttonRef,
}: ItemRowProps & { item: Extract<ListedItem, { kind: "dir-entry" }> }) {
  const e = item.entry;
  const Icon =
    e.kind === "workflow" ? WorkflowIcon : e.kind === "dir" ? FolderIcon : FileIcon;
  const disabled = e.kind === "file";
  const subtitle =
    e.kind === "workflow"
      ? "workflow · open"
      : e.kind === "dir"
        ? e.name === ".."
          ? "parent folder"
          : "folder · open"
        : e.symlink
          ? "symlink (not followed)"
          : "file";
  return (
    <button
      type="button"
      ref={buttonRef}
      className={rowClass(selected, open)}
      onClick={onClick}
      onMouseEnter={onHover}
      disabled={disabled}
      role="option"
      aria-selected={selected}
      title={e.path}
    >
      <span className="result-row-icon" aria-hidden>
        <Icon />
      </span>
      <div className="result-row-body">
        <div className="result-row-name">{e.name}</div>
        <div className="result-row-meta">
          <span>{subtitle}</span>
        </div>
      </div>
    </button>
  );
}

/**
 * True when a normalized remote ref carries a subpath beyond the bare
 * `host/owner/repo[@ref]` form. Mirrors `cori_run::remote::refspec`'s
 * parser by counting slashes in the pre-`@` portion.
 *
 *   github.com/acme/flows           → 2 slashes → no subpath
 *   github.com/acme/flows@v1        → 2 slashes → no subpath
 *   github.com/acme/flows/translate → 3 slashes → has subpath
 *   github.com/acme/flows//x@v1     → 3 slashes (explicit split) → has subpath
 */
function remoteRefHasSubpath(normalized: string): boolean {
  const preAt = normalized.split("@")[0] ?? normalized;
  // Strip a scheme like `https://` so `//` after the host doesn't
  // count toward the slash budget.
  const stripped = preAt.replace(/^[a-z]+:\/\//i, "");
  return (stripped.match(/\//g) ?? []).length >= 3;
}

function RemoteEntryRow({
  item,
  selected,
  open,
  onClick,
  onHover,
  buttonRef,
}: ItemRowProps & { item: Extract<ListedItem, { kind: "remote-entry" }> }) {
  const e = item.entry;
  const label = e.subpath || `${item.listing.repo} (root)`;
  return (
    <button
      type="button"
      ref={buttonRef}
      className={rowClass(selected, open)}
      onClick={onClick}
      onMouseEnter={onHover}
      role="option"
      aria-selected={selected}
      title={`${item.listing.host}/${item.listing.repo}/${e.subpath}`}
    >
      <span className="result-row-icon" aria-hidden>
        <WorkflowIcon />
      </span>
      <div className="result-row-body">
        <div className="result-row-name">{e.name || label}</div>
        <div className="result-row-meta">
          <MiddleTruncate
            text={label}
            tail={Math.min(22, Math.floor(label.length / 2))}
            className="result-row-source is-remote"
          />
        </div>
        {e.description && (
          <div className="result-row-desc">{e.description}</div>
        )}
      </div>
    </button>
  );
}

function StarterLibraryHome({
  category,
  searchQuery,
  visibleWorkflows,
  onChangeCategory,
  onPick,
}: {
  category: StarterCategory;
  searchQuery: string;
  visibleWorkflows: readonly StarterWorkflow[];
  onChangeCategory: (category: StarterCategory) => void;
  onPick: (workflow: StarterWorkflow) => void;
}) {
  const activeCategory = starterCategory(category);
  const workflows = [...visibleWorkflows].sort((a, b) => {
    if (category === "all" && Boolean(a.featured) !== Boolean(b.featured)) {
      return a.featured ? -1 : 1;
    }
    return a.name.localeCompare(b.name);
  });

  return (
    <section className="library-home" aria-labelledby="library-title">
      <header className="library-home-head">
        <div className="library-eyebrow">Ready-made workflows</div>
        <div className="library-title-row">
          <div>
            <h1 id="library-title">{activeCategory.label}</h1>
            <p>{activeCategory.description}</p>
          </div>
          <span className="library-total">
            {searchQuery
              ? `${workflows.length} ${workflows.length === 1 ? "match" : "matches"}`
              : category === "all"
              ? `${STARTER_WORKFLOWS.length} starters`
              : `${workflows.length} in this collection`}
          </span>
        </div>
        <div className="library-categories" aria-label="Starter categories">
          {STARTER_CATEGORIES.map((candidate) => (
            <button
              key={candidate.id}
              type="button"
              className={candidate.id === category ? "is-active" : ""}
              aria-pressed={candidate.id === category}
              onClick={() => onChangeCategory(candidate.id)}
            >
              {candidate.shortLabel}
            </button>
          ))}
        </div>
      </header>

      <div className="library-grid">
        {workflows.length === 0 && (
          <div className="library-no-results" role="status">
            No starter matches “{searchQuery}”. Try a product name such as
            Calendar, Sheets, Drive, or Gmail.
          </div>
        )}
        {workflows.map((workflow) => (
          <button
            key={workflow.id}
            type="button"
            className="library-card"
            onClick={() => onPick(workflow)}
            aria-label={`Preview ${workflow.name}`}
          >
            <span className="library-card-icon" aria-hidden>
              <StarterIcon category={workflow.category} />
            </span>
            <span className="library-card-main">
              <span className="library-card-topline">
                <span className="library-card-name">{workflow.name}</span>
                {workflow.featured && (
                  <span className="library-featured">Featured</span>
                )}
              </span>
              <span className="library-card-description">
                {workflow.description}
              </span>
              <span className="library-card-meta">
                <span
                  className={`library-effect is-${workflow.effect.replace("_", "-")}`}
                >
                  {starterEffectLabel(workflow.effect)}
                </span>
                {workflow.google_services && (
                  <span>{workflow.google_services.join(" + ")}</span>
                )}
                {workflow.min_tool_version && (
                  <span>gws ≥ {workflow.min_tool_version}</span>
                )}
                {!workflow.google_services && workflow.tools.length > 0 && (
                  <span>{workflow.tools.join(" + ")}</span>
                )}
                {workflow.platforms && (
                  <span>{workflow.platforms.join(" / ")}</span>
                )}
                {workflow.tools.length === 0 && <span>No credentials</span>}
                {workflow.requires_llm && <span>AI step</span>}
              </span>
            </span>
            <span className="library-card-arrow" aria-hidden>
              →
            </span>
          </button>
        ))}
      </div>

      <aside className="library-trust-note">
        <span aria-hidden>◇</span>
        <span>
          Starters are bundled with Cori or pinned to a versioned git source.
          You can inspect their steps, permissions, and parameters before the
          first run.
        </span>
      </aside>
    </section>
  );
}

function EmptyHistory({ onOpenLibrary }: { onOpenLibrary: () => void }) {
  return (
    <div className="empty-history">
      <div className="empty-history-icon" aria-hidden>
        <WorkflowIcon />
      </div>
      <div className="empty-history-title">No runs yet</div>
      <p>Choose a starter, open a local folder, or paste a git ref above.</p>
      <button type="button" className="welcome-link" onClick={onOpenLibrary}>
        Open starter library →
      </button>
      <button
        type="button"
        className="welcome-link is-muted"
        onClick={() => {
          void openUrl("https://docs.cori.do/getting-started/capture-from-agent");
        }}
      >
        Create with an agent →
      </button>
    </div>
  );
}

function describeRecentSource(r: RecentWorkflow): string {
  const s = r.source;
  if (!s) return r.key;
  if (s.kind === "local") return s.path;
  if (s.kind === "remote") {
    const tail = s.subpath ? `${s.repo}/${s.subpath}` : s.repo;
    return s.ref ? `${s.host}/${tail}@${s.ref}` : `${s.host}/${tail}`;
  }
  return r.key;
}

/**
 * Rank this user's workflows by valid local run history. Path migrations can
 * leave more than one history directory for the same source, so merge their
 * counts and retain the newest metadata before sorting by use, then recency.
 */
function rankFrequentlyRunWorkflows(
  recents: RecentWorkflow[],
): RecentWorkflow[] {
  const merged = new Map<string, RecentWorkflow>();

  for (const recent of recents) {
    // The path/ref is the workflow identity. Its display name can legitimately
    // change between versions without splitting one user's frequency history.
    const identity = recentSourcePath(recent);
    const existing = merged.get(identity);
    if (!existing) {
      merged.set(identity, { ...recent });
      continue;
    }

    const recentIsNewer =
      Date.parse(recent.last_run_at) > Date.parse(existing.last_run_at);
    const newest = recentIsNewer ? recent : existing;
    merged.set(identity, {
      ...newest,
      run_count: existing.run_count + recent.run_count,
    });
  }

  return [...merged.values()].sort((a, b) => {
    const byCount = b.run_count - a.run_count;
    if (byCount !== 0) return byCount;
    return Date.parse(b.last_run_at) - Date.parse(a.last_run_at);
  });
}

function recentSourcePath(recent: RecentWorkflow): string {
  const source = recent.source;
  if (!source) return recent.key;
  if (source.kind === "local") return source.path;
  return source.subpath
    ? `${source.host}/${source.repo}/${source.subpath}`
    : `${source.host}/${source.repo}`;
}

function formatErr(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

// ─── Self-update banner ──────────────────────────────────────────────────

/**
 * Shown when the updater announces a newer signed build. Install is
 * human-initiated — clicking downloads, verifies, installs, restarts.
 */
function UpdateBanner() {
  const [version, setVersion] = useState<string | null>(null);
  const [phase, setPhase] = useState<"idle" | "busy" | "error">("idle");

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    onUpdaterAvailable((v) => !cancelled && setVersion(v))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (!version) return null;
  return (
    <div className="first-run-cli" role="region" aria-label="Update available">
      <span className="first-run-cli-text">
        Cori {version} is available.
        {phase === "error" && (
          <span style={{ color: "var(--red)" }}> Install failed — try again.</span>
        )}
      </span>
      <button
        type="button"
        className="btn primary"
        disabled={phase === "busy"}
        onClick={() => {
          setPhase("busy");
          installUpdate().catch(() => setPhase("error"));
        }}
      >
        {phase === "busy" ? "Installing…" : "Install & restart"}
      </button>
    </div>
  );
}

// ─── First-run CLI install prompt ────────────────────────────────────────

const CLI_PROMPT_DISMISSED_KEY = "cori.first-run-cli-prompt-dismissed";

/**
 * On first launch, offer to put `cori` on PATH — so plugin/agent users
 * never have to find the footer button (or a terminal). Shows only when
 * the bundle ships the CLI and it isn't installed yet; one dismissal is
 * remembered forever. The footer's "Install CLI" button remains as the
 * quiet, always-available path.
 */
function FirstRunCliPrompt() {
  const [state, setState] = useState<
    | { kind: "hidden" }
    | { kind: "offer" }
    | { kind: "busy" }
    | { kind: "done"; path: string; onPath: boolean }
    | { kind: "error"; message: string }
  >({ kind: "hidden" });

  useEffect(() => {
    let cancelled = false;
    if (localStorage.getItem(CLI_PROMPT_DISMISSED_KEY)) return;
    getCliInstallStatus()
      .then((s) => {
        if (!cancelled && s.bundled && !s.installed_path) {
          setState({ kind: "offer" });
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  if (state.kind === "hidden") return null;

  const dismiss = () => {
    localStorage.setItem(CLI_PROMPT_DISMISSED_KEY, "1");
    setState({ kind: "hidden" });
  };

  return (
    <div className="first-run-cli" role="region" aria-label="Install the cori command">
      {state.kind === "done" ? (
        <>
          <span className="first-run-cli-text">
            ✓ <code>cori</code> installed
            {state.onPath ? "" : " (add its directory to your PATH to use it in terminals)"}
            {" — agents and terminals can now use Cori."}
          </span>
          <button type="button" className="btn" onClick={dismiss}>
            Done
          </button>
        </>
      ) : (
        <>
          <span className="first-run-cli-text">
            Put the <code>cori</code> command on your PATH? Agents (Claude, Cursor, …)
            and terminals need it to check and run workflows.
            {state.kind === "error" && (
              <span style={{ color: "var(--red)" }}> Install failed: {state.message}</span>
            )}
          </span>
          <button
            type="button"
            className="btn primary"
            disabled={state.kind === "busy"}
            onClick={() => {
              setState({ kind: "busy" });
              installCli()
                .then((r) => setState({ kind: "done", path: r.path, onPath: r.on_path }))
                .catch((e) => setState({ kind: "error", message: formatErr(e) }));
            }}
          >
            {state.kind === "busy" ? "Installing…" : "Install CLI"}
          </button>
          <button type="button" className="btn" disabled={state.kind === "busy"} onClick={dismiss}>
            Not now
          </button>
        </>
      )}
    </div>
  );
}

// ─── CLI install ──────────────────────────────────────────────────────────

/**
 * "Install CLI" footer action. Rendered only when this bundle ships
 * the CLI sidecar AND `cori` isn't already on PATH — users who
 * installed via install.sh (or already clicked this) never see it.
 */
function CliInstallAction() {
  const [visible, setVisible] = useState(false);
  const [phase, setPhase] = useState<
    | { kind: "idle" }
    | { kind: "busy" }
    | { kind: "done"; path: string; onPath: boolean }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

  useEffect(() => {
    let cancelled = false;
    getCliInstallStatus()
      .then((s) => {
        if (!cancelled) setVisible(s.bundled && !s.installed_path);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  if (!visible) return null;

  if (phase.kind === "done") {
    return (
      <span
        className="launcher-foot-note"
        title={
          phase.onPath
            ? `Installed at ${phase.path}`
            : `Installed at ${phase.path} — add its directory to your PATH to use it`
        }
      >
        ✓ cori installed{phase.onPath ? "" : " (not on PATH)"}
      </span>
    );
  }

  return (
    <button
      type="button"
      className="btn"
      disabled={phase.kind === "busy"}
      onClick={() => {
        setPhase({ kind: "busy" });
        installCli()
          .then((r) =>
            setPhase({ kind: "done", path: r.path, onPath: r.on_path }),
          )
          .catch((e) => setPhase({ kind: "error", message: formatErr(e) }));
      }}
      title={
        phase.kind === "error"
          ? `Install failed: ${phase.message} — click to retry`
          : "Put the `cori` command on your PATH so terminals and agents can use it"
      }
    >
      {phase.kind === "busy"
        ? "Installing…"
        : phase.kind === "error"
          ? "Install CLI ✗"
          : "Install CLI"}
    </button>
  );
}

// ─── Footer + icons ───────────────────────────────────────────────────────

/**
 * Engine state, top-right of the chrome bar — a dot and one word, the
 * same place the site's launcher says "engine ready". Which identity and
 * which endpoint that engine is are facts, not state, so they live in the
 * footer strip instead.
 */
function EngineBadge({
  stack,
  status,
}: {
  stack: StackStatus | undefined;
  status: StatusResponse | null;
}) {
  const state = stack?.state ?? (status?.reachable ? "up" : "starting");
  const dot =
    state === "up" ? "dot ok" : state === "down" ? "dot bad" : "dot warn";
  const label =
    state === "up"
      ? "engine ready"
      : state === "down"
        ? "engine offline"
        : state === "degraded"
          ? "engine degraded"
          : "engine starting…";
  const reason =
    stack && (stack.state === "degraded" || stack.state === "down")
      ? stack.reason
      : undefined;
  return (
    <span className="engine-badge" title={reason ?? label}>
      <span className={dot} />
      {label}
    </span>
  );
}

/**
 * The footer's mono strip: which queue this machine dispatches to, and
 * which Temporal it talks to. Both are the answer to "where am I", which
 * is what the site's `~/cori/workflows · main · 2 ahead` answers there.
 */
function MachineFacts({ status }: { status: StatusResponse | null }) {
  if (!status) return null;
  const identity = identityLabel(status);
  return (
    <div className="launcher-foot-facts">
      <span title={`Task queue: ${status.task_queue}`}>
        {identity ?? status.task_queue}
      </span>
      <span className="sep" aria-hidden>
        ·
      </span>
      <span title={`Temporal endpoint: ${status.endpoint}`}>
        {status.endpoint}
      </span>
    </div>
  );
}

function identityLabel(s: StatusResponse | null): string | null {
  if (!s) return null;
  if (s.identity.kind === "person") return s.identity.user_id;
  if (s.identity.kind === "service") return `service:${s.identity.pool}`;
  return null;
}

/**
 * Two different things can be true of a row, so they get two marks:
 * `is-selected` is where the keyboard is, `is-open` is what the right
 * pane is showing. Usually the same row — but arrowing down the list
 * while a run is under way is exactly when they differ, and that is
 * exactly when the difference matters.
 */
function rowClass(selected: boolean, open: boolean): string {
  return `result-row${selected ? " is-selected" : ""}${open ? " is-open" : ""}`;
}

function WorkflowIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M7 5.5v13l11-6.5z" />
    </svg>
  );
}

function StarterIcon({
  category,
}: {
  category: StarterWorkflow["category"];
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {category === "local" && (
        <>
          <rect x="3" y="4" width="18" height="13" rx="2" />
          <path d="M8 21h8M12 17v4" />
        </>
      )}
      {category === "google_workspace" && (
        <>
          <rect x="3" y="3" width="7" height="7" rx="1.5" />
          <rect x="14" y="3" width="7" height="7" rx="1.5" />
          <rect x="3" y="14" width="7" height="7" rx="1.5" />
          <rect x="14" y="14" width="7" height="7" rx="1.5" />
        </>
      )}
      {category === "developer" && (
        <>
          <circle cx="6" cy="5" r="2" />
          <circle cx="18" cy="8" r="2" />
          <circle cx="8" cy="19" r="2" />
          <path d="M6 7v4a6 6 0 0 0 6 6h4M8 17V9a3 3 0 0 1 3-3h5" />
        </>
      )}
      {category === "essentials" && (
        <>
          <path d="m12 3 1.5 4.2L18 9l-4.5 1.8L12 15l-1.5-4.2L6 9l4.5-1.8z" />
          <path d="m18.5 15 .7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7zM5 15v5h7" />
        </>
      )}
    </svg>
  );
}

function RefreshIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="13"
      height="13"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M4 12a8 8 0 0 1 13.7-5.6L21 9" />
      <path d="M21 4v5h-5" />
      <path d="M20 12a8 8 0 0 1-13.7 5.6L3 15" />
      <path d="M3 20v-5h5" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M3 7.5a1.5 1.5 0 0 1 1.5-1.5h4l2 2H19.5A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19H4.5A1.5 1.5 0 0 1 3 17.5v-10z" />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M6 3.5h8l4 4v13a1.5 1.5 0 0 1-1.5 1.5h-10.5A1.5 1.5 0 0 1 4.5 20.5v-15A1.5 1.5 0 0 1 6 4z" />
      <path d="M14 3.5v4h4" />
    </svg>
  );
}

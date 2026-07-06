import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRevalidator } from "react-router";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MiddleTruncate } from "../components/middle-truncate";
import { ThemeIconButton } from "../components/theme-icon-button";
import {
  getLastLocalDir,
  getStackStatus,
  getStatus,
  isIpcError,
  listDir,
  listRecentWorkflows,
  listRemoteWorkflows,
  onStackStatus,
  peekSource,
  sourceToCli,
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
import { formatRelative } from "../lib/format";
import { openLaunch, openManage } from "../lib/windows";

export function meta() {
  return [{ title: "Cori" }];
}

interface LauncherData {
  status: StatusResponse | null;
  recents: RecentWorkflow[];
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

/**
 * Unified items model for the results pane. Each context yields a
 * sequence of `ListedItem`s; the selection / keyboard nav code below
 * doesn't care which kind it's looking at.
 */
type ListedItem =
  | { kind: "recent"; recent: RecentWorkflow; key: string }
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
  const [ctx, setCtx] = useState<LauncherContext>({ kind: "recents" });
  const [dragOver, setDragOver] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
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

  // Tray menu's History / Schedules / Workers items emit this with a
  // `tab` payload — route through the launcher so window creation
  // stays a UI concern. The launcher itself isn't surfaced; the user
  // asked for a specific tab, not the launcher.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ tab?: "runs" | "schedules" | "workers" }>(
      "tray:open-manage",
      (e) => {
        void openManage(e.payload?.tab);
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

  // Recents are derived from persisted run traces on disk, so the
  // loader's snapshot goes stale the moment a run completes in another
  // window (or from the CLI). Re-run the loader whenever the launcher
  // regains focus — that's when the user is about to look at the list.
  useEffect(() => {
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

  // Drop-a-folder support. Workflow folder → openLaunch directly;
  // plain folder → enter the local-browse context. Multi-path drops:
  // pick the first path that classifies as a local directory.
  useEffect(() => {
    const w = getCurrentWebviewWindow();
    let unlistenFn: UnlistenFn | undefined;
    let cancelled = false;

    const handleDrop = async (paths: string[]) => {
      for (const path of paths) {
        try {
          const peek = await peekSource(path);
          if (peek.kind === "local" && peek.local_exists) {
            if (peek.is_workflow_dir) {
              void openLaunch(peek.normalized);
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
      return fuzzyFilter(recents, input.trim(), (r) => [
        r.name ?? "",
        r.workflow_id,
        describeRecentSource(r),
      ]).map((r, i) => ({
        kind: "recent",
        recent: r,
        key: `${r.key}-${i}`,
      }));
    }
    if (ctx.kind === "local") {
      const entries = ctx.listing?.entries ?? [];
      return fuzzyFilter(entries, input.trim(), (e) => e.name).map((e) => ({
        kind: "dir-entry",
        entry: e,
        key: e.path,
      }));
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

  function activateItem(item: ListedItem) {
    if (item.kind === "recent") {
      const src = sourceToCli(item.recent.source);
      if (src) void openLaunch(src);
      return;
    }
    if (item.kind === "dir-entry") {
      const e = item.entry;
      if (e.kind === "workflow") {
        void openLaunch(e.path);
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
      void openLaunch(buildRemoteSource(item.listing, item.entry));
      return;
    }
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
          // Phase 3: a subpath-bearing ref names a specific workflow —
          // open the launch screen directly; the launch window's
          // `resolve_workflow` will surface consent or capability gaps.
          void openLaunch(peek.normalized);
          return;
        }
        // Phase 4: bare `host/owner/repo[@ref]` — list workflows in
        // the repo so the user can pick a subpath.
        enterRemoteContext(peek.normalized);
        return;
      }
    }

    const item = items[selIndex];
    if (item) activateItem(item);
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
          popContext();
        }
        break;
    }
  }

  return (
    <div className={`launcher${dragOver ? " is-drag-over" : ""}`}>
      <header className="launcher-head" data-tauri-drag-region>
        <img
          src="/cori-mark.png"
          alt=""
          className="launcher-mark"
          width={22}
          height={22}
        />
        <div className="launcher-title">Cori</div>
        <div className="launcher-head-spacer" />
        <ThemeIconButton />
      </header>

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

      <Breadcrumb
        context={ctx}
        onPop={popContext}
        onRefresh={
          ctx.kind === "remote"
            ? () => enterRemoteContext(ctx.refStr, true)
            : undefined
        }
      />

      <ResultsPane
        ctx={ctx}
        items={items}
        selectedIndex={selIndex}
        onSelect={(i) => {
          setSelIndex(i);
          activateItem(items[i]);
        }}
        onHover={setSelIndex}
        inputIsEmpty={input.length === 0}
        recentsCount={recents.length}
      />

      <footer className="launcher-foot">
        <StackBadge stack={stack} status={status} />
        <div className="launcher-foot-actions">
          <button
            type="button"
            className="btn"
            onClick={() => void openManage("runs")}
            title="Past runs"
          >
            History
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void openManage("schedules")}
            title="Cron-driven runs"
          >
            Schedules
          </button>
        </div>
      </footer>
    </div>
  );
}

function placeholderFor(ctx: LauncherContext): string {
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
        local folder{!peek.local_exists && " ✗"}
      </span>
    );
  }
  return (
    <span className="search-bar-chip remote" title={peek.normalized}>
      <span style={{ marginRight: 6 }}>remote</span>
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
        <span className="crumb-label">Recents</span>
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
          title="Back to recents (Esc)"
          aria-label="Back to recents"
        >
          ←
        </button>
        <span className="crumb-label">Local</span>
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
          title="Back to recents (Esc)"
          aria-label="Back to recents"
        >
          ←
        </button>
        <span className="crumb-label">Remote</span>
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
  onSelect: (index: number) => void;
  onHover: (index: number) => void;
  inputIsEmpty: boolean;
  recentsCount: number;
}

function ResultsPane({
  ctx,
  items,
  selectedIndex,
  onSelect,
  onHover,
  inputIsEmpty,
  recentsCount,
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

  if (ctx.kind === "recents" && items.length === 0) {
    if (!inputIsEmpty) {
      return (
        <div className="results">
          <div className="results-empty">
            No recent workflow matches that filter.
          </div>
        </div>
      );
    }
    if (recentsCount === 0) {
      return (
        <div className="results">
          <Welcome />
        </div>
      );
    }
    return (
      <div className="results">
        <div className="results-empty">No recents to show.</div>
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
  onClick: () => void;
  onHover: () => void;
  buttonRef: (el: HTMLButtonElement | null) => void;
}

function ItemRow(props: ItemRowProps) {
  if (props.item.kind === "recent") {
    return <RecentRow {...props} item={props.item} />;
  }
  if (props.item.kind === "dir-entry") {
    return <DirEntryRow {...props} item={props.item} />;
  }
  return <RemoteEntryRow {...props} item={props.item} />;
}

function RecentRow({
  item,
  selected,
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
      className={`result-row${selected ? " is-selected" : ""}`}
      onClick={onClick}
      onMouseEnter={onHover}
      disabled={disabled}
      role="option"
      aria-selected={selected}
      title={
        disabled
          ? "Older run — no recoverable source on disk"
          : `${displayName} — open launch screen`
      }
    >
      <span className="result-row-icon" aria-hidden>
        <WorkflowIcon />
      </span>
      <div className="result-row-body">
        <div className="result-row-name">{displayName}</div>
        <div className="result-row-meta">
          <span className={`pill ${pillFor(r.last_status)}`}>
            {r.last_status}
          </span>
          <span>{formatRelative(r.last_run_at)}</span>
          {sourceLabel && (
            <MiddleTruncate
              text={sourceLabel}
              tail={Math.min(22, Math.floor(sourceLabel.length / 2))}
            />
          )}
        </div>
      </div>
    </button>
  );
}

function DirEntryRow({
  item,
  selected,
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
      ? "workflow · open launch screen"
      : e.kind === "dir"
        ? "folder · open"
        : e.symlink
          ? "symlink (not followed)"
          : "file";
  return (
    <button
      type="button"
      ref={buttonRef}
      className={`result-row${selected ? " is-selected" : ""}`}
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
      className={`result-row${selected ? " is-selected" : ""}`}
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
          />
        </div>
        {e.description && (
          <div className="result-row-desc">{e.description}</div>
        )}
      </div>
    </button>
  );
}

function Welcome() {
  return (
    <div className="welcome">
      <h2 className="welcome-title">Welcome to Cori</h2>

      <section className="welcome-section">
        <h3 className="welcome-subtitle">Run an existing workflow</h3>
        <p className="welcome-body">
          Open one you already have on your machine — click the folder
          icon at the right of the search bar, or drag a folder onto
          this window. To run a workflow from a git repository, paste
          a reference like <code>github.com/cori-do/workflows</code> and press
          Enter.
        </p>
      </section>

      <section className="welcome-section">
        <h3 className="welcome-subtitle">Create your first workflow</h3>
        <p className="welcome-body">
          Install the <code>cori-save-workflow</code> agent skill, then
          ask your AI assistant to save a workflow with you.
        </p>
        <button
          type="button"
          className="welcome-link"
          onClick={() => {
            void openUrl("https://docs.cori.do/getting-started/capture-from-agent");
          }}
        >
          Learn more →
        </button>
      </section>
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

function formatErr(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

// ─── Footer + icons ───────────────────────────────────────────────────────

function StackBadge({
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
      ? "Ready"
      : state === "down"
        ? "Offline"
        : state === "degraded"
          ? "Degraded"
          : "Starting…";
  const identity = identityLabel(status);
  const reason =
    stack && (stack.state === "degraded" || stack.state === "down")
      ? stack.reason
      : undefined;
  return (
    <div className="launcher-status" title={reason}>
      <span className={dot} />
      <span className="launcher-status-text">
        <span className="launcher-status-state">{label}</span>
        {identity && (
          <span className="launcher-status-meta">{identity}</span>
        )}
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

function pillFor(status: string): string {
  if (status === "succeeded") return "ok";
  if (status === "failed") return "bad";
  return "muted";
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

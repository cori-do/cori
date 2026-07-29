// Window-spawning helpers. Each helper computes a deterministic label,
// reuses an existing window if one is open (focus-don't-duplicate), and
// otherwise creates a new WebviewWindow at the right route.
//
// Window kinds:
//   • launcher   — persistent, tray-toggled; created by tauri.conf.json.
//                  Picking a workflow and running it happen inside it.
//   • run-<id>   — disposable, one per run. Only for runs the launcher
//                  did not start itself: history, and runs an agent or
//                  a schedule kicked off elsewhere.
//   • manage     — single, tabbed

import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { emit } from "@tauri-apps/api/event";

export type ManageTab = "workers" | "schedules" | "runs" | "approvals";

const RUN_SIZE = { width: 820, height: 720 } as const;
const MANAGE_SIZE = { width: 900, height: 700 } as const;

/**
 * Open (or focus) a run window. Live by default; pass `{ key, utc }`
 * to open the historical trace view for that run instead.
 */
export async function openRun(
  runId: string,
  opts?: { key?: string; utc?: string },
): Promise<void> {
  const label = `run-${runId}`;
  if (await focusExisting(label)) return;
  const url =
    opts?.key && opts?.utc
      ? `/runs/${encodeURIComponent(opts.key)}/${encodeURIComponent(opts.utc)}`
      : `/runs/live/${encodeURIComponent(runId)}`;
  new WebviewWindow(label, {
    url,
    title: "Live run — Cori",
    ...RUN_SIZE,
    minWidth: 640,
    minHeight: 480,
    resizable: true,
  });
}

/**
 * Open (or focus) the single manage window, optionally landing on a
 * specific tab. Defaults to the runs history tab.
 */
export async function openManage(tab: ManageTab = "runs"): Promise<void> {
  const label = "manage";
  const url = `/manage/${tab}`;
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.show();
    await existing.setFocus();
    // The user explicitly asked for a tab (footer button, tray menu) —
    // flip to it even if the window was already showing a different one.
    // The manage window's effect-handler picks this up and navigates.
    await emit("manage:set-tab", { tab });
    return;
  }
  new WebviewWindow(label, {
    url,
    title: "Manage — Cori",
    ...MANAGE_SIZE,
    minWidth: 720,
    minHeight: 520,
    resizable: true,
  });
}

async function focusExisting(label: string): Promise<boolean> {
  const w = await WebviewWindow.getByLabel(label);
  if (!w) return false;
  await w.show();
  await w.setFocus();
  return true;
}


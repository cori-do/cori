import { NavLink, Outlet, useLocation, useNavigate } from "react-router";
import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export function meta() {
  return [{ title: "Manage — Cori" }];
}

// Labels are user-facing; URL slugs are stable routing keys. "History"
// is the more specific name for the runs tab — see the launcher footer
// and tray menu, which call the same tab History.
const TABS: Array<{ to: string; label: string }> = [
  { to: "/manage/approvals", label: "Inbox" },
  { to: "/manage/runs", label: "History" },
  { to: "/manage/schedules", label: "Schedules" },
  { to: "/manage/capabilities", label: "Capabilities" },
  { to: "/manage/workers", label: "Workers" },
];

const VALID_TABS = new Set([
  "runs",
  "schedules",
  "capabilities",
  "workers",
  "approvals",
]);

export default function Manage() {
  const navigate = useNavigate();
  const { pathname } = useLocation();

  // Bare /manage → default to History (the most useful tab in practice).
  useEffect(() => {
    if (pathname === "/manage" || pathname === "/manage/") {
      navigate("/manage/runs", { replace: true });
    }
  }, [pathname, navigate]);

  // `openManage(tab)` emits this when the manage window was already
  // open — flip to the requested tab instead of leaving the user on
  // whichever one they last touched.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ tab: string }>("manage:set-tab", (e) => {
      const tab = e.payload?.tab;
      if (tab && VALID_TABS.has(tab)) {
        navigate(`/manage/${tab}`);
      }
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
  }, [navigate]);

  return (
    <div className="manage">
      <nav className="manage-tabs" aria-label="Manage sections">
        {TABS.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            className={({ isActive }) =>
              "manage-tab" + (isActive ? " is-active" : "")
            }
          >
            {t.label}
          </NavLink>
        ))}
      </nav>
      <main className="manage-body">
        <Outlet />
      </main>
    </div>
  );
}

import { NavLink, Outlet, useLocation, useNavigate } from "react-router";
import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export function meta() {
  return [{ title: "Settings — Cori" }];
}

// Launcher sections (Workflows, Inbox, Schedules) stay in the launcher.
// This separate window contains machine and worker settings only.
const TABS: Array<{ to: string; label: string }> = [
  { to: "/settings/workers", label: "Workers" },
  { to: "/settings/runs", label: "History" },
  { to: "/settings/capabilities", label: "Capabilities" },
  { to: "/settings/providers", label: "AI Providers" },
];

const VALID_TABS = new Set(["runs", "capabilities", "providers", "workers"]);

export default function Manage() {
  const navigate = useNavigate();
  const { pathname } = useLocation();

  // Bare /settings → the worker this machine contributes.
  useEffect(() => {
    if (pathname === "/settings" || pathname === "/settings/") {
      navigate("/settings/workers", { replace: true });
    }
  }, [pathname, navigate]);

  // `openSettings(tab)` emits this when the settings window was already
  // open — flip to the requested tab instead of leaving the user on
  // whichever one they last touched.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ tab: string }>("settings:set-tab", (e) => {
      const tab = e.payload?.tab;
      if (tab && VALID_TABS.has(tab)) {
        navigate(`/settings/${tab}`);
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
      <nav className="manage-tabs" aria-label="Settings sections">
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

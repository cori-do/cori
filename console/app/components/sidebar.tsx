import { useEffect, useState } from "react";
import { NavLink } from "react-router";
import { ThemeToggle } from "./theme-toggle";
import {
  getStackStatus,
  getStatus,
  onStackStatus,
  type StackStatus,
  type StatusResponse,
} from "../lib/api";

interface NavItem {
  to: string;
  label: string;
  icon: React.ReactNode;
  end?: boolean;
}

const NAV: NavItem[] = [
  { to: "/", label: "Dashboard", end: true, icon: <IconDashboard /> },
  { to: "/run", label: "Run", icon: <IconPlay /> },
  { to: "/runs", label: "History", icon: <IconHistory /> },
  { to: "/workers", label: "Workers", icon: <IconWorkers /> },
  { to: "/schedules", label: "Schedules", icon: <IconSchedule /> },
];

export function Sidebar() {
  const [stack, setStack] = useState<StackStatus | undefined>(undefined);
  const [status, setStatus] = useState<StatusResponse | undefined>(undefined);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    getStackStatus()
      .then((s) => !cancelled && setStack(s))
      .catch(() => {});
    getStatus()
      .then((s) => !cancelled && setStatus(s))
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

  const state = stack?.state ?? (status?.reachable ? "up" : "starting");
  const dot =
    state === "up"
      ? "dot ok"
      : state === "down"
        ? "dot bad"
        : "dot warn";
  const stateLabel =
    state === "up"
      ? "Connected"
      : state === "down"
        ? "Offline"
        : state === "degraded"
          ? "Degraded"
          : "Starting…";

  const identity = identityLabel(status);

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <NavLink to="/" aria-label="Cori — home" className="brand">
          <img
            src="/cori-mark.png"
            alt=""
            className="brand-mark"
            width={28}
            height={28}
          />
          <div className="brand-text">
            <span className="brand-name">Cori</span>
            <span className="brand-tag">Console</span>
          </div>
        </NavLink>
      </div>

      <nav className="sidebar-nav" aria-label="Primary">
        {NAV.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.end}
            className={({ isActive }) =>
              "nav-item" + (isActive ? " is-active" : "")
            }
          >
            <span className="nav-icon" aria-hidden>
              {item.icon}
            </span>
            <span className="nav-label">{item.label}</span>
          </NavLink>
        ))}
      </nav>

      <div className="sidebar-footer">
        <div className="status-pill" title={stackReason(stack)}>
          <span className={dot} />
          <span className="status-text">
            <span className="status-state">{stateLabel}</span>
            {identity && <span className="status-meta">{identity}</span>}
          </span>
        </div>
        <ThemeToggle />
      </div>
    </aside>
  );
}

function identityLabel(s: StatusResponse | undefined): string | null {
  if (!s) return null;
  if (s.identity.kind === "person") return s.identity.user_id;
  if (s.identity.kind === "service") return `service:${s.identity.pool}`;
  return null;
}

function stackReason(s: StackStatus | undefined): string | undefined {
  if (!s) return undefined;
  if (s.state === "degraded" || s.state === "down") return s.reason;
  return undefined;
}

// ── Minimal inline icons (no extra dep). Stroke-based, 1.5px, 18×18. ──

function IconDashboard() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3.5" y="3.5" width="7" height="9" rx="1.4" />
      <rect x="13.5" y="3.5" width="7" height="5" rx="1.4" />
      <rect x="3.5" y="15.5" width="7" height="5" rx="1.4" />
      <rect x="13.5" y="11.5" width="7" height="9" rx="1.4" />
    </svg>
  );
}

function IconPlay() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="M7 5.5v13l11-6.5z" />
    </svg>
  );
}

function IconHistory() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1" />
      <path d="M3.5 4.5v4h4" />
      <path d="M12 7.5V12l3 2" />
    </svg>
  );
}

function IconWorkers() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3.5" y="4.5" width="17" height="6" rx="1.4" />
      <rect x="3.5" y="13.5" width="17" height="6" rx="1.4" />
      <circle cx="7" cy="7.5" r="0.8" fill="currentColor" stroke="none" />
      <circle cx="7" cy="16.5" r="0.8" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconSchedule() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3.5" y="5" width="17" height="15" rx="1.6" />
      <path d="M3.5 9.5h17" />
      <path d="M8 3v4" />
      <path d="M16 3v4" />
    </svg>
  );
}

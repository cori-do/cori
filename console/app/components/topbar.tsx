import { useLocation } from "react-router";

interface PageInfo {
  title: string;
  subtitle?: string;
}

function pageFor(pathname: string): PageInfo {
  if (pathname === "/" || pathname === "") {
    return { title: "Dashboard", subtitle: "Live workspace status" };
  }
  if (pathname.startsWith("/runs/live/")) {
    return { title: "Live run", subtitle: "Streaming events" };
  }
  if (pathname.startsWith("/runs/")) {
    return { title: "Run trace", subtitle: "Historical run detail" };
  }
  if (pathname === "/runs") {
    return { title: "History", subtitle: "Past workflow runs" };
  }
  if (pathname === "/run") {
    return { title: "Run a workflow", subtitle: "Inspect, configure, launch" };
  }
  if (pathname === "/workers") {
    return { title: "Workers", subtitle: "Cluster capability reports" };
  }
  if (pathname === "/schedules") {
    return { title: "Schedules", subtitle: "Cron-driven runs" };
  }
  return { title: "Cori" };
}

export function Topbar() {
  const { pathname } = useLocation();
  const info = pageFor(pathname);
  return (
    <header className="topbar" data-tauri-drag-region>
      <div className="topbar-titles" data-tauri-drag-region>
        <h1 className="topbar-title" data-tauri-drag-region>
          {info.title}
        </h1>
        {info.subtitle && (
          <p className="topbar-subtitle" data-tauri-drag-region>
            {info.subtitle}
          </p>
        )}
      </div>
    </header>
  );
}

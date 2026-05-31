import {
  isRouteErrorResponse,
  Links,
  Meta,
  Outlet,
  Scripts,
  ScrollRestoration,
  Link,
  useRouteError,
} from "react-router";

import { ThemeToggle } from "./components/theme-toggle";
import { ensureSession } from "./lib/session";
import "./styles/base.css";

// Runs synchronously in <head> before paint to avoid a light flash on
// dark systems. Mirrors the standard "ThemeProvider" trick.
const THEME_BOOT = `(function(){try{var k='cori-theme';var s=localStorage.getItem(k);var d=s==='dark'||(!s&&matchMedia('(prefers-color-scheme: dark)').matches);if(d)document.documentElement.classList.add('dark');}catch(e){}})();`;

export const links = () => [
  { rel: "icon", type: "image/png", href: "/cori-mark.png" },
  { rel: "apple-touch-icon", href: "/cori-mark.png" },
];

export const meta = () => [{ title: "Cori Console" }];

// Run once on initial app load: exchange the URL `?t=` token for a
// session cookie. Child route loaders await the same promise so they
// don't race the bootstrap.
export async function clientLoader() {
  await ensureSession();
  return null;
}

export function HydrateFallback() {
  return (
    <div className="boot">
      <p>Loading Cori Console…</p>
    </div>
  );
}

export function Layout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <Meta />
        <Links />
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOT }} />
      </head>
      <body>
        <header className="topbar">
          <Link to="/" className="brand" aria-label="Cori Console — home">
            <img
              src="/cori-logo.png"
              alt="Cori"
              className="brand-logo brand-logo-light"
              width={2600}
              height={1072}
            />
            <img
              src="/cori-logo-white.png"
              alt="Cori"
              className="brand-logo brand-logo-dark"
              width={2600}
              height={1072}
            />
            <span className="brand-tag">Console</span>
          </Link>
          <nav>
            <Link to="/">Dashboard</Link>
            <Link to="/run">Run</Link>
            <Link to="/runs">Runs</Link>
            <Link to="/workers">Workers</Link>
            <Link to="/schedules">Schedules</Link>
          </nav>
          <ThemeToggle />
        </header>
        <main className="main">{children}</main>
        <ScrollRestoration />
        <Scripts />
      </body>
    </html>
  );
}

export default function App() {
  return <Outlet />;
}

export function ErrorBoundary() {
  const error = useRouteError();
  let title = "Something went wrong";
  let detail = "Unknown error";

  if (isRouteErrorResponse(error)) {
    title = `${error.status} ${error.statusText}`;
    detail =
      typeof error.data === "string"
        ? error.data
        : JSON.stringify(error.data);
    if (error.status === 401) {
      title = "Session required";
      detail =
        "Open the URL printed by `cori work` on startup — it includes a one-time token.";
    }
  } else if (error instanceof Error) {
    detail = error.message;
  }

  return (
    <div className="card error">
      <h1>{title}</h1>
      <pre>{detail}</pre>
    </div>
  );
}

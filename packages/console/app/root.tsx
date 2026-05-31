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

import { ensureSession } from "./lib/session";
import "./styles/base.css";

export const links = () => [
  {
    rel: "icon",
    href:
      "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Ctext y='14' font-size='14'%3E%E2%9A%99%EF%B8%8F%3C/text%3E%3C/svg%3E",
  },
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
      </head>
      <body>
        <header className="topbar">
          <Link to="/" className="brand">
            Cori Console
          </Link>
          <nav>
            <Link to="/">Dashboard</Link>
            <Link to="/runs">Runs</Link>
          </nav>
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

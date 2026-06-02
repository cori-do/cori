import {
  isRouteErrorResponse,
  Links,
  Meta,
  Outlet,
  Scripts,
  ScrollRestoration,
  useRouteError,
} from "react-router";

import "./styles/base.css";

// Runs synchronously in <head> before paint to avoid a light flash on
// dark systems. Mirrors the standard "ThemeProvider" trick.
const THEME_BOOT = `(function(){try{var k='cori-theme';var s=localStorage.getItem(k);var d=s==='dark'||(!s&&matchMedia('(prefers-color-scheme: dark)').matches);if(d)document.documentElement.classList.add('dark');}catch(e){}})();`;

export const links = () => [
  { rel: "icon", type: "image/png", href: "/cori-mark.png" },
  { rel: "apple-touch-icon", href: "/cori-mark.png" },
];

export const meta = () => [{ title: "Cori" }];

export function HydrateFallback() {
  return (
    <div className="boot">
      <p>Loading Cori…</p>
    </div>
  );
}

// No global sidebar/topbar: each window loads one route and renders
// its own full-bleed chrome. See §6.2 of the launcher implementation
// guide.
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
        {children}
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
  } else if (error instanceof Error) {
    detail = error.message;
  }

  return (
    <div className="card error" style={{ margin: 24 }}>
      <h1>{title}</h1>
      <pre>{detail}</pre>
    </div>
  );
}

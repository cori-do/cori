// Trade the URL `?t=<token>` for an HttpOnly session cookie set by
// the server. After exchange we strip `?t=` from the address bar so
// reload doesn't re-attempt (and so the token isn't visible).
//
// The master token is also kept in module-scope memory for Phase 3
// state-changing requests (`Authorization: Bearer …`). It is never
// stored in localStorage/sessionStorage — the page reload requires
// re-opening the URL from `cori work`.

let bootstrapPromise: Promise<void> | null = null;
let masterToken: string | null = null;

export function getMasterToken(): string | null {
  return masterToken;
}

export function ensureSession(): Promise<void> {
  if (bootstrapPromise) return bootstrapPromise;
  bootstrapPromise = (async () => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    const tokenFromUrl = url.searchParams.get("t");
    if (!tokenFromUrl) return; // rely on existing cookie

    const res = await fetch("/api/session", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ token: tokenFromUrl }),
      credentials: "include",
    });
    if (!res.ok) {
      throw new Response(
        "session exchange failed — open the URL printed by `cori work`",
        { status: 401 },
      );
    }

    masterToken = tokenFromUrl;
    url.searchParams.delete("t");
    window.history.replaceState({}, "", url.pathname + url.search + url.hash);
  })();
  return bootstrapPromise;
}

import { useEffect } from "react";
import { MOCK } from "./api";
import { brand } from "./brand";

/** The browser tab names the page: "Releases · Team Requests — Cori". */
export function useTitle(...parts: (string | undefined | null | false)[]) {
  const t = parts.filter(Boolean).join(" · ");
  useEffect(() => { document.title = `${MOCK ? "[demo] " : ""}${t ? `${t} — ` : ""}${brand.name}`; }, [t]);
}

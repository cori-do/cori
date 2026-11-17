import type { LlmStarter } from "./types";

/** A version hash as its 12-character short form. */
export const short = (h?: string | null) => (h || "").replace("sha256:", "").slice(0, 12);
/** A cell value as text (objects as JSON). */
export const fmt = (v: unknown) => (v === null || v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));
/** A SQL identifier, quoted. */
export const qi = (n: string) => '"' + String(n).replace(/"/g, '""') + '"';
/** A store target as its route segment: `live` or the version's short hash. */
export const tseg = (t: string) => (t === "live" ? "live" : short(t));
/** An amount in euros: cents, or four decimals below one cent. */
export const eur = (n?: number | string | null) => {
  if (n === null || n === undefined || n === "") return "";
  const x = Number(n);
  return `${x !== 0 && Math.abs(x) < 0.01 ? x.toFixed(4) : x.toFixed(2)} €`;
};
export const plural = (n: number, word: string, many = word + "s") => `${n.toLocaleString()} ${n === 1 ? word : many}`;

/** A size in decimal units, as the server counts storage: `850 B`, `12.4 KB`, `3 MB`, `1.5 GB`. */
export const sizeText = (n?: number | null) => {
  if (n === null || n === undefined) return "";
  const [v, unit] = n >= 1e12 ? [n / 1e12, "TB"] : n >= 1e9 ? [n / 1e9, "GB"] : n >= 1e6 ? [n / 1e6, "MB"] : n >= 1e3 ? [n / 1e3, "KB"] : [n, "B"];
  return `${+v.toFixed(1)} ${unit}`;
};

const date = (t?: string | null) => (t ? new Date(t) : null);
const valid = (d: Date | null): d is Date => !!d && !Number.isNaN(d.getTime());

/** "just now", "5 min ago", "3 h ago", "yesterday", "12 Oct", "12 Oct 2025"; "in 3 days" for the future. */
export function ago(t?: string | null): string {
  const d = date(t);
  if (!valid(d)) return "";
  const s = (Date.now() - d.getTime()) / 1000;
  const future = s < 0, a = Math.abs(s);
  const say = (n: number, unit: string) => (future ? `in ${n} ${unit}` : `${n} ${unit} ago`);
  if (a < 45) return future ? "in a moment" : "just now";
  if (a < 3600) return say(Math.round(a / 60), "min");
  if (a < 86400) return say(Math.round(a / 3600), "h");
  if (a < 2 * 86400 && !future) return "yesterday";
  if (a < 7 * 86400) return say(Math.round(a / 86400), "days");
  return day(t);
}

/** "12 Oct" this year, "12 Oct 2025" otherwise. */
export function day(t?: string | null): string {
  const d = date(t);
  if (!valid(d)) return "";
  const thisYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", ...(thisYear ? {} : { year: "numeric" }) });
}

/** "12 Oct, 14:05" (with the year when not this one). */
export function when(t?: string | null): string {
  const d = date(t);
  if (!valid(d)) return "";
  return `${day(t)}, ${d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`;
}

/** The full local date and time, for a tooltip. */
export const exact = (t?: string | null) => { const d = date(t); return valid(d) ? d.toLocaleString() : ""; };

/** A duration in ms as "820 ms", "4.2 s", "3 min 05 s", "2 h 10 min". */
export function duration(ms?: number | null): string {
  if (ms === null || ms === undefined || Number.isNaN(ms)) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
  const m = Math.floor(ms / 60_000);
  if (m < 60) return `${m} min ${String(Math.floor((ms % 60_000) / 1000)).padStart(2, "0")} s`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}
export const between = (from?: string | null, to?: string | null) => {
  const a = date(from), b = to ? date(to) : new Date();
  return valid(a) && valid(b) ? b.getTime() - a.getTime() : undefined;
};

/** A name as initials for an avatar: "Alice Moreau" → "AM", "pat@x.io" → "P". */
export function initials(s: string): string {
  const clean = s.replace(/@.*/, "").replace(/[._-]+/g, " ").trim();
  const words = clean.split(/\s+/).filter(Boolean);
  return ((words[0]?.[0] || "?") + (words.length > 1 ? words[words.length - 1][0] : "")).toUpperCase();
}

/** A machine name as words: `submit_request` → "Submit request", `refund_eur` → "Refund (€)". */
export const words = (id: string) => {
  if (/^id$/i.test(id)) return "ID";
  const s = id.replace(/[_-](eur|euros)$/i, " (€)").replace(/[_-](usd)$/i, " ($)").replace(/[_-]id$/i, " ID").replace(/[_-]+/g, " ").trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
};

/** A form value typed for a column; throws a message naming the column when it does not fit. */
export function coerce(v: string, c: { name: string; type?: string; nullable?: boolean }): unknown {
  const t = (c.type || "").toLowerCase();
  if (v === "" && c.nullable) return null;
  if (/int|serial/.test(t)) { const n = Number(v); if (v === "" || !Number.isInteger(n)) throw new Error(`${c.name} takes an integer`); return n; }
  if (/numeric|decimal|double|real|float/.test(t)) { const n = Number(v); if (v === "" || Number.isNaN(n)) throw new Error(`${c.name} takes a number`); return n; }
  if (/bool/.test(t)) return /^(true|t|1|yes)$/i.test(v);
  if (/json/.test(t)) { try { return JSON.parse(v); } catch { throw new Error(`${c.name} takes JSON`); } }
  return v;
}

/** The calls an organization has of the starter model, in words ("5 calls a minute and 50 a day"). */
export function starterCalls(s: Pick<LlmStarter, "calls_per_minute" | "calls_per_day">) {
  const minute = s.calls_per_minute ? `${s.calls_per_minute} calls a minute` : "";
  const dayN = s.calls_per_day ? `${s.calls_per_day}${minute ? "" : " calls"} a day` : "";
  return [minute, dayN].filter(Boolean).join(" and ") || "no limit on calls";
}

/** A cron expression in words when it is a common one; the expression otherwise. */
export function cronWords(cron: string, tz?: string): string {
  const p = cron.trim().split(/\s+/);
  const zone = tz && tz !== "UTC" ? ` (${tz})` : tz ? " UTC" : "";
  if (p.length !== 5) return cron;
  const [min, hour, dom, mon, dow] = p;
  const at = /^\d+$/.test(min) && /^\d+$/.test(hour) ? `${hour.padStart(2, "0")}:${min.padStart(2, "0")}` : null;
  const DOW = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  if (at && dom === "*" && mon === "*" && dow === "*") return `Every day at ${at}${zone}`;
  if (at && dom === "*" && mon === "*" && dow === "1-5") return `Weekdays at ${at}${zone}`;
  if (at && dom === "*" && mon === "*" && /^[0-6]$/.test(dow)) return `Every ${DOW[Number(dow)]} at ${at}${zone}`;
  if (at && /^\d+$/.test(dom) && mon === "*" && dow === "*") return `Monthly on day ${dom} at ${at}${zone}`;
  if (/^\*\/\d+$/.test(min) && hour === "*" && dom === "*" && mon === "*" && dow === "*") return `Every ${min.slice(2)} minutes`;
  if (min === "0" && hour === "*" && dom === "*" && mon === "*" && dow === "*") return "Every hour";
  return cron;
}

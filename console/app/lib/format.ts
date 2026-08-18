// Display formatters. Keep tiny — no date-fns/luxon for Phase 2.

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)}s`;
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}m${String(s).padStart(2, "0")}s`;
}

export function formatRelative(iso: string): string {
  const t = new Date(iso).getTime();
  const now = Date.now();
  const diff = (now - t) / 1000;
  // Future timestamps (e.g. a schedule's next fire) read "in 4s", never
  // the nonsensical "-4s ago".
  const abs = Math.max(0, Math.floor(Math.abs(diff)));
  const span =
    abs < 60
      ? `${abs}s`
      : abs < 3600
        ? `${Math.floor(abs / 60)}m`
        : abs < 86400
          ? `${Math.floor(abs / 3600)}h`
          : `${Math.floor(abs / 86400)}d`;
  return diff < 0 ? `in ${span}` : `${span} ago`;
}

export function formatAbsolute(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

export function formatCost(eur: number | undefined | null): string {
  if (eur == null || eur === 0) return "—";
  return `€${eur.toFixed(4)}`;
}

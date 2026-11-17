import type { ReactNode } from "react";
import { Database, ListChecks, Rocket, Trash2, UserPlus } from "lucide-react";
import type { Decision, DecisionKind } from "../../lib/decisions";
import { askedBy } from "../../lib/decisions";
import { ago } from "../../lib/format";
import { cx, Time } from "../../ui/core";

export const KIND: Record<DecisionKind, { icon: ReactNode; label: string; tint: string }> = {
  step: { icon: <ListChecks />, label: "Approval", tint: "bg-brand-50 text-brand-700 dark:bg-brand-500/15 dark:text-brand-100" },
  "go-live": { icon: <Rocket />, label: "Go-live", tint: "bg-sky-50 text-sky-700 dark:bg-sky-500/15 dark:text-sky-300" },
  data: { icon: <Database />, label: "Data change", tint: "bg-teal-50 text-teal-700 dark:bg-teal-500/15 dark:text-teal-300" },
  deletion: { icon: <Trash2 />, label: "Deletion", tint: "bg-rose-50 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300" },
  guest: { icon: <UserPlus />, label: "Guest access", tint: "bg-amber-50 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300" },
};

export function KindIcon({ kind, size = "md" }: { kind: DecisionKind; size?: "md" | "lg" }) {
  const k = KIND[kind];
  return <span className={cx("flex shrink-0 items-center justify-center rounded-lg", k.tint, size === "lg" ? "size-10 [&_svg]:size-5" : "size-8 [&_svg]:size-4")} aria-hidden>{k.icon}</span>;
}

/** A decision in a list: what, for which app, who asked, when. */
export function DecisionRow({ d, href, active, showApp = true }: { d: Decision; href: string; active?: boolean; showApp?: boolean }) {
  const by = askedBy(d);
  return (
    <a href={href} aria-current={active ? "true" : undefined}
      className={cx("flex items-start gap-3 px-4 py-3 transition-colors", active ? "bg-brand-50/70 dark:bg-brand-500/10" : "hover:bg-surface-2")}>
      <KindIcon kind={d.kind} />
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-2">
          <span className="min-w-0 flex-1 truncate font-medium text-fg">{d.title}</span>
          <span className="shrink-0 text-xs text-fg-4"><Time at={d.at}>{ago(d.at)}</Time></span>
        </span>
        <span className="mt-0.5 block truncate text-[13px] text-fg-3">
          {[showApp && d.appName, KIND[d.kind].label, by && `by ${by}`].filter(Boolean).join(" · ")}
        </span>
      </span>
    </a>
  );
}

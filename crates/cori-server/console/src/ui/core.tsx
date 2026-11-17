import { clsx } from "clsx";
import { forwardRef, useState, type AnchorHTMLAttributes, type ButtonHTMLAttributes, type ComponentProps, type ReactNode, type SelectHTMLAttributes, type TextareaHTMLAttributes } from "react";
import { AlertTriangle, Check, Copy, Info, Loader2, XCircle } from "lucide-react";
import { exact, initials } from "../lib/format";
import { toast } from "./toast";

export const cx = clsx;

// ---- buttons

type Variant = "primary" | "secondary" | "ghost" | "danger" | "danger-soft";
type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "xs" | "sm" | "md"; loading?: boolean; icon?: ReactNode };

const VARIANT: Record<Variant, string> = {
  primary: "bg-brand-600 text-white shadow-xs hover:bg-brand-700 disabled:hover:bg-brand-600",
  secondary: "bg-surface text-fg shadow-xs ring-1 ring-line-2 ring-inset hover:bg-surface-2",
  ghost: "text-fg-2 hover:bg-surface-2 hover:text-fg",
  danger: "bg-rose-600 text-white shadow-xs hover:bg-rose-700",
  "danger-soft": "text-rose-700 hover:bg-rose-50 dark:text-rose-400 dark:hover:bg-rose-500/10",
};
const SIZE = { xs: "h-7 gap-1.5 px-2 text-[13px] [&_svg]:size-3.5", sm: "h-8 gap-1.5 px-3 text-[13px] [&_svg]:size-4", md: "h-9 gap-2 px-3.5 text-sm [&_svg]:size-4" };

const BUTTON = "inline-flex shrink-0 cursor-pointer items-center justify-center rounded-lg font-medium whitespace-nowrap transition-colors select-none disabled:cursor-not-allowed disabled:opacity-50 [&_svg]:shrink-0";

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "sm", loading, icon, className, children, disabled, type = "button", ...rest }, ref) {
  return (
    <button ref={ref} {...rest} type={type} disabled={disabled || loading} className={cx(BUTTON, SIZE[size], VARIANT[variant], className)}>
      {loading ? <Loader2 className="animate-spin" aria-hidden /> : icon}
      {children}
    </button>
  );
});

/** A link that looks like a button (a page to go to, an app to open). `external` opens it in a new tab. */
export function ButtonLink({ variant = "secondary", size = "sm", icon, className, children, external, ...rest }: AnchorHTMLAttributes<HTMLAnchorElement> & { variant?: Variant; size?: "xs" | "sm" | "md"; icon?: ReactNode; external?: boolean }) {
  return (
    <a {...rest} target={external ? "_blank" : rest.target} rel={external ? "noopener noreferrer" : rest.rel} className={cx(BUTTON, SIZE[size], VARIANT[variant], className)}>
      {icon}{children}
    </a>
  );
}

/** A button that is only an icon: `label` names it for screen readers and the tooltip. */
export const IconButton = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & { label: string; size?: "2xs" | "xs" | "sm" }>(function IconButton(
  { label, size = "sm", className, children, type = "button", ...rest }, ref) {
  return (
    <button ref={ref} {...rest} type={type} aria-label={label} title={label}
      className={cx("inline-flex shrink-0 cursor-pointer items-center justify-center text-fg-3 transition-colors hover:bg-surface-2 hover:text-fg disabled:cursor-not-allowed disabled:opacity-40",
        size === "2xs" ? "size-5 rounded-full [&_svg]:size-3" : size === "xs" ? "size-7 rounded-lg [&_svg]:size-3.5" : "size-8 rounded-lg [&_svg]:size-4", className)}>
      {children}
    </button>
  );
});

// ---- form controls

// Controls take their height and type size from `h` (never from a class: two heights in one class list do not override
// each other reliably).
const control = "rounded-lg border-0 bg-surface text-fg shadow-xs ring-1 ring-line-2 ring-inset placeholder:text-fg-4 focus:ring-2 focus:ring-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:bg-surface-2 disabled:text-fg-3";
type H = "xs" | "sm" | "md";
const HEIGHT: Record<H, string> = { xs: "h-7 px-2 text-xs", sm: "h-8 px-2.5 text-[13px]", md: "h-9 px-3 text-[14px]" };
export const Input = forwardRef<HTMLInputElement, ComponentProps<"input"> & { h?: H }>(function Input({ h = "md", className, ...p }, ref) {
  return <input ref={ref} {...p} className={cx(control, HEIGHT[h], className?.includes("w-") ? "" : "w-full", className)} />;
});
export const Select = ({ h = "md", className, ...p }: SelectHTMLAttributes<HTMLSelectElement> & { h?: H }) =>
  <select {...p} className={cx(control, HEIGHT[h], "cursor-pointer pr-8", className?.includes("w-") ? "" : "w-full", className)} />;
export const Textarea = ({ className, ...p }: TextareaHTMLAttributes<HTMLTextAreaElement>) =>
  <textarea {...p} className={cx(control, "w-full px-3 py-2 text-[14px] leading-relaxed", className)} />;

/** A labelled form field: its label, the control, a hint under it, or what is wrong. */
export function Field({ label, hint, error, children, className, optional }: { label: ReactNode; hint?: ReactNode; error?: ReactNode; children: ReactNode; className?: string; optional?: boolean }) {
  return (
    <label className={cx("block", className)}>
      <span className="mb-1.5 block text-[13px] font-medium text-fg">{label}{optional && <span className="ml-1 font-normal text-fg-4">optional</span>}</span>
      {children}
      {error ? <span className="mt-1.5 block text-[13px] text-rose-600 dark:text-rose-400">{error}</span>
        : hint ? <span className="mt-1.5 block text-[13px] leading-snug text-fg-3">{hint}</span> : null}
    </label>
  );
}

/** One choice among a few, as a row of segments. */
export function Segmented<K extends string>({ value, onChange, options, label, size = "sm" }: { value: K; onChange: (k: K) => void; options: { value: K; label: ReactNode; count?: number }[]; label: string; size?: "xs" | "sm" }) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex shrink-0 rounded-lg bg-surface-2 p-0.5 ring-1 ring-line ring-inset">
      {options.map((o) => (
        <button key={o.value} type="button" role="radio" aria-checked={value === o.value} onClick={() => onChange(o.value)}
          className={cx("inline-flex cursor-pointer items-center gap-1.5 rounded-md font-medium whitespace-nowrap transition-colors",
            size === "xs" ? "h-6 px-2 text-xs" : "h-7 px-2.5 text-[13px]",
            value === o.value ? "bg-surface text-fg shadow-xs ring-1 ring-line" : "text-fg-3 hover:text-fg")}>
          {o.label}
          {o.count !== undefined && <span className={cx("rounded px-1 text-[11px] tabular-nums", value === o.value ? "bg-surface-2 text-fg-2" : "text-fg-4")}>{o.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** A choice shown as cards (a setting with two or three meaningful options). */
export function RadioCards<K extends string>({ value, onChange, options, name, disabled }: { value: K; onChange: (k: K) => void; name: string; disabled?: boolean; options: { value: K; title: ReactNode; body?: ReactNode }[] }) {
  return (
    <div className="grid gap-2">
      {options.map((o) => (
        <label key={o.value} className={cx("flex gap-3 rounded-xl p-3.5 ring-1 ring-inset transition-colors",
          disabled ? "cursor-not-allowed opacity-70" : "cursor-pointer",
          value === o.value ? "bg-brand-50/60 ring-brand-500 dark:bg-brand-500/10" : "ring-line hover:bg-surface-2")}>
          <input type="radio" name={name} className="mt-0.5 size-4 shrink-0" checked={value === o.value} disabled={disabled} onChange={() => onChange(o.value)} />
          <span className="min-w-0">
            <span className="block font-medium text-fg">{o.title}</span>
            {o.body && <span className="mt-0.5 block text-[13px] leading-snug text-fg-3">{o.body}</span>}
          </span>
        </label>
      ))}
    </div>
  );
}

// ---- status

export type Tone = "neutral" | "brand" | "success" | "warning" | "danger";
const TONE: Record<Tone, string> = {
  neutral: "bg-surface-2 text-fg-2 ring-line",
  brand: "bg-brand-50 text-brand-700 ring-brand-600/15 dark:bg-brand-500/15 dark:text-brand-100 dark:ring-brand-500/25",
  success: "bg-emerald-50 text-emerald-700 ring-emerald-600/15 dark:bg-emerald-500/10 dark:text-emerald-300 dark:ring-emerald-400/20",
  warning: "bg-amber-50 text-amber-800 ring-amber-600/20 dark:bg-amber-500/10 dark:text-amber-300 dark:ring-amber-400/20",
  danger: "bg-rose-50 text-rose-700 ring-rose-600/15 dark:bg-rose-500/10 dark:text-rose-300 dark:ring-rose-400/20",
};
const DOT: Record<Tone, string> = { neutral: "bg-fg-4", brand: "bg-brand-500", success: "bg-emerald-500", warning: "bg-amber-500", danger: "bg-rose-500" };

export function Badge({ tone = "neutral", children, className, title, dot }: { tone?: Tone; children: ReactNode; className?: string; title?: string; dot?: boolean }) {
  return (
    <span title={title} className={cx("inline-flex h-5.5 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-xs font-medium whitespace-nowrap ring-1 ring-inset", TONE[tone], className)}>
      {dot && <span className={cx("size-1.5 rounded-full", DOT[tone])} aria-hidden />}
      {children}
    </span>
  );
}

/** A coloured dot and a word: the quiet way to say a state in a list. `pulse` for something under way. */
export function StatusText({ tone, children, pulse, className, title }: { tone: Tone; children: ReactNode; pulse?: boolean; className?: string; title?: string }) {
  return (
    <span title={title} className={cx("inline-flex items-center gap-2 whitespace-nowrap", className)}>
      <span className="relative flex size-2 shrink-0">
        {pulse && <span className={cx("absolute inline-flex size-full animate-ping rounded-full opacity-60", DOT[tone])} />}
        <span className={cx("relative inline-flex size-2 rounded-full", DOT[tone])} />
      </span>
      {children}
    </span>
  );
}

/** A run's or a step's status, as a tone and a word. */
export function runTone(status: string): { tone: Tone; label: string; pulse?: boolean } {
  switch (status) {
    case "succeeded": case "ok": case "done": return { tone: "success", label: status === "ok" ? "ok" : "Succeeded" };
    case "failed": case "error": return { tone: "danger", label: "Failed" };
    case "running": case "started": return { tone: "brand", label: "Running", pulse: true };
    case "pending": case "waiting": case "pending_approval": return { tone: "warning", label: "Waiting" };
    case "cancelled": case "canceled": return { tone: "neutral", label: "Cancelled" };
    case "terminated": return { tone: "neutral", label: "Stopped" };
    case "timed_out": return { tone: "danger", label: "Timed out" };
    case "not_taken": case "skipped": return { tone: "neutral", label: "Skipped" };
    default: return { tone: "neutral", label: status };
  }
}
export function RunStatus({ status, className }: { status: string; className?: string }) {
  const t = runTone(status);
  return <StatusText tone={t.tone} pulse={t.pulse} className={className}>{t.label}</StatusText>;
}

// ---- layout

export function PageHeader({ title, description, actions, eyebrow, badges }: { title: ReactNode; description?: ReactNode; actions?: ReactNode; eyebrow?: ReactNode; badges?: ReactNode }) {
  return (
    <header className="mb-6 flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
      <div className="min-w-0">
        {eyebrow && <div className="mb-1 text-[13px] text-fg-3">{eyebrow}</div>}
        <div className="flex flex-wrap items-center gap-2.5">
          <h1 className="text-[22px] leading-tight font-semibold tracking-tight text-fg">{title}</h1>
          {badges}
        </div>
        {description && <p className="mt-1.5 max-w-2xl text-fg-3">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}

export function Section({ title, description, actions, children, className, id }: { title?: ReactNode; description?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string; id?: string }) {
  return (
    <section id={id} className={cx("mt-8 first:mt-0", className)}>
      {(title || actions) && (
        <div className="mb-3 flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
          <div className="min-w-0">
            {title && <h2 className="text-[15px] font-semibold text-fg">{title}</h2>}
            {description && <p className="mt-0.5 text-[13px] text-fg-3">{description}</p>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

export function Card({ className, children, as: As = "div" }: { className?: string; children: ReactNode; as?: "div" | "section" | "li" }) {
  return <As className={cx("rounded-xl bg-surface shadow-xs ring-1 ring-line", className)}>{children}</As>;
}

/** Rows inside a card, divided by hairlines. */
export const List = ({ children, className }: { children: ReactNode; className?: string }) => (
  <Card className={cx("divide-y divide-line overflow-hidden", className)}>{children}</Card>
);

/** A figure with its label, for a row of key numbers. */
export function Stat({ label, value, hint, href, tone }: { label: string; value: ReactNode; hint?: ReactNode; href?: string; tone?: Tone }) {
  const body = (
    <>
      <div className="text-[13px] text-fg-3">{label}</div>
      <div className={cx("mt-1 text-2xl font-semibold tracking-tight tabular-nums", tone === "danger" ? "text-rose-600 dark:text-rose-400" : tone === "warning" ? "text-amber-600 dark:text-amber-400" : "text-fg")}>{value}</div>
      {hint && <div className="mt-0.5 truncate text-[13px] text-fg-3">{hint}</div>}
    </>
  );
  return href
    ? <a href={href} className="block rounded-xl bg-surface px-4 py-3.5 shadow-xs ring-1 ring-line transition-colors hover:bg-surface-2">{body}</a>
    : <div className="rounded-xl bg-surface px-4 py-3.5 shadow-xs ring-1 ring-line">{body}</div>;
}

/** Facts as label/value pairs. */
export function Facts({ items, className }: { items: [ReactNode, ReactNode][]; className?: string }) {
  return (
    <dl className={cx("grid grid-cols-[minmax(5.5rem,auto)_1fr] gap-x-5 gap-y-2.5 text-[14px] sm:grid-cols-[minmax(8rem,auto)_1fr]", className)}>
      {items.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="pt-px text-[13px] text-fg-3">{k}</dt>
          <dd className="min-w-0 break-words text-fg">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

// ---- states

export function Empty({ icon, title, children, action, className, compact }: { icon?: ReactNode; title: ReactNode; children?: ReactNode; action?: ReactNode; className?: string; compact?: boolean }) {
  return (
    <div className={cx("flex flex-col items-center rounded-xl border border-dashed border-line-2 text-center", compact ? "px-4 py-6" : "px-6 py-12", className)}>
      {icon && <div className="mb-3 flex size-10 items-center justify-center rounded-full bg-surface-2 text-fg-3 [&_svg]:size-5">{icon}</div>}
      <div className="font-medium text-fg">{title}</div>
      {children && <div className="mt-1 max-w-md text-[13px] leading-relaxed text-fg-3">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function ErrorNote({ children, retry }: { children: ReactNode; retry?: () => void }) {
  return (
    <div role="alert" className="flex items-start gap-3 rounded-xl bg-rose-50 px-4 py-3 text-rose-800 ring-1 ring-rose-200 ring-inset dark:bg-rose-500/10 dark:text-rose-200 dark:ring-rose-500/20">
      <XCircle className="mt-0.5 size-4 shrink-0" aria-hidden />
      <div className="min-w-0 flex-1 break-words">{children}</div>
      {retry && <button type="button" onClick={retry} className="shrink-0 cursor-pointer text-[13px] font-medium underline-offset-2 hover:underline">Try again</button>}
    </div>
  );
}

/** A short message in a page: `info` explains, `warning` asks for attention, `danger` says something is wrong. */
export function Callout({ tone = "info", title, children, action, className }: { tone?: "info" | "warning" | "danger" | "success"; title?: ReactNode; children?: ReactNode; action?: ReactNode; className?: string }) {
  const style = {
    info: "bg-surface-2 text-fg-2 ring-line",
    success: "bg-emerald-50 text-emerald-900 ring-emerald-200 dark:bg-emerald-500/10 dark:text-emerald-100 dark:ring-emerald-500/20",
    warning: "bg-amber-50 text-amber-900 ring-amber-200 dark:bg-amber-500/10 dark:text-amber-100 dark:ring-amber-500/20",
    danger: "bg-rose-50 text-rose-900 ring-rose-200 dark:bg-rose-500/10 dark:text-rose-100 dark:ring-rose-500/20",
  }[tone];
  const Icon = tone === "info" ? Info : tone === "success" ? Check : AlertTriangle;
  return (
    <div className={cx("flex items-start gap-3 rounded-xl px-4 py-3 ring-1 ring-inset", style, className)}>
      <Icon className="mt-0.5 size-4 shrink-0 opacity-80" aria-hidden />
      <div className="min-w-0 flex-1 text-[13.5px] leading-relaxed">
        {title && <div className="font-medium">{title}</div>}
        {children && <div className={cx(title && "mt-0.5 opacity-90")}>{children}</div>}
      </div>
      {action && <div className="shrink-0 self-center">{action}</div>}
    </div>
  );
}

export function Spinner({ label = "Loading", className }: { label?: string; className?: string }) {
  return <div role="status" className={cx("flex items-center gap-2 py-8 text-fg-3", className)}><Loader2 className="size-4 animate-spin" aria-hidden />{label}…</div>;
}

/** Grey bars where content will be, while it loads. */
export function Skeleton({ className }: { className?: string }) {
  return <div className={cx("animate-pulse rounded-md bg-surface-3/70", className)} aria-hidden />;
}
export function SkeletonRows({ rows = 4 }: { rows?: number }) {
  return (
    <List>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-4 px-4 py-3.5">
          <Skeleton className="size-2 rounded-full" />
          <Skeleton className="h-3.5 w-40" />
          <Skeleton className="ml-auto h-3.5 w-20" />
        </div>
      ))}
    </List>
  );
}

// ---- small pieces

export const Kbd = ({ children, className }: { children: ReactNode; className?: string }) => (
  <kbd className={cx("inline-flex h-5 min-w-5 items-center justify-center rounded border border-line-2 bg-surface px-1 font-sans text-[11px] font-medium text-fg-3", className)}>{children}</kbd>
);

/** A time, relative ("5 min ago"), exact on hover. */
export const Time = ({ at, children }: { at?: string | null; children: ReactNode }) => <time dateTime={at || undefined} title={exact(at)}>{children}</time>;

const AVATAR_TONES = ["bg-violet-100 text-violet-700", "bg-sky-100 text-sky-700", "bg-emerald-100 text-emerald-700", "bg-amber-100 text-amber-800", "bg-rose-100 text-rose-700", "bg-teal-100 text-teal-700", "bg-indigo-100 text-indigo-700"];
const AVATAR_TONES_DARK = ["dark:bg-violet-500/20 dark:text-violet-200", "dark:bg-sky-500/20 dark:text-sky-200", "dark:bg-emerald-500/20 dark:text-emerald-200", "dark:bg-amber-500/20 dark:text-amber-200", "dark:bg-rose-500/20 dark:text-rose-200", "dark:bg-teal-500/20 dark:text-teal-200", "dark:bg-indigo-500/20 dark:text-indigo-200"];
export function Avatar({ name, size = "md", className }: { name: string; size?: "xs" | "sm" | "md" | "lg"; className?: string }) {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  const i = h % AVATAR_TONES.length;
  return (
    <span aria-hidden className={cx("inline-flex shrink-0 items-center justify-center rounded-full font-semibold select-none", AVATAR_TONES[i], AVATAR_TONES_DARK[i],
      size === "xs" ? "size-5 text-[9px]" : size === "sm" ? "size-6 text-[10px]" : size === "lg" ? "size-10 text-sm" : "size-8 text-xs", className)}>
      {initials(name)}
    </span>
  );
}

/** A command, URL or prompt to copy, with its copy button. */
export function CopyField({ value, label = "Copy", className, multiline }: { value: string; label?: string; className?: string; multiline?: boolean }) {
  const [done, setDone] = useState(false);
  const copy = () => navigator.clipboard?.writeText(value).then(() => { setDone(true); setTimeout(() => setDone(false), 1500); }, () => toast.error("The browser refused the clipboard: select the text instead"));
  return (
    <div className={cx("group flex items-start gap-1 rounded-lg bg-surface-2 py-1 pr-1 pl-3 ring-1 ring-line ring-inset", className)}>
      <code className={cx("min-w-0 flex-1 py-1 font-mono text-[12.5px] leading-relaxed text-fg", multiline ? "break-words whitespace-pre-wrap" : "overflow-x-auto whitespace-nowrap no-scrollbar")}>{value}</code>
      <IconButton label={done ? "Copied" : label} size="xs" onClick={copy} className="text-fg-3">{done ? <Check className="text-emerald-600" /> : <Copy />}</IconButton>
    </div>
  );
}

/** Copies `value` and says so. */
export function copy(value: string, what = "Copied") {
  navigator.clipboard?.writeText(value).then(() => toast.success(what), () => toast.error("The browser refused the clipboard"));
}

/** Tabs that are links (each tab a route). */
export function TabLinks({ tabs, active, label }: { tabs: { key: string; label: ReactNode; href: string; count?: number }[]; active: string; label: string }) {
  return (
    <nav aria-label={label} className="no-scrollbar -mb-px flex gap-1 overflow-x-auto">
      {tabs.map((t) => (
        <a key={t.key} href={t.href} aria-current={t.key === active ? "page" : undefined}
          className={cx("relative flex h-10 shrink-0 items-center gap-1.5 rounded-t-md px-3 text-[14px] font-medium transition-colors",
            t.key === active ? "text-fg after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-brand-600" : "text-fg-3 hover:text-fg")}>
          {t.label}
          {!!t.count && <span className="rounded-full bg-amber-500 px-1.5 text-[11px] leading-[18px] font-semibold text-white tabular-nums">{t.count}</span>}
        </a>
      ))}
    </nav>
  );
}

/** The plain words for a thing that may be empty. */
export const Dash = () => <span className="text-fg-4">—</span>;
export const Muted = ({ children, className }: { children: ReactNode; className?: string }) => <span className={cx("text-fg-3", className)}>{children}</span>;

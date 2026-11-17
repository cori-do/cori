import { clsx } from "clsx";
import { AlertTriangle, Inbox, Loader2, Upload } from "lucide-react";
import { useState } from "react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";

export const cx = clsx;

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "secondary" | "ghost" | "danger"; size?: "sm" | "md"; loading?: boolean };
export function Button({ variant = "secondary", size = "md", loading, className, children, disabled, ...rest }: ButtonProps) {
  return (
    <button
      {...rest}
      disabled={disabled || loading}
      className={cx(
        "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand-500 disabled:cursor-not-allowed disabled:opacity-50",
        size === "sm" ? "h-8 px-2.5 text-xs" : "h-9 px-3.5 text-sm",
        variant === "primary" && "bg-brand-600 text-white shadow-sm hover:bg-brand-500",
        variant === "secondary" && "bg-white text-zinc-800 shadow-sm ring-1 ring-zinc-200 hover:bg-zinc-50 dark:bg-zinc-900 dark:text-zinc-100 dark:ring-zinc-800 dark:hover:bg-zinc-800",
        variant === "ghost" && "text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100",
        variant === "danger" && "bg-rose-600 text-white shadow-sm hover:bg-rose-500",
        className,
      )}
    >
      {loading && <Loader2 className="size-4 animate-spin" aria-hidden />}
      {children}
    </button>
  );
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <section className={cx("rounded-xl bg-white shadow-sm ring-1 ring-zinc-200 dark:bg-zinc-900 dark:ring-zinc-800", className)}>{children}</section>;
}

export function CardHeader({ title, description, action }: { title: ReactNode; description?: ReactNode; action?: ReactNode }) {
  return (
    <header className="flex items-start justify-between gap-3 border-b border-zinc-100 px-5 py-4 dark:border-zinc-800">
      <div>
        <h2 className="text-sm font-semibold">{title}</h2>
        {description && <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">{description}</p>}
      </div>
      {action}
    </header>
  );
}

export function CardBody({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cx("px-5 py-4", className)}>{children}</div>;
}

const TONES = {
  neutral: "bg-zinc-100 text-zinc-700 ring-zinc-500/10 dark:bg-zinc-800 dark:text-zinc-300 dark:ring-zinc-400/20",
  brand: "bg-brand-50 text-brand-700 ring-brand-600/15 dark:bg-brand-500/10 dark:text-brand-100 dark:ring-brand-500/25",
  green: "bg-emerald-50 text-emerald-700 ring-emerald-600/15 dark:bg-emerald-500/10 dark:text-emerald-300 dark:ring-emerald-400/25",
  amber: "bg-amber-50 text-amber-800 ring-amber-600/20 dark:bg-amber-500/10 dark:text-amber-300 dark:ring-amber-400/25",
  red: "bg-rose-50 text-rose-700 ring-rose-600/15 dark:bg-rose-500/10 dark:text-rose-300 dark:ring-rose-400/25",
};
export type Tone = keyof typeof TONES;
export function Badge({ tone = "neutral", children, className }: { tone?: Tone; children: ReactNode; className?: string }) {
  return <span className={cx("inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs font-medium ring-1 ring-inset", TONES[tone], className)}>{children}</span>;
}

/** A labelled form control; pass the input, select or textarea as children. */
export function Field({ label, hint, error, optional, htmlFor, children }: { label: string; hint?: string; error?: string; optional?: boolean; htmlFor: string; children: ReactNode }) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={htmlFor} className="flex items-baseline gap-2 text-sm font-medium">
        {label}
        {optional && <span className="text-xs font-normal text-zinc-400">optional</span>}
      </label>
      {children}
      {error ? <p className="text-xs text-rose-600 dark:text-rose-400">{error}</p> : hint && <p className="text-xs text-zinc-500 dark:text-zinc-400">{hint}</p>}
    </div>
  );
}

const control = "block w-full rounded-lg border-0 bg-white px-3 py-2 text-sm shadow-sm ring-1 ring-zinc-300 placeholder:text-zinc-400 focus:ring-2 focus:ring-brand-500 focus:outline-none dark:bg-zinc-900 dark:ring-zinc-700";
export const Input = (p: InputHTMLAttributes<HTMLInputElement>) => <input {...p} className={cx(control, "h-9", p.className)} />;
export const Select = (p: SelectHTMLAttributes<HTMLSelectElement>) => <select {...p} className={cx(control, "h-9", p.className)} />;
export const Textarea = (p: TextareaHTMLAttributes<HTMLTextAreaElement>) => <textarea {...p} className={cx(control, "min-h-24", p.className)} />;

/** A file picker for a `file` parameter (`multiple` for `files`): click or drop. Give the chosen Files to the workflow's
 *  `run()` / `start()`, which uploads them first; `accept` (as the parameter's: `application/pdf,image/*`) filters the
 *  picker only, the server checks every file. */
export function FileInput({ id, accept, multiple, files, onChange, disabled }: { id: string; accept?: string; multiple?: boolean; files: File[]; onChange: (files: File[]) => void; disabled?: boolean }) {
  const [over, setOver] = useState(false);
  const pick = (list: FileList | null) => { const all = Array.from(list ?? []); onChange(multiple ? all : all.slice(0, 1)); };
  return (
    <label htmlFor={id} onDragOver={(e) => { e.preventDefault(); if (!disabled) setOver(true); }} onDragLeave={() => setOver(false)} onDrop={(e) => { e.preventDefault(); setOver(false); if (!disabled) pick(e.dataTransfer.files); }}
      className={cx("flex min-h-20 w-full cursor-pointer flex-col items-center justify-center gap-1 rounded-lg border-2 border-dashed bg-white px-3 py-3 text-center text-sm focus-within:ring-2 focus-within:ring-brand-500 dark:bg-zinc-900",
        over ? "border-brand-500" : "border-zinc-300 dark:border-zinc-700", disabled && "cursor-not-allowed opacity-60")}>
      <input id={id} type="file" className="sr-only" accept={accept} multiple={multiple} disabled={disabled} onChange={(e) => pick(e.target.files)} />
      <Upload className="size-4 text-zinc-400" aria-hidden />
      {files.length ? <span className="break-words">{files.map((f) => f.name).join(", ")}</span> : <span className="text-zinc-500 dark:text-zinc-400">Drop {multiple ? "files" : "a file"} here, or click to choose</span>}
    </label>
  );
}

export function Spinner({ label = "Loading…" }: { label?: string }) {
  return (
    <div role="status" className="flex items-center gap-2 py-8 text-sm text-zinc-500 justify-center">
      <Loader2 className="size-4 animate-spin" aria-hidden /> {label}
    </div>
  );
}

export function EmptyState({ title, description, action, icon = <Inbox className="size-5" /> }: { title: string; description?: ReactNode; action?: ReactNode; icon?: ReactNode }) {
  return (
    <div className="flex flex-col items-center px-6 py-12 text-center">
      <div className="mb-3 grid size-10 place-items-center rounded-full bg-zinc-100 text-zinc-500 dark:bg-zinc-800">{icon}</div>
      <h3 className="text-sm font-semibold">{title}</h3>
      {description && <p className="mt-1 max-w-sm text-sm text-zinc-500 dark:text-zinc-400">{description}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function ErrorBanner({ title = "Something went wrong", message, onRetry }: { title?: string; message: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex items-start gap-3 rounded-lg bg-rose-50 p-4 text-sm text-rose-800 ring-1 ring-rose-200 dark:bg-rose-950/40 dark:text-rose-200 dark:ring-rose-900">
      <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
      <div className="flex-1">
        <p className="font-medium">{title}</p>
        <p className="mt-0.5">{message}</p>
      </div>
      {onRetry && <Button size="sm" variant="secondary" onClick={onRetry}>Retry</Button>}
    </div>
  );
}

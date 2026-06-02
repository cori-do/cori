// Middle-truncate a string at any container width, keeping the
// load-bearing tail intact. CSS-only: head shrinks with ellipsis,
// tail uses `flex-shrink: 0`. No ResizeObserver needed — flex layout
// re-measures naturally on container width changes.
//
// Example: a breadcrumb `github.com/acme/flows @ v2.3.1 · a1b9f4c`
// with tail length 22 keeps `… @ v2.3.1 · a1b9f4c` visible at any
// narrow width, eliding only the leading host/path.

interface MiddleTruncateProps {
  /** Full string to render. */
  text: string;
  /**
   * Number of trailing characters to preserve un-elided. Defaults to
   * half the string, capped at 16 (sensible for breadcrumbs / paths).
   */
  tail?: number;
  className?: string;
  title?: string;
}

export function MiddleTruncate({
  text,
  tail,
  className,
  title,
}: MiddleTruncateProps) {
  const tailLen = Math.min(text.length, tail ?? Math.min(16, Math.floor(text.length / 2)));
  // Nothing to truncate if the tail is the whole string.
  if (tailLen >= text.length) {
    return (
      <span className={className} title={title ?? text}>
        {text}
      </span>
    );
  }
  const head = text.slice(0, text.length - tailLen);
  const tailStr = text.slice(text.length - tailLen);
  return (
    <span
      className={`middle-truncate${className ? ` ${className}` : ""}`}
      title={title ?? text}
    >
      <span className="middle-truncate-head">{head}</span>
      <span className="middle-truncate-tail">{tailStr}</span>
    </span>
  );
}

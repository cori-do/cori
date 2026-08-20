// Rich rendering for workflow output — the three things a raw <pre>
// can't do: markdown from LLM steps, HTML documents (rendered inside a
// sandboxed iframe, never injected into the app's own DOM), and
// one-click copy of any JSON/source payload.
//
// The markdown renderer emits React elements directly — no
// dangerouslySetInnerHTML anywhere in this file, so untrusted step
// output can never script the Console. HTML only ever renders inside
// an iframe with scripts disabled.

import {
  createElement,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

// ── Copy button ───────────────────────────────────────────────────────

export function CopyButton({
  text,
  label = "copy",
}: {
  /** Value to copy; pass a function to defer stringification until click. */
  text: string | (() => string);
  label?: string;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(null);
  return (
    <button
      type="button"
      className={`copy-chip${copied ? " is-copied" : ""}`}
      onClick={() => {
        const value = typeof text === "function" ? text() : text;
        void navigator.clipboard?.writeText(value).catch(() => {});
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "copied" : label}
    </button>
  );
}

/** A <pre> of pretty-printed JSON with a copy button in its corner. */
export function JsonBlock({
  value,
  className,
}: {
  value: unknown;
  className?: string;
}) {
  const text = JSON.stringify(value, null, 2) ?? "null";
  return (
    <div className="json-block">
      <CopyButton text={text} label="copy json" />
      <pre className={className}>{text}</pre>
    </div>
  );
}

// ── Content-type detection ────────────────────────────────────────────

/** True when the string reads as an HTML document/fragment rather than
 *  text that merely mentions a tag. */
export function looksLikeHtml(value: string): boolean {
  const head = value.trimStart().slice(0, 256).toLowerCase();
  if (head.startsWith("<!doctype") || head.startsWith("<html")) return true;
  const tags = value.match(/<\/?[a-z][a-z0-9-]*(?:\s[^<>]*)?>/gi) ?? [];
  return (
    tags.length >= 3 &&
    /<\/(?:div|p|table|tbody|body|section|article|span|h[1-6]|ul|ol|li|td|tr|th|a|strong|em|b|i)>/i.test(
      value,
    )
  );
}

/**
 * String content, rendered by what it is: HTML gets the sandboxed
 * frame (with the source one click away), everything else goes through
 * the markdown renderer — which degrades to plain paragraphs when the
 * text has no markdown in it.
 */
export function RichText({ value }: { value: string }) {
  if (looksLikeHtml(value)) return <HtmlFrame html={value} />;
  return <Markdown source={value} />;
}

// ── Sandboxed HTML preview ────────────────────────────────────────────

const FRAME_MAX_HEIGHT = 640;

export function HtmlFrame({ html }: { html: string }) {
  const [height, setHeight] = useState(180);
  const [showSource, setShowSource] = useState(false);
  const isDocument = /^\s*(?:<!doctype|<html)/i.test(html);
  const srcDoc = isDocument
    ? html
    : `<!doctype html><html><head><meta charset="utf-8"><style>body{margin:12px;font:13px/1.55 -apple-system,system-ui,sans-serif;color:#1c2333}</style></head><body>${html}</body></html>`;
  return (
    <div className="html-frame">
      <div className="html-frame-bar">
        <span className="html-frame-note">rendered html · scripts off</span>
        <button
          type="button"
          className="copy-chip"
          onClick={() => setShowSource((s) => !s)}
        >
          {showSource ? "preview" : "source"}
        </button>
        <CopyButton text={html} label="copy source" />
      </div>
      {showSource ? (
        <pre className="html-frame-source">{html}</pre>
      ) : (
        <iframe
          className="html-frame-view"
          title="Rendered HTML output"
          // Same-origin (so we can size the frame and route link clicks
          // to the system browser) but no scripts, forms, or popups.
          sandbox="allow-same-origin"
          srcDoc={srcDoc}
          style={{ height }}
          onLoad={(e) => {
            const frame = e.currentTarget;
            const doc = frame.contentDocument;
            if (!doc) return;
            const measured = doc.documentElement?.scrollHeight ?? 0;
            setHeight(Math.min(Math.max(measured + 4, 60), FRAME_MAX_HEIGHT));
            // Links escape to the system browser; nothing navigates the frame.
            doc.addEventListener("click", (ev) => {
              const anchor = (ev.target as Element | null)?.closest?.("a");
              if (!anchor) return;
              ev.preventDefault();
              const href = anchor.getAttribute("href");
              if (href && /^https?:\/\//i.test(href)) {
                void openUrl(href).catch(() => {});
              }
            });
          }}
        />
      )}
    </div>
  );
}

// ── Markdown → React elements ─────────────────────────────────────────
//
// Deliberately small: headings, lists, fenced code, tables, quotes,
// rules, links, emphasis, inline code. Plain text passes through as
// paragraphs with preserved line breaks.

export function Markdown({ source }: { source: string }) {
  return <div className="md">{renderBlocks(source)}</div>;
}

const BLOCK_START =
  /^\s*(?:#{1,6}\s|>|```|~~~|(?:[-*+]|\d{1,3}[.)])\s)/;

function renderBlocks(source: string): ReactNode[] {
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  const out: ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }

    const fence = line.match(/^\s*(```|~~~)\s*(\S*)\s*$/);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) {
        body.push(lines[i]);
        i++;
      }
      i++; // closing fence
      out.push(
        <pre key={key++} data-lang={fence[2] || undefined}>
          <code>{body.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      out.push(
        createElement(
          `h${heading[1].length}`,
          { key: key++ },
          renderInline(heading[2].replace(/\s#+\s*$/, "")),
        ),
      );
      i++;
      continue;
    }

    if (/^\s*([-*_])\s*(?:\1\s*){2,}$/.test(line)) {
      out.push(<hr key={key++} />);
      i++;
      continue;
    }

    if (/^\s*>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) {
        body.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      out.push(<blockquote key={key++}>{renderBlocks(body.join("\n"))}</blockquote>);
      continue;
    }

    if (
      line.includes("|") &&
      i + 1 < lines.length &&
      /^\s*\|?\s*:?-{2,}[\s:|-]*$/.test(lines[i + 1])
    ) {
      const header = splitTableRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) {
        rows.push(splitTableRow(lines[i]));
        i++;
      }
      out.push(
        <div className="md-table-wrap" key={key++}>
          <table>
            <thead>
              <tr>
                {header.map((cell, c) => (
                  <th key={c}>{renderInline(cell)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, r) => (
                <tr key={r}>
                  {header.map((_, c) => (
                    <td key={c}>{renderInline(row[c] ?? "")}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      continue;
    }

    const listMark = line.match(/^(\s*)([-*+]|\d{1,3}[.)])\s+/);
    if (listMark) {
      const indent = listMark[1].length;
      const ordered = /^\d/.test(listMark[2]);
      const items: string[][] = [];
      while (i < lines.length) {
        const m = lines[i].match(/^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$/);
        if (m && m[1].length <= indent + 1) {
          items.push([m[3]]);
          i++;
          continue;
        }
        // Indented continuation (wrapped text or a nested list).
        if (items.length > 0 && lines[i].trim() && /^\s{2,}/.test(lines[i])) {
          items[items.length - 1].push(lines[i].replace(/^ {2,4}/, ""));
          i++;
          continue;
        }
        break;
      }
      const rendered = items.map((item, n) => (
        <li key={n}>
          {item.length === 1 ? renderInline(item[0]) : renderBlocks(item.join("\n"))}
        </li>
      ));
      out.push(
        ordered ? <ol key={key++}>{rendered}</ol> : <ul key={key++}>{rendered}</ul>,
      );
      continue;
    }

    // Paragraph: consecutive plain lines, single newlines kept as breaks.
    const body = [line];
    i++;
    while (i < lines.length && lines[i].trim() && !BLOCK_START.test(lines[i])) {
      body.push(lines[i]);
      i++;
    }
    out.push(
      <p key={key++}>
        {body.flatMap((text, n) =>
          n === 0
            ? renderInline(text)
            : [<br key={`br${n}`} />, ...renderInline(text)],
        )}
      </p>,
    );
  }

  return out;
}

function splitTableRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((cell) => cell.trim());
}

const INLINE_RE =
  /(`[^`\n]+`)|(!?\[[^\]\n]*\]\([^)\n]+\))|(\*\*[^*\n]+\*\*)|(__[^_\n]+__)|(\*[^*\s][^*\n]*\*)|(_[^_\s][^_\n]*_)|(~~[^~\n]+~~)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/;

function renderInline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let rest = text;
  let key = 0;
  while (rest.length > 0) {
    const match = INLINE_RE.exec(rest);
    if (!match || match.index == null) {
      out.push(rest);
      break;
    }
    if (match.index > 0) out.push(rest.slice(0, match.index));
    const token = match[0];
    if (match[1]) {
      out.push(<code key={key++}>{token.slice(1, -1)}</code>);
    } else if (match[2]) {
      const link = token.match(/^(!?)\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)$/);
      if (link) {
        const [, , label, url] = link;
        out.push(<MdLink key={key++} url={url} label={label || url} />);
      } else {
        out.push(token);
      }
    } else if (match[3] || match[4]) {
      out.push(<strong key={key++}>{renderInline(token.slice(2, -2))}</strong>);
    } else if (match[5] || match[6]) {
      out.push(<em key={key++}>{renderInline(token.slice(1, -1))}</em>);
    } else if (match[7]) {
      out.push(<del key={key++}>{renderInline(token.slice(2, -2))}</del>);
    } else if (match[8]) {
      out.push(<MdLink key={key++} url={token} label={token} />);
    }
    rest = rest.slice(match.index + token.length);
  }
  return out;
}

function MdLink({ url, label }: { url: string; label: string }) {
  const external = /^https?:\/\//i.test(url);
  return (
    <a
      href={url}
      title={url}
      onClick={(e) => {
        e.preventDefault();
        if (external) void openUrl(url).catch(() => {});
      }}
    >
      {label}
    </a>
  );
}

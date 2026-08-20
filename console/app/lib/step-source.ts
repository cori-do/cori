// Pull the "what runs" expression out of a step's TypeScript source for
// the inspector: the `command` argv builder of a `cli` step, the `run`
// function of a `code` step, the `prompt` of an `llm` step, the `args`
// of an `mcp_tool` step, the selector function of a builtin.
//
// Step files follow the canonical SDK shape the compiler already
// enforces (`export default step.<kind>({ ... })`), so a balanced-brace
// scan is enough — no TypeScript parser. The scanner is string-,
// template- and comment-aware; when a file deviates, extraction returns
// null and the inspector falls back to the full source.

import type { StepSummary } from "./api";

/** Index just past a comment / string / template starting at `i`, or
 *  null when `i` starts none of them. */
function skipInert(src: string, i: number): number | null {
  const c = src[i];
  if (c === "/" && src[i + 1] === "/") {
    const nl = src.indexOf("\n", i);
    return nl === -1 ? src.length : nl + 1;
  }
  if (c === "/" && src[i + 1] === "*") {
    const end = src.indexOf("*/", i + 2);
    return end === -1 ? src.length : end + 2;
  }
  if (c === '"' || c === "'") {
    for (let j = i + 1; j < src.length; j += 1) {
      if (src[j] === "\\") j += 1;
      else if (src[j] === c || src[j] === "\n") return j + 1;
    }
    return src.length;
  }
  if (c === "`") {
    for (let j = i + 1; j < src.length; j += 1) {
      if (src[j] === "\\") j += 1;
      else if (src[j] === "$" && src[j + 1] === "{") {
        j = skipBalanced(src, j + 1) - 1;
      } else if (src[j] === "`") {
        return j + 1;
      }
    }
    return src.length;
  }
  return null;
}

const OPENERS = "({[";
const CLOSERS = ")}]";

/** `src[start]` is an opener; index just past its matching closer. */
function skipBalanced(src: string, start: number): number {
  let depth = 0;
  let j = start;
  while (j < src.length) {
    const inert = skipInert(src, j);
    if (inert != null) {
      j = inert;
      continue;
    }
    const c = src[j];
    if (OPENERS.includes(c)) depth += 1;
    else if (CLOSERS.includes(c)) {
      depth -= 1;
      if (depth === 0) return j + 1;
    }
    j += 1;
  }
  return src.length;
}

/** The inside of the `{...}` options object of
 *  `export default step.<kind>({ ... })`, or null. */
function optionsSpan(source: string): string | null {
  const head = /export\s+default\s+step\.[A-Za-z_][A-Za-z0-9_]*\s*\(/.exec(
    source,
  );
  if (!head) return null;
  let i = head.index + head[0].length;
  while (i < source.length && /\s/.test(source[i])) i += 1;
  if (source[i] !== "{") return null;
  const end = skipBalanced(source, i);
  return source.slice(i + 1, end - 1);
}

const IDENT = /[A-Za-z0-9_$]/;

/** Strip the common indentation of every line after the first, so a
 *  snippet lifted from a nested object reads flush-left. */
function dedent(snippet: string): string {
  const lines = snippet.split("\n");
  if (lines.length < 2) return snippet;
  let common = Infinity;
  for (const line of lines.slice(1)) {
    if (line.trim().length === 0) continue;
    const indent = line.length - line.trimStart().length;
    common = Math.min(common, indent);
  }
  if (!Number.isFinite(common) || common === 0) return snippet;
  return [lines[0], ...lines.slice(1).map((l) => l.slice(common))].join("\n");
}

/**
 * The verbatim expression of one top-level field (`command`, `run`,
 * `prompt`, `args`, `if`, `on`, `over`, `until`, `for`, …) in the step's
 * options object. Null when the file does not follow the canonical
 * shape or the field is absent.
 */
export function stepField(source: string, field: string): string | null {
  const span = optionsSpan(source);
  if (span == null) return null;
  let j = 0;
  while (j < span.length) {
    const inert = skipInert(span, j);
    if (inert != null) {
      j = inert;
      continue;
    }
    const c = span[j];
    if (OPENERS.includes(c)) {
      j = skipBalanced(span, j);
      continue;
    }
    // A top-level `field:` — the key must sit on an identifier boundary.
    if (
      span.startsWith(field, j) &&
      (j === 0 || !IDENT.test(span[j - 1])) &&
      IDENT.test(field[0])
    ) {
      let k = j + field.length;
      while (k < span.length && /\s/.test(span[k])) k += 1;
      if (span[k] === ":") {
        k += 1;
        while (k < span.length && /\s/.test(span[k])) k += 1;
        return dedent(span.slice(k, valueEnd(span, k)).trimEnd());
      }
    }
    j += 1;
  }
  return null;
}

/** End of the value starting at `from`: the next `,` at value depth. */
function valueEnd(span: string, from: number): number {
  let j = from;
  while (j < span.length) {
    const inert = skipInert(span, j);
    if (inert != null) {
      j = inert;
      continue;
    }
    const c = span[j];
    if (OPENERS.includes(c)) {
      j = skipBalanced(span, j);
      continue;
    }
    if (c === ",") return j;
    // An arrow body without braces may contain `?:` — a lone `:` never
    // ends a value, only the comma does.
    j += 1;
  }
  return span.length;
}

/** Which field carries a step's actual work, per kind. */
export function whatRuns(
  step: StepSummary,
): { key: string; field: string } | null {
  switch (step.kind) {
    case "cli":
      return { key: "command", field: "command" };
    case "code":
      return { key: "code", field: "run" };
    case "llm":
      return { key: "prompt", field: "prompt" };
    case "mcp_tool":
      return { key: "args", field: "args" };
    case "builtin":
      switch (step.builtin) {
        case "branch":
          return { key: "if", field: "if" };
        case "switch":
          return { key: "on", field: "on" };
        case "for_each":
          return { key: "over", field: "over" };
        case "loop":
          return { key: "until", field: "until" };
        case "wait":
          return { key: "for", field: "for" };
        default:
          return null;
      }
    default:
      return null;
  }
}
